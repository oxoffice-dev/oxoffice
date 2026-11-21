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

// Paired source events prove physical correspondence while permitting only
// namespace declarations to differ. Non-namespace source remains exact.
#[allow(clippy::type_complexity)] // Concrete physical map and changed namespace offsets form one proof.
pub(crate) fn comment_namespace_source_boundaries(
    canonical: &[u8],
    restored: &[u8],
) -> Result<Option<(BTreeMap<usize, usize>, Vec<usize>)>> {
    let mut left = NsReader::from_reader(canonical);
    let mut right = NsReader::from_reader(restored);
    let mut boundaries = BTreeMap::new();
    let mut changed_namespace_facts = Vec::new();
    loop {
        let left_start = left.buffer_position() as usize;
        let right_start = right.buffer_position() as usize;
        let a = left.read_event().map_err(|e| Error::Other(e.to_string()))?;
        let b = right
            .read_event()
            .map_err(|e| Error::Other(e.to_string()))?;
        let same = match (&a, &b) {
            (Event::Start(a), Event::Start(b)) | (Event::Empty(a), Event::Empty(b)) => {
                let attributes = |element: &BytesStart<'_>| -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
                    element
                        .attributes()
                        .filter_map(|attribute| match attribute {
                            Ok(attribute)
                                if attribute.key.as_ref() == b"xmlns"
                                    || attribute.key.as_ref().starts_with(b"xmlns:") =>
                            {
                                None
                            }
                            Ok(attribute) => Some(Ok((
                                attribute.key.as_ref().to_vec(),
                                attribute.value.as_ref().to_vec(),
                            ))),
                            Err(error) => Some(Err(Error::Other(error.to_string()))),
                        })
                        .collect()
                };
                let a_attributes = attributes(a)?;
                let b_attributes = attributes(b)?;
                let same_namespaces = left.resolver().resolve_element(a.name()).0
                    == right.resolver().resolve_element(b.name()).0
                    && a.attributes()
                        .filter_map(|attribute| attribute.ok())
                        .filter(|attribute| {
                            attribute.key.as_ref() != b"xmlns"
                                && !attribute.key.as_ref().starts_with(b"xmlns:")
                        })
                        .all(|attribute| {
                            left.resolver().resolve_attribute(attribute.key).0
                                == right.resolver().resolve_attribute(attribute.key).0
                        });
                if !same_namespaces {
                    changed_namespace_facts.push(left_start);
                }
                a.name() == b.name() && a_attributes == b_attributes
            }
            _ => a == b,
        };
        if !same {
            return Ok(None);
        }
        boundaries.insert(left_start, right_start);
        boundaries.insert(
            left.buffer_position() as usize,
            right.buffer_position() as usize,
        );
        if matches!(a, Event::Eof) {
            break;
        }
    }
    Ok(Some((boundaries, changed_namespace_facts)))
}

// Existing checked paragraph paths shared by discovery and rich projection.
// Neither consumer invents a second index axis. Every owner of one source and
// every control in it is scanned from its own excerpt, with each family of
// scopes inventoried in one pass, so no owner or control rereads the source
// from its start. One list per owner, in the order given.
#[allow(clippy::type_complexity)] // One located span list per owner.
fn story_range_paragraph_spans(
    xml: &[u8],
    owners: &[(StoryId, StoryOwnerSpan)],
) -> Result<Vec<Vec<(ContentLocation, Range<usize>)>>> {
    let owner_scopes =
        story_namespace_scopes_at(xml, owners.iter().map(|(_, owner)| owner.full.start))?;
    let mut inventories = Vec::with_capacity(owners.len());
    for (_, owner) in owners {
        let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
            Error::Other("story owner namespace scope was not inventoried".to_owned())
        })?;
        inventories.push(scan_story_items_with_scope(xml, owner, scope, None)?);
    }
    let control_scopes = story_namespace_scopes_at(
        xml,
        inventories
            .iter()
            .flatten()
            .filter(|item| item.kind == StoryItemKind::ContentControl)
            .map(|item| item.full.start),
    )?;
    story_range_paragraph_spans_from_inventory(xml, owners, &inventories, &control_scopes)
}

#[allow(clippy::type_complexity)] // Story-ordered paragraphs retain checked locations and source spans.
fn story_range_paragraph_spans_from_inventory(
    xml: &[u8],
    owners: &[(StoryId, StoryOwnerSpan)],
    inventories: &[Vec<StoryItemSpan>],
    control_scopes: &HashMap<usize, BTreeMap<String, String>>,
) -> Result<Vec<Vec<(ContentLocation, Range<usize>)>>> {
    let mut spans = Vec::with_capacity(owners.len());
    for ((story, _), items) in owners.iter().zip(inventories) {
        let mut paragraphs = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let item_spans = match item.kind {
                StoryItemKind::Paragraph => vec![(vec![index], item.full.clone())],
                StoryItemKind::ContentControl => {
                    let scope = control_scopes.get(&item.full.start).ok_or_else(|| {
                        Error::Other(
                            "content control namespace scope was not inventoried".to_owned(),
                        )
                    })?;
                    scan_story_control_paragraphs_with_scope(xml, item, scope)?
                        .into_iter()
                        .enumerate()
                        .map(|(paragraph, span)| (vec![index, paragraph], span))
                        .collect()
                }
                _ => continue,
            };
            for (path, span) in item_spans {
                paragraphs.push((
                    ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path),
                    span,
                ));
            }
        }
        spans.push(paragraphs);
    }
    Ok(spans)
}

struct StoryLinkSpan {
    full: Range<usize>,
    rel_id: Option<String>,
    anchor: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StoryNamespace {
    Word,
    Vml,
    Drawing,
    DrawingPicture,
    WordDrawing,
    WordShape,
    WordGroup,
    MarkupCompatibility,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StorySdtContext {
    Block,
    Table,
    Row,
    Inline,
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

fn story_frame_local_name(value: &[u8]) -> Cow<'static, [u8]> {
    match value {
        b"p" => Cow::Borrowed(b"p".as_slice()),
        b"r" => Cow::Borrowed(b"r".as_slice()),
        b"rPr" => Cow::Borrowed(b"rPr".as_slice()),
        b"t" => Cow::Borrowed(b"t".as_slice()),
        _ => Cow::Owned(value.to_vec()),
    }
}

fn xml_decodes(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                for attribute in element.attributes() {
                    let Ok(attribute) = attribute else {
                        return false;
                    };
                    if attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .is_err()
                        || attribute
                            .key
                            .as_ref()
                            .strip_prefix(b"xmlns:")
                            .is_some_and(|prefix| std::str::from_utf8(prefix).is_err())
                    {
                        return false;
                    }
                }
            }
            Ok(Event::Eof) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
        buffer.clear();
    }
}

fn scoped_proof_marker(original: &[u8], restored: &[u8]) -> Result<String> {
    let mut identity = 0usize;
    loop {
        let marker = format!("rdocxScopedPositionProof{identity}");
        if !original
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes())
            && !restored
                .windows(marker.len())
                .any(|bytes| bytes == marker.as_bytes())
        {
            return Ok(marker);
        }
        identity = identity
            .checked_add(1)
            .ok_or_else(|| Error::Other("scoped source proof identity exhausted".into()))?;
    }
}

fn scoped_proof_sentinel(selected_closed: &[u8], marker: &str) -> Result<String> {
    let mut reader = NsReader::from_reader(selected_closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let root = match event {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return Err(Error::Other("scoped source proof has no root".into())),
    };
    if !word_element(&namespace) {
        return Err(Error::Other(
            "scoped source proof needs a qualified Word block".into(),
        ));
    }
    let paragraph =
        format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:r><w:t>{marker}</w:t></w:r></w:p>"#);
    Ok(match root.local_name().as_ref() {
        b"p" => paragraph,
        b"tbl" => format!(
            r#"<w:tbl xmlns:w="{WORD_NAMESPACE}"><w:tr><w:tc>{paragraph}</w:tc></w:tr></w:tbl>"#
        ),
        b"tc" => format!(r#"<w:tc xmlns:w="{WORD_NAMESPACE}">{paragraph}</w:tc>"#),
        b"sdt" => format!(
            r#"<w:sdt xmlns:w="{WORD_NAMESPACE}"><w:sdtPr/><w:sdtContent>{paragraph}</w:sdtContent></w:sdt>"#
        ),
        _ => return Err(Error::Other("unsupported scoped source proof kind".into())),
    })
}

fn canonical_retained_main_part(
    retained: &[u8],
    remap: &HashMap<String, String>,
    modeled_drawing_ids: &[u32],
    nested_image_relationship_ids: &HashSet<String>,
    canonical_drawing_ids: &[(u32, u32)],
) -> Result<Option<(Vec<u8>, CT_Document)>> {
    let mut retained = remap_xml_relationship_ids(retained, remap)?;
    let slots = authored_drawing_slots_in_source_order(
        &retained,
        modeled_drawing_ids,
        nested_image_relationship_ids,
    )?;
    if slots.len() != canonical_drawing_ids.len()
        || slots
            .iter()
            .zip(canonical_drawing_ids)
            .any(|((_, _, old), (expected, _))| old != expected)
    {
        return Ok(None);
    }
    for ((_, span, _), (_, id)) in slots
        .into_iter()
        .rev()
        .zip(canonical_drawing_ids.iter().rev())
    {
        retained.splice(span, id.to_string().into_bytes());
    }
    let document = CT_Document::from_xml(&retained)?;
    Ok(Some((retained, document)))
}

fn word_element(namespace: &ResolveResult<'_>) -> bool {
    matches!(
        namespace,
        ResolveResult::Bound(Namespace(uri)) if *uri == WORD_NAMESPACE.as_bytes()
    )
}

fn story_namespace(namespace: &ResolveResult<'_>) -> StoryNamespace {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => match *uri {
            uri if uri == WORD_NAMESPACE.as_bytes() => StoryNamespace::Word,
            b"urn:schemas-microsoft-com:vml" => StoryNamespace::Vml,
            b"http://schemas.openxmlformats.org/drawingml/2006/main" => StoryNamespace::Drawing,
            b"http://schemas.openxmlformats.org/drawingml/2006/picture" => {
                StoryNamespace::DrawingPicture
            }
            b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" => {
                StoryNamespace::WordDrawing
            }
            b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => {
                StoryNamespace::WordShape
            }
            b"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" => {
                StoryNamespace::WordGroup
            }
            b"http://schemas.openxmlformats.org/markup-compatibility/2006" => {
                StoryNamespace::MarkupCompatibility
            }
            _ => StoryNamespace::Other,
        },
        _ => StoryNamespace::Other,
    }
}

fn modeled_word_child(parent: &[u8], child: &[u8]) -> bool {
    match parent {
        b"document" => child == b"body",
        b"footnotes" => child == b"footnote",
        b"endnotes" => child == b"endnote",
        b"comments" => child == b"comment",
        b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment" | b"txbxContent" => {
            matches!(child, b"p" | b"tbl" | b"sdt" | b"sectPr")
        }
        b"tc" => matches!(child, b"tcPr" | b"p" | b"tbl" | b"sdt"),
        b"tbl" => matches!(child, b"tblPr" | b"tblGrid" | b"tr" | b"sdt"),
        b"tr" => matches!(child, b"trPr" | b"tc" | b"sdt"),
        b"p" => matches!(
            child,
            b"pPr"
                | b"r"
                | b"sdt"
                | b"hyperlink"
                | b"fldSimple"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"bookmarkStart"
                | b"bookmarkEnd"
                | b"commentRangeStart"
                | b"commentRangeEnd"
                | b"proofErr"
        ),
        b"r" => matches!(
            child,
            b"rPr"
                | b"t"
                | b"delText"
                | b"instrText"
                | b"delInstrText"
                | b"fldChar"
                | b"drawing"
                | b"pict"
                | b"object"
                | b"tab"
                | b"br"
                | b"cr"
                | b"sym"
                | b"noBreakHyphen"
                | b"softHyphen"
                | b"footnoteReference"
                | b"endnoteReference"
                | b"commentReference"
                | b"lastRenderedPageBreak"
        ),
        b"sdt" => matches!(child, b"sdtPr" | b"sdtEndPr" | b"sdtContent"),
        b"sdtContent" => false,
        b"hyperlink" => {
            matches!(child, b"r" | b"ins" | b"del" | b"moveFrom" | b"moveTo")
        }
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => matches!(
            child,
            b"r" | b"sdt" | b"hyperlink" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        b"fldSimple" => child == b"r",
        _ => false,
    }
}

fn story_sdt_context(stack: &[XmlElementFrame]) -> Option<StorySdtContext> {
    let parent = stack.last()?;
    if parent.namespace != StoryNamespace::Word {
        return None;
    }
    match parent.local_name.as_ref() {
        b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment" | b"txbxContent"
        | b"tc" => Some(StorySdtContext::Block),
        b"tbl" => Some(StorySdtContext::Table),
        b"tr" => Some(StorySdtContext::Row),
        b"p" | b"hyperlink" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo" => {
            Some(StorySdtContext::Inline)
        }
        b"sdtContent" => parent.sdt_context,
        _ => None,
    }
}

fn story_element_sdt_context(
    stack: &[XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
) -> Option<StorySdtContext> {
    if namespace != StoryNamespace::Word {
        return None;
    }
    match local_name {
        b"sdt" => story_sdt_context(stack),
        b"sdtContent" => stack.last().and_then(|parent| {
            (parent.namespace == StoryNamespace::Word && parent.local_name.as_ref() == b"sdt")
                .then_some(parent.sdt_context)
                .flatten()
        }),
        _ => None,
    }
}

fn modeled_sdt_content_child(context: Option<StorySdtContext>, child: &[u8]) -> bool {
    child == b"sdt"
        || match context {
            Some(StorySdtContext::Block) => matches!(child, b"p" | b"tbl"),
            Some(StorySdtContext::Table) => child == b"tr",
            Some(StorySdtContext::Row) => child == b"tc",
            Some(StorySdtContext::Inline) => {
                matches!(
                    child,
                    b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                )
            }
            None => false,
        }
}

fn modeled_story_child(
    stack: &[XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
) -> bool {
    let Some(parent) = stack.last() else {
        return namespace == StoryNamespace::Word;
    };
    match (parent.namespace, namespace) {
        (StoryNamespace::Word, StoryNamespace::Word)
            if parent.local_name.as_ref() == b"sdtContent" =>
        {
            modeled_sdt_content_child(parent.sdt_context, local_name)
        }
        (StoryNamespace::Word, StoryNamespace::Word) => {
            modeled_word_child(&parent.local_name, local_name)
        }
        (StoryNamespace::Word, StoryNamespace::Vml) => {
            matches!(parent.local_name.as_ref(), b"pict" | b"object")
                && matches!(
                    local_name,
                    b"shape"
                        | b"group"
                        | b"rect"
                        | b"roundrect"
                        | b"oval"
                        | b"line"
                        | b"polyline"
                        | b"curve"
                        | b"arc"
                )
        }
        (StoryNamespace::Vml, StoryNamespace::Vml) => match parent.local_name.as_ref() {
            b"group" => matches!(
                local_name,
                b"shape"
                    | b"group"
                    | b"rect"
                    | b"roundrect"
                    | b"oval"
                    | b"line"
                    | b"polyline"
                    | b"curve"
                    | b"arc"
            ),
            b"shape" | b"rect" | b"roundrect" | b"oval" | b"line" | b"polyline" | b"curve"
            | b"arc" => local_name == b"textbox",
            _ => false,
        },
        (StoryNamespace::Vml, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"textbox" && local_name == b"txbxContent"
        }
        // Word writes a text box twice in a run, as DrawingML in `mc:Choice` and
        // as VML in `mc:Fallback`. One Choice is read, see
        // `modeled_story_event`, and the Fallback stays opaque, so the text box
        // is one story.
        (StoryNamespace::Word, StoryNamespace::MarkupCompatibility) => {
            parent.local_name.as_ref() == b"r" && local_name == b"AlternateContent"
        }
        (StoryNamespace::MarkupCompatibility, StoryNamespace::MarkupCompatibility) => {
            parent.local_name.as_ref() == b"AlternateContent" && local_name == b"Choice"
        }
        (StoryNamespace::MarkupCompatibility, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"Choice" && local_name == b"drawing"
        }
        (StoryNamespace::Word, StoryNamespace::WordDrawing) => {
            parent.local_name.as_ref() == b"drawing" && matches!(local_name, b"inline" | b"anchor")
        }
        (StoryNamespace::WordDrawing, StoryNamespace::Drawing) => {
            matches!(parent.local_name.as_ref(), b"inline" | b"anchor") && local_name == b"graphic"
        }
        (StoryNamespace::Drawing, StoryNamespace::Drawing) => match parent.local_name.as_ref() {
            b"graphic" => local_name == b"graphicData",
            _ => false,
        },
        (StoryNamespace::Drawing, StoryNamespace::WordShape) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"wsp"
        }
        (StoryNamespace::Drawing, StoryNamespace::WordGroup) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"wgp"
        }
        (StoryNamespace::WordGroup, StoryNamespace::WordGroup) => {
            matches!(parent.local_name.as_ref(), b"wgp" | b"grpSp") && local_name == b"grpSp"
        }
        (StoryNamespace::WordGroup, StoryNamespace::WordShape) => {
            matches!(parent.local_name.as_ref(), b"wgp" | b"grpSp") && local_name == b"wsp"
        }
        (StoryNamespace::WordShape, StoryNamespace::WordShape) => {
            parent.local_name.as_ref() == b"wsp" && local_name == b"txbx"
        }
        (StoryNamespace::WordShape, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"txbx" && local_name == b"txbxContent"
        }
        (StoryNamespace::Drawing, StoryNamespace::DrawingPicture) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"pic"
        }
        _ => false,
    }
}

fn field_instruction_has_name(instruction: &str) -> bool {
    story_field_instruction_has_name(instruction)
}

fn story_word_prefixes_at(
    element: &BytesStart<'_>,
    inherited: &Arc<Vec<String>>,
    is_word_element: bool,
) -> Result<Arc<Vec<String>>> {
    let mut prefixes = Arc::clone(inherited);
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace declaration scan failed: {error}"))
        })?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            b"".as_slice()
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            prefix
        } else {
            continue;
        };
        let prefix = std::str::from_utf8(prefix)
            .map_err(|error| Error::Other(format!("story namespace prefix is invalid: {error}")))?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| {
                Error::Other(format!(
                    "story namespace declaration decode failed: {error}"
                ))
            })?
            .into_owned();
        let prefixes = Arc::make_mut(&mut prefixes);
        prefixes.retain(|candidate| {
            candidate != &prefix
                && candidate
                    .strip_prefix('\0')
                    .and_then(|rest| rest.split_once('\0'))
                    .is_none_or(|(bound, _)| bound != prefix)
        });
        prefixes.push(format!("\0{prefix}\0{value}"));
        if value == WORD_NAMESPACE {
            prefixes.push(prefix);
        }
    }
    if is_word_element {
        let name = element.name();
        let prefix = name
            .as_ref()
            .split(|byte| *byte == b':')
            .next()
            .filter(|_| name.as_ref().contains(&b':'))
            .unwrap_or(b"");
        let prefix = std::str::from_utf8(prefix)
            .map_err(|error| Error::Other(format!("story element prefix is invalid: {error}")))?;
        if !prefixes.iter().any(|value| value == prefix) {
            Arc::make_mut(&mut prefixes).push(prefix.to_owned());
        }
    }
    Ok(prefixes)
}

// Locate the selected reference's actual run without changing raw attributes.
fn comment_reference_run_span(xml: &[u8], owner: &Range<usize>, id: i32) -> Result<Range<usize>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
    let mut found = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if word
                    && element.local_name().as_ref() == b"commentReference"
                    && owner.contains(&before)
                {
                    let mut selected = false;
                    for attr in element.attributes() {
                        let attr = attr.map_err(|error| Error::Other(error.to_string()))?;
                        let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                        if word_element(&ns) && local.as_ref() == b"id" {
                            selected = attr
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?
                                .parse::<i32>()
                                .ok()
                                == Some(id);
                        }
                    }
                    if selected {
                        let start = stack
                            .iter()
                            .rev()
                            .find(|(word, name, _)| *word && name == b"r")
                            .map(|(_, _, start)| *start)
                            .ok_or_else(|| {
                                Error::Other("comment reference has no source run".into())
                            })?;
                        found.push(start..story_element_end(xml, start)?);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push((word, element.local_name().as_ref().to_vec(), before));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if found.len() != 1 {
        return Err(Error::Other(
            "comment move requires one qualified reference run".into(),
        ));
    }
    Ok(found.remove(0))
}

#[allow(clippy::type_complexity)] // Concrete edit spans, reference bytes and original run span.
fn comment_move_source_edits(
    xml: &[u8],
    selected: &[Range<usize>],
) -> Result<(Vec<Range<usize>>, Vec<u8>, Range<usize>)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
    let mut reference = None;
    let mut controls = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if selected.iter().any(|span| span.start == before) {
                    for (_, _, start) in stack
                        .iter()
                        .filter(|(word, name, _)| *word && name == b"sdt")
                    {
                        let span = *start..story_element_end(xml, *start)?;
                        if !controls.contains(&span) {
                            controls.push(span);
                        }
                    }
                    if word && element.local_name().as_ref() == b"commentReference" {
                        let start = stack
                            .iter()
                            .rev()
                            .find(|(word, name, _)| *word && name == b"r")
                            .map(|(_, _, start)| *start)
                            .ok_or_else(|| {
                                Error::Other("comment reference has no source run".into())
                            })?;
                        if reference
                            .replace(start..story_element_end(xml, start)?)
                            .is_some()
                        {
                            return Err(Error::Other(
                                "comment move has duplicate reference runs".into(),
                            ));
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push((word, element.local_name().as_ref().to_vec(), before));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let reference_span = reference
        .ok_or_else(|| Error::Other("comment move has no selected reference run".into()))?;
    let scope = story_namespace_scope_at(xml, reference_span.start)?;
    let reference = close_content_fragment_namespaces(&xml[reference_span.clone()], &scope)?;
    // Only the selected direct reference and run properties travel. Other
    // children retain their exact source sequence, even when visible text and
    // opaque producer payload share this run with the reference.
    let selected_reference = selected
        .iter()
        .find(|span| reference_span.contains(&span.start))
        .ok_or_else(|| Error::Other("comment move reference source was not selected".into()))?;
    let mut original_reader = quick_xml::Reader::from_reader(&xml[reference_span.clone()]);
    original_reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let original_open_end = original_reader.buffer_position() as usize;
    let mut reader = NsReader::from_reader(reference.as_slice());
    let mut depth = 0usize;
    let mut reference_found = false;
    let mut open_end = 0usize;
    let mut close_start = 0usize;
    let mut kept = Vec::new();
    let mut mixed = false;
    let mut properties_count = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        let after = reader.buffer_position() as usize;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 0 {
                    open_end = after;
                }
                if depth == 1 {
                    let end = if matches!(event, Event::Empty(_)) {
                        after
                    } else {
                        story_element_end(&reference, before)?
                    };
                    let is_selected = before
                        == selected_reference.start - reference_span.start + open_end
                            - original_open_end;
                    if is_selected && word && element.local_name().as_ref() == b"commentReference" {
                        if reference_found {
                            return Err(Error::Other("comment reference run is ambiguous".into()));
                        }
                        reference_found = true;
                        kept.push(before..end);
                    } else if word && element.local_name().as_ref() == b"rPr" {
                        properties_count += 1;
                        kept.push(before..end);
                    } else {
                        mixed = true;
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_start = before;
                }
            }
            Event::Text(text) if depth == 1 && !text.iter().all(u8::is_ascii_whitespace) => {
                mixed = true;
            }
            Event::Comment(_) | Event::PI(_) if depth == 1 => {
                mixed = true;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !reference_found || properties_count > 1 || close_start == 0 {
        return Err(Error::Other(
            "comment reference is not a uniquely separable direct run child".into(),
        ));
    }
    let mut transport = reference[..open_end].to_vec();
    for span in kept {
        transport.extend_from_slice(&reference[span]);
    }
    transport.extend_from_slice(&reference[close_start..]);
    let mut edits = selected
        .iter()
        .filter(|span| !reference_span.contains(&span.start))
        .cloned()
        .collect::<Vec<_>>();
    edits.push(if mixed {
        selected_reference.clone()
    } else {
        reference_span.clone()
    });
    controls.sort_by_key(|span| std::cmp::Reverse(span.start));
    for control in controls {
        let mut fragment = xml[control.clone()].to_vec();
        let mut contained = edits
            .iter()
            .filter(|span| control.start <= span.start && span.end <= control.end)
            .cloned()
            .collect::<Vec<_>>();
        contained.sort_by_key(|span| span.start);
        for span in contained.iter().rev() {
            fragment.drain(span.start - control.start..span.end - control.start);
        }
        if pure_empty_google_comment_control(
            &fragment,
            &story_namespace_scope_at(xml, control.start)?,
        )? {
            edits.retain(|span| !(control.start <= span.start && span.end <= control.end));
            edits.push(control);
        }
    }
    edits.sort_by_key(|span| span.start);
    if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other("comment move edits overlap".into()));
    }
    Ok((edits, transport, reference_span))
}

fn pure_empty_google_comment_control(xml: &[u8], scope: &BTreeMap<String, String>) -> Result<bool> {
    // An authored namespace declaration or any other root attribute is payload.
    let mut raw_reader = quick_xml::Reader::from_reader(xml);
    if !matches!(raw_reader.read_event(), Ok(Event::Start(ref root)) if root.attributes().next().is_none())
    {
        return Ok(false);
    }
    let closed = close_content_fragment_namespaces(xml, scope)?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut stack = Vec::<Vec<u8>>::new();
    let mut tag = false;
    let mut content = false;
    let mut buffer = Vec::new();
    loop {
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let name = element.local_name().as_ref().to_vec();
                let parent = stack.last().map(Vec::as_slice);
                let allowed = word
                    && matches!(
                        (parent, name.as_slice()),
                        (None, b"sdt")
                            | (Some(b"sdt"), b"sdtPr" | b"sdtContent")
                            | (Some(b"sdtPr"), b"tag")
                            | (Some(b"sdtContent"), b"p")
                    );
                if !allowed {
                    return Ok(false);
                }
                if name == b"sdtContent" {
                    if content {
                        return Ok(false);
                    }
                    content = true;
                }
                for attr in element.attributes() {
                    let attr = attr.map_err(|error| Error::Other(error.to_string()))?;
                    if stack.is_empty()
                        && (attr.key.as_ref() == b"xmlns"
                            || attr.key.as_ref().starts_with(b"xmlns:"))
                    {
                        continue;
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                    if name != b"tag" || !word_element(&ns) || local.as_ref() != b"val" || tag {
                        return Ok(false);
                    }
                    tag = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?
                        .starts_with("goog_rdk_");
                    if !tag {
                        return Ok(false);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(name);
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => break,
            _ => return Ok(false),
        }
        buffer.clear();
    }
    Ok(tag && content)
}

fn story_element_end(xml: &[u8], start: usize) -> Result<usize> {
    let fragment = xml
        .get(start..)
        .ok_or_else(|| Error::Other("story element begins outside its source part".to_owned()))?;
    let mut reader = quick_xml::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story element scan failed: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(start + reader.buffer_position() as usize);
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(start + reader.buffer_position() as usize);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story element has no matching closing element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

// Most elements inherit an unchanged map. Stop at the first declaration, then
// let the original decoder apply the whole element to a full inherited copy.
// Stopping here preserves namespace-value errors before later attribute errors.
fn changed_story_namespace_scope(
    inherited: &BTreeMap<String, String>,
    element: &BytesStart<'_>,
) -> Result<Option<BTreeMap<String, String>>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace scope attribute failed: {error}"))
        })?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            let mut scope = inherited.clone();
            update_story_namespace_scope(&mut scope, element)?;
            return Ok(Some(scope));
        }
    }
    Ok(None)
}

// This is the finite edited CT_P boundary, not a recursive Word schema.
// ECMA-376 fifth edition wml.xsd CT_P/EG_PContent expands to these 35 names.
fn edited_paragraph_schema_child(namespace: &str, local: &str) -> bool {
    (namespace == M_NS && matches!(local, "oMath" | "oMathPara"))
        || (namespace == W_NS
            && matches!(
                local,
                "pPr"
                    | "customXml"
                    | "smartTag"
                    | "sdt"
                    | "dir"
                    | "bdo"
                    | "r"
                    | "fldSimple"
                    | "hyperlink"
                    | "subDoc"
                    | "proofErr"
                    | "permStart"
                    | "permEnd"
                    | "bookmarkStart"
                    | "bookmarkEnd"
                    | "moveFromRangeStart"
                    | "moveFromRangeEnd"
                    | "moveToRangeStart"
                    | "moveToRangeEnd"
                    | "commentRangeStart"
                    | "commentRangeEnd"
                    | "customXmlInsRangeStart"
                    | "customXmlInsRangeEnd"
                    | "customXmlDelRangeStart"
                    | "customXmlDelRangeEnd"
                    | "customXmlMoveFromRangeStart"
                    | "customXmlMoveFromRangeEnd"
                    | "customXmlMoveToRangeStart"
                    | "customXmlMoveToRangeEnd"
                    | "ins"
                    | "del"
                    | "moveFrom"
                    | "moveTo"
            ))
}

#[derive(Clone, Default)]
struct EditedParagraphContext {
    namespaces: BTreeMap<String, String>,
    ignorable: HashSet<String>,
}

#[derive(Default)]
struct EditedParagraphScopeChanges {
    namespaces: Vec<(String, Option<String>)>,
    ignorable: Vec<String>,
}

impl EditedParagraphContext {
    fn expanded_name(&self, name: &[u8], attribute: bool) -> Result<(&str, String)> {
        let name = std::str::from_utf8(name).map_err(|error| Error::Other(error.to_string()))?;
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        let uri = if prefix == "xml" {
            "http://www.w3.org/XML/1998/namespace"
        } else if prefix.is_empty() && attribute {
            ""
        } else {
            match self.namespaces.get(prefix) {
                Some(uri) => uri,
                None if prefix.is_empty() => "",
                None => {
                    return Err(Error::Other(format!(
                        "replace_paragraph_xml has an unbound prefix in {name}"
                    )));
                }
            }
        };
        Ok((uri, local.to_owned()))
    }

    fn enter(&mut self, element: &BytesStart<'_>) -> Result<EditedParagraphScopeChanges> {
        let mut changes = EditedParagraphScopeChanges::default();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let name = attribute.key.as_ref();
            let prefix = if name == b"xmlns" {
                ""
            } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
                std::str::from_utf8(prefix).map_err(|error| Error::Other(error.to_string()))?
            } else {
                continue;
            };
            changes
                .namespaces
                .push((prefix.to_owned(), self.namespaces.remove(prefix)));
        }
        update_story_namespace_scope(&mut self.namespaces, element)?;
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            let (uri, local) = self.expanded_name(attribute.key.as_ref(), true)?;
            if uri == MC_NS && local == "Ignorable" {
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|error| Error::Other(error.to_string()))?;
                for prefix in value.split_ascii_whitespace() {
                    let uri = self
                        .namespaces
                        .get(prefix)
                        .filter(|uri| uri.as_str() != MC_NS)
                        .ok_or_else(|| {
                            Error::Other(format!(
                                "replace_paragraph_xml has invalid mc:Ignorable prefix {prefix}"
                            ))
                        })?;
                    if !self.ignorable.contains(uri) {
                        self.ignorable.insert(uri.clone());
                        changes.ignorable.push(uri.clone());
                    }
                }
            }
        }
        Ok(changes)
    }

    fn leave(&mut self, changes: EditedParagraphScopeChanges) {
        for uri in changes.ignorable {
            self.ignorable.remove(&uri);
        }
        for (prefix, previous) in changes.namespaces.into_iter().rev() {
            self.namespaces.remove(&prefix);
            if let Some(uri) = previous {
                self.namespaces.insert(prefix, uri);
            }
        }
    }
}

// Original and replacement scans share these IDs. A route keeps one parent ID,
// expanded name and sibling ordinal, never a copy of every ancestor route.
#[derive(Default)]
struct EditedParagraphRoutes {
    namespaces: HashMap<String, usize>,
    routes: HashMap<(usize, usize, String, usize), usize>,
}

impl EditedParagraphRoutes {
    fn namespace(&mut self, uri: &str) -> usize {
        let next = self.namespaces.len();
        *self.namespaces.entry(uri.to_owned()).or_insert(next)
    }

    fn child(&mut self, parent: usize, uri: &str, local: &str, ordinal: usize) -> usize {
        let namespace = self.namespace(uri);
        let next = self.routes.len() + 1;
        *self
            .routes
            .entry((parent, namespace, local.to_owned(), ordinal))
            .or_insert(next)
    }
}

fn edited_paragraph_ancestor_context(xml: &[u8], offset: usize) -> Result<EditedParagraphContext> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut ancestors = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) if before == offset => {
                // Do not inherit the original paragraph's own declarations into
                // a replacement that removed or rebound them.
                let _ = element;
                let mut context = EditedParagraphContext::default();
                for ancestor in &ancestors {
                    context.enter(ancestor)?;
                }
                return Ok(context);
            }
            Event::Start(element) => {
                ancestors.push(element.into_owned());
            }
            Event::Empty(_) => {}
            Event::End(_) => {
                ancestors.pop();
            }
            Event::Eof => {
                return Err(Error::Other(
                    "replace_paragraph_xml source ancestry was not found".into(),
                ));
            }
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct EditedParagraphIsland {
    // Expanded ancestor names and ordered element occurrences belong to this
    // selected physical owner. Equal raw bytes elsewhere are no authority.
    route: usize,
    name: (usize, String),
    raw: Vec<u8>,
    meaning: Vec<(usize, String, bool)>,
    authored_ignorable: bool,
}

fn edited_paragraph_complex_mc(
    element: &BytesStart<'_>,
    context: &EditedParagraphContext,
) -> Result<bool> {
    let (uri, _) = context.expanded_name(element.name().as_ref(), false)?;
    if uri == MC_NS {
        return Ok(true);
    }
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (uri, local) = context.expanded_name(attribute.key.as_ref(), true)?;
        if uri == MC_NS && local != "Ignorable" {
            return Ok(true);
        }
    }
    Ok(false)
}

struct EditedParagraphMeaning {
    names: Vec<(usize, String, bool)>,
    complex_mc: bool,
}

fn edited_paragraph_island_meaning(
    xml: &[u8],
    context: &mut EditedParagraphContext,
    routes: &mut EditedParagraphRoutes,
) -> Result<EditedParagraphMeaning> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut scopes = Vec::new();
    let mut meaning = Vec::new();
    let mut complex = false;
    loop {
        let event = reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let changes = context.enter(element)?;
                let (uri, local) = context.expanded_name(element.name().as_ref(), false)?;
                let mc_choice = uri == MC_NS && local == "Choice";
                meaning.push((
                    routes.namespace(uri),
                    local,
                    context.ignorable.contains(uri),
                ));
                complex |= edited_paragraph_complex_mc(element, context)?;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let (uri, local) = context.expanded_name(attribute.key.as_ref(), true)?;
                    meaning.push((
                        routes.namespace(uri),
                        format!("@{local}"),
                        context.ignorable.contains(uri),
                    ));
                    // Opaque QName-valued producer attributes retain any
                    // in-scope prefix meanings they actually mention.
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    for token in value.split_ascii_whitespace() {
                        let prefix = token.split_once(':').map(|(prefix, _)| prefix);
                        if let Some(prefix) = prefix
                            && let Some(uri) = context.namespaces.get(prefix)
                        {
                            meaning.push((routes.namespace(uri), format!("value:{prefix}"), false));
                        }
                        if (uri == MC_NS || (mc_choice && uri.is_empty() && local == "Requires"))
                            && let Some(uri) = context.namespaces.get(token)
                        {
                            meaning.push((routes.namespace(uri), format!("mc:{token}"), false));
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    scopes.push(changes);
                } else {
                    context.leave(changes);
                }
            }
            Event::End(_) => {
                context.leave(scopes.pop().expect("open island element"));
            }
            Event::Eof => {
                return Ok(EditedParagraphMeaning {
                    names: meaning,
                    complex_mc: complex,
                });
            }
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct EditedParagraphMcAttribute {
    route: usize,
    name: String,
    raw_value: Vec<u8>,
    meaning: Vec<(String, Option<usize>, bool)>,
}

#[derive(PartialEq)]
enum EditedParagraphSection {
    Parsed(Box<CT_SectPr>),
    Preserved(EditedParagraphIsland),
}

struct EditedParagraphAdmission {
    islands: Vec<EditedParagraphIsland>,
    mc_attributes: Vec<EditedParagraphMcAttribute>,
    sections: Vec<EditedParagraphSection>,
}

fn edited_paragraph_admission(
    xml: &[u8],
    inherited: &EditedParagraphContext,
    routes: &mut EditedParagraphRoutes,
) -> Result<EditedParagraphAdmission> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut context = inherited.clone();
    let mut frames = vec![(EditedParagraphScopeChanges::default(), 0usize, 0usize)];
    let mut islands = Vec::new();
    let mut mc_attributes = Vec::new();
    let mut ppr = false;
    let mut ppr_route = None;
    let mut sections = Vec::new();
    let mut content = false;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let (_, parent_route, next) = frames.last_mut().expect("initial frame");
                let parent_route = *parent_route;
                let changes = context.enter(element)?;
                let (uri, local) = context.expanded_name(element.name().as_ref(), false)?;
                let route = routes.child(parent_route, uri, &local, *next);
                *next += 1;
                let direct = frames.len() == 2;
                let schema = edited_paragraph_schema_child(uri, &local);
                if direct && schema {
                    if uri == W_NS && local == "pPr" {
                        if ppr || content {
                            return Err(Error::Other(format!(
                                "replace_paragraph_xml requires a single first {{{W_NS}}}pPr child"
                            )));
                        }
                        ppr = true;
                        ppr_route = Some(route);
                    } else {
                        content = true;
                    }
                }
                if Some(parent_route) == ppr_route && uri == W_NS && local == "sectPr" {
                    let end = story_element_end(xml, start)?;
                    let raw = &xml[start..end];
                    // Empty sectPr is preserved raw by CT_PPr. Expand a temporary
                    // copy for the same semantic model used by paired elements.
                    let mut section = raw.to_vec();
                    if matches!(event, Event::Empty(_)) {
                        section.truncate(section.len() - 2);
                        section.extend_from_slice(b"></");
                        section.extend_from_slice(element.name().as_ref());
                        section.push(b'>');
                    }
                    let section = close_content_fragment_namespaces(&section, &context.namespaces)?;
                    let mut paragraph = format!("<w:p xmlns:w=\"{W_NS}\"><w:pPr>").into_bytes();
                    paragraph.extend_from_slice(&section);
                    paragraph.extend_from_slice(b"</w:pPr></w:p>");
                    match CT_P::from_xml_fragment_checked(&paragraph) {
                        Ok(parsed) => {
                            let section = parsed
                                .properties
                                .and_then(|properties| properties.sect_pr)
                                .ok_or_else(|| {
                                    Error::Other(
                                        "paragraph section properties could not be checked".into(),
                                    )
                                })?;
                            sections.push(EditedParagraphSection::Parsed(Box::new(section)));
                        }
                        Err(_) if matches!(event, Event::Empty(_)) => {
                            // The ordinary checked paragraph parser already admitted
                            // this raw Empty section. Preserve its exact bytes and
                            // namespace meaning when paired parsing cannot model it.
                            let mut opaque_context = context.clone();
                            let meaning =
                                edited_paragraph_island_meaning(raw, &mut opaque_context, routes)?;
                            sections.push(EditedParagraphSection::Preserved(
                                EditedParagraphIsland {
                                    route: 0,
                                    name: (routes.namespace(W_NS), "sectPr".into()),
                                    raw: raw.to_vec(),
                                    meaning: meaning.names,
                                    authored_ignorable: false,
                                },
                            ));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                if (!direct
                    && uri == W_NS
                    && matches!(local.as_str(), "drawing" | "pict" | "object"))
                    || (schema && uri == M_NS)
                {
                    // Existing native providers own supported foreign payloads,
                    // math and their established compatibility routes.
                    if matches!(event, Event::Start(_)) {
                        reader
                            .read_to_end(element.name())
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                    }
                    context.leave(changes);
                    continue;
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let (attribute_uri, attribute_local) =
                        context.expanded_name(attribute.key.as_ref(), true)?;
                    if attribute_uri != MC_NS || attribute_local == "Ignorable" {
                        continue;
                    }
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    let meaning = value
                        .split_ascii_whitespace()
                        .map(|token| {
                            let prefix = token.split_once(':').map_or(token, |(prefix, _)| prefix);
                            let uri = if prefix == "xml" {
                                Some("http://www.w3.org/XML/1998/namespace")
                            } else {
                                context.namespaces.get(prefix).map(String::as_str)
                            };
                            let ignored = uri.is_some_and(|uri| context.ignorable.contains(uri));
                            (
                                token.to_owned(),
                                uri.map(|uri| routes.namespace(uri)),
                                ignored,
                            )
                        })
                        .collect();
                    mc_attributes.push(EditedParagraphMcAttribute {
                        route,
                        name: attribute_local,
                        raw_value: attribute.value.as_ref().to_vec(),
                        meaning,
                    });
                }
                if (direct && !schema) || uri == MC_NS {
                    let end = story_element_end(xml, start)?;
                    let raw = &xml[start..end];
                    let ignored_foreign = !uri.is_empty()
                        && uri != W_NS
                        && uri != M_NS
                        && uri != MC_NS
                        && context.ignorable.contains(uri);
                    let name = (routes.namespace(uri), local);
                    let meaning = edited_paragraph_island_meaning(raw, &mut context, routes)?;
                    let authored_ignorable = ignored_foreign && !meaning.complex_mc;
                    islands.push(EditedParagraphIsland {
                        route,
                        name,
                        raw: raw.to_vec(),
                        meaning: meaning.names,
                        authored_ignorable,
                    });
                    if matches!(event, Event::Start(_)) {
                        reader
                            .read_to_end(element.name())
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                    }
                } else if matches!(event, Event::Start(_)) {
                    frames.push((changes, route, 0));
                    continue;
                }
                context.leave(changes);
            }
            Event::End(_) => {
                let (changes, _, _) = frames.pop().expect("open paragraph element");
                context.leave(changes);
            }
            Event::Eof => {
                return Ok(EditedParagraphAdmission {
                    islands,
                    mc_attributes,
                    sections,
                });
            }
            _ => {}
        }
    }
}

fn admit_edited_paragraph(
    original: &[u8],
    incoming: &[u8],
    inherited: &EditedParagraphContext,
) -> Result<()> {
    let mut routes = EditedParagraphRoutes::default();
    let original = edited_paragraph_admission(original, inherited, &mut routes)?;
    let incoming = edited_paragraph_admission(incoming, inherited, &mut routes)?;
    if original.sections != incoming.sections {
        return Err(Error::Other(
            "element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into(),
        ));
    }
    let mut original_counts = HashMap::new();
    let mut incoming_counts = HashMap::new();
    for island in &original.islands {
        *original_counts.entry(island).or_insert(0usize) += 1;
    }
    for island in &incoming.islands {
        *incoming_counts.entry(island).or_insert(0usize) += 1;
    }
    for island in original
        .islands
        .iter()
        .chain(incoming.islands.iter())
        .filter(|island| !island.authored_ignorable)
    {
        if original_counts.get(island) != Some(&1) || incoming_counts.get(island) != Some(&1) {
            let (namespace, local) = &island.name;
            let uri = routes
                .namespaces
                .iter()
                .find_map(|(uri, id)| (id == namespace).then_some(uri))
                .expect("interned island namespace");
            return Err(Error::Other(format!(
                "replace_paragraph_xml refuses new or reinterpreted opaque child {{{uri}}}{local}"
            )));
        }
    }
    let mut original_attributes = HashMap::new();
    let mut incoming_attributes = HashMap::new();
    for attribute in &original.mc_attributes {
        *original_attributes.entry(attribute).or_insert(0usize) += 1;
    }
    for attribute in &incoming.mc_attributes {
        *incoming_attributes.entry(attribute).or_insert(0usize) += 1;
    }
    for attribute in original
        .mc_attributes
        .iter()
        .chain(incoming.mc_attributes.iter())
    {
        if original_attributes.get(attribute) != Some(&1)
            || incoming_attributes.get(attribute) != Some(&1)
        {
            return Err(Error::Other(format!(
                "replace_paragraph_xml refuses changed unsupported {{{MC_NS}}}{} declaration",
                attribute.name
            )));
        }
    }
    Ok(())
}

pub(crate) fn story_namespace_scope_at(
    xml: &[u8],
    offset: usize,
) -> Result<BTreeMap<String, String>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = vec![BTreeMap::<String, String>::new()];
    let mut changed_frames = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story namespace scope scan failed: {error}")))?
        {
            Event::Start(element) => {
                let inherited = scopes.last().expect("initial namespace scope");
                let changed = changed_story_namespace_scope(inherited, &element)?;
                if before == offset {
                    return Ok(changed.unwrap_or_else(|| inherited.clone()));
                }
                changed_frames.push(changed.is_some());
                if let Some(scope) = changed {
                    scopes.push(scope);
                }
            }
            Event::Empty(element) => {
                if before == offset {
                    let inherited = scopes.last().expect("initial namespace scope");
                    return Ok(changed_story_namespace_scope(inherited, &element)?
                        .unwrap_or_else(|| inherited.clone()));
                }
            }
            Event::End(_) => {
                if changed_frames.pop() == Some(true) {
                    scopes.pop();
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story item namespace scope was not found".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

// Pooled local declaration frames and exact target offsets form one lexical proof.
#[allow(clippy::type_complexity)]
pub(crate) fn story_namespace_ancestors_at(
    xml: &[u8],
    offsets: &[usize],
) -> Result<(Vec<Vec<(String, String)>>, HashMap<usize, Vec<usize>>)> {
    let requested = offsets.iter().copied().collect::<HashSet<_>>();
    if requested.len() != offsets.len() {
        return Err(Error::Other(
            "field-cache paragraphs share a canonical start".into(),
        ));
    }
    let mut frames = Vec::new();
    let mut found = HashMap::with_capacity(requested.len());
    let mut active = Vec::<Option<usize>>::new();
    let mut decoded_bytes = 0usize;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("field-cache ancestor scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                // Snapshot BEFORE the target's own declarations. The root is
                // never pooled: the original model rebuilds its writer-time root.
                if requested.contains(&before) {
                    found.insert(before, active.iter().flatten().copied().collect());
                }
                let empty = xml.get(
                    (reader.buffer_position() as usize).saturating_sub(2)
                        ..reader.buffer_position() as usize,
                ) == Some(b"/>");
                if !empty {
                    let mut local = Vec::new();
                    if !active.is_empty() {
                        for attribute in element.attributes() {
                            let attribute = attribute.map_err(|error| {
                                Error::Other(format!(
                                    "story namespace scope attribute failed: {error}"
                                ))
                            })?;
                            if let Some((prefix, namespace)) =
                                decoded_story_namespace_declaration(&element, &attribute)?
                            {
                                decoded_bytes = decoded_bytes
                                    .checked_add(prefix.len())
                                    .and_then(|bytes| bytes.checked_add(namespace.len()))
                                    .filter(|bytes| *bytes <= xml.len())
                                    .ok_or_else(|| {
                                        Error::Other(
                                            "field-cache declaration source work overflow".into(),
                                        )
                                    })?;
                                local.push((prefix, namespace));
                            }
                        }
                    }
                    if local.is_empty() {
                        active.push(None);
                    } else {
                        let index = frames.len();
                        frames.push(local);
                        active.push(Some(index));
                    }
                }
            }
            Event::End(_) => {
                active.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if found.len() != requested.len() || !active.is_empty() {
        return Err(Error::Other(
            "field-cache ancestor inventory is incomplete".into(),
        ));
    }
    Ok((frames, found))
}

fn story_namespace_scopes_at(
    xml: &[u8],
    offsets: impl IntoIterator<Item = usize>,
) -> Result<HashMap<usize, BTreeMap<String, String>>> {
    let requested = offsets.into_iter().collect::<HashSet<_>>();
    let mut found = HashMap::with_capacity(requested.len());
    if requested.is_empty() {
        return Ok(found);
    }
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = vec![BTreeMap::<String, String>::new()];
    let mut changed_frames = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story namespace scope scan failed: {error}")))?
        {
            Event::Start(element) => {
                let inherited = scopes.last().expect("initial namespace scope");
                // Snapshot inherited context and preserve the original early
                // return before this target's declarations are validated.
                if requested.contains(&before) {
                    found.insert(before, inherited.clone());
                    if found.len() == requested.len() {
                        return Ok(found);
                    }
                }
                let changed = changed_story_namespace_scope(inherited, &element)?;
                changed_frames.push(changed.is_some());
                if let Some(scope) = changed {
                    scopes.push(scope);
                }
            }
            Event::Empty(_) => {
                if requested.contains(&before) {
                    found.insert(
                        before,
                        scopes.last().expect("initial namespace scope").clone(),
                    );
                    if found.len() == requested.len() {
                        return Ok(found);
                    }
                }
            }
            Event::End(_) => {
                if changed_frames.pop() == Some(true) {
                    scopes.pop();
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let missing = requested
        .into_iter()
        .filter(|offset| !found.contains_key(offset))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(found)
    } else {
        Err(Error::Other(format!(
            "story namespace scopes were not found at offsets {missing:?}"
        )))
    }
}

fn scoped_story_fragment(
    xml: &[u8],
    range: Range<usize>,
    scope: &BTreeMap<String, String>,
) -> Result<(Vec<u8>, usize)> {
    let original = xml
        .get(range)
        .ok_or_else(|| Error::Other("story fragment lies outside its source part".to_owned()))?;
    let closed = close_content_fragment_namespaces(original, scope)?;
    let added = closed
        .len()
        .checked_sub(original.len())
        .ok_or_else(|| Error::Other("namespace closure shortened a story fragment".to_owned()))?;
    Ok((closed, added))
}

fn scan_story_items_with_scope(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    scope: &BTreeMap<String, String>,
    template_field_ranges: Option<&mut Vec<Range<usize>>>,
) -> Result<Vec<StoryItemSpan>> {
    let (closed, added) = scoped_story_fragment(xml, owner.full.clone(), scope)?;
    let mut local_owner = owner.clone();
    local_owner.full = 0..closed.len();
    let mut local_fields = Vec::new();
    let mut items = scan_story_items(
        &closed,
        &local_owner,
        template_field_ranges.as_ref().map(|_| &mut local_fields),
    )?;
    let map = |position: usize| {
        if position == 0 {
            owner.full.start
        } else {
            owner.full.start + position - added
        }
    };
    if let Some(ranges) = template_field_ranges {
        ranges.extend(
            local_fields
                .into_iter()
                .map(|range| map(range.start)..map(range.end)),
        );
    }
    for item in &mut items {
        item.full = map(item.full.start)..map(item.full.end);
        item.scan = map(item.scan.start)..map(item.scan.end);
        for ancestor in &mut item.complex_ancestors {
            *ancestor = map(*ancestor);
        }
    }
    Ok(items)
}

fn scoped_story_item(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<(Vec<u8>, StoryItemSpan, usize)> {
    let (closed, added, scan) = if item.complex_field {
        // A complex field spans sibling runs. Bind their shared context on an
        // inert envelope, leaving every source byte and local declaration intact.
        let original = xml.get(item.scan.clone()).ok_or_else(|| {
            Error::Other("complex story field lies outside its source part".to_owned())
        })?;
        let mut closed = b"<rdocx_story_excerpt".to_vec();
        for (prefix, namespace) in scope {
            closed.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                closed.push(b':');
                closed.extend_from_slice(prefix.as_bytes());
            }
            closed.extend_from_slice(b"=\"");
            closed.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
            closed.push(b'"');
        }
        closed.push(b'>');
        let added = closed.len();
        closed.extend_from_slice(original);
        let end = closed.len();
        closed.extend_from_slice(b"</rdocx_story_excerpt>");
        (closed, added, added..end)
    } else {
        let (closed, added) = scoped_story_fragment(xml, item.scan.clone(), scope)?;
        let end = closed.len();
        (closed, added, 0..end)
    };
    let source_len = item
        .scan
        .end
        .checked_sub(item.scan.start)
        .ok_or_else(|| Error::Other("story item has an inverted scan span".to_owned()))?;
    let local = |position: usize| -> Result<usize> {
        if !item.complex_field && position == item.scan.start {
            return Ok(0);
        }
        position
            .checked_sub(item.scan.start)
            .filter(|relative| *relative <= source_len)
            .and_then(|relative| relative.checked_add(added))
            .ok_or_else(|| Error::Other("story item lies outside its scan span".to_owned()))
    };
    let full = local(item.full.start)?..local(item.full.end)?;
    if full.start > full.end {
        return Err(Error::Other(
            "story item has an inverted source span".to_owned(),
        ));
    }
    let local = StoryItemSpan {
        kind: item.kind,
        full,
        scan,
        direct_owner_child: item.direct_owner_child,
        complex_field: item.complex_field,
        // Ancestors gate typed discovery in scan_story_items. Neither excerpt
        // scanner consumes them, and an external ancestor has no local offset.
        complex_ancestors: Vec::new(),
        sdt_context: item.sdt_context,
    };
    Ok((closed, local, added))
}

fn story_item_text_with_scope(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
    qualified_items: &[StoryItemSpan],
) -> Result<Option<String>> {
    let (closed, local, added) = scoped_story_item(xml, item, scope)?;
    story_item_text(&closed, &local, qualified_items, item.scan.clone(), added)
}

fn scan_story_item_links_with_scope(
    xml: &[u8],
    item: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<Vec<StoryLinkSpan>> {
    let (closed, local, added) = scoped_story_item(xml, item, scope)?;
    let links = scan_story_item_links(&closed, &local)?;
    let map = |position: usize| -> Result<usize> {
        if !item.complex_field && position == 0 {
            return Ok(item.scan.start);
        }
        position
            .checked_sub(added)
            .and_then(|relative| item.scan.start.checked_add(relative))
            .filter(|source| item.scan.start <= *source && *source <= item.scan.end)
            .ok_or_else(|| Error::Other("story hyperlink lies outside its source span".to_owned()))
    };
    links
        .into_iter()
        .map(|link| {
            let full = map(link.full.start)?..map(link.full.end)?;
            if full.start > full.end {
                return Err(Error::Other(
                    "story hyperlink has an inverted source span".to_owned(),
                ));
            }
            Ok(StoryLinkSpan {
                full,
                rel_id: link.rel_id,
                anchor: link.anchor,
            })
        })
        .collect()
}

/// Scan one story's hyperlinks in physical source order, each paired with the
/// smallest item that owns it. [`Document::story_links`] numbers this list.
fn story_link_spans(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    story: &StoryId,
) -> Result<Vec<(ContentLocation, StoryLinkSpan)>> {
    let items = scan_story_items(xml, owner, None)?;
    let item_scopes = story_namespace_scopes_at(xml, items.iter().map(|item| item.scan.start))?;
    ordered_story_link_spans(xml, story, items, &item_scopes)
}

/// Pair the hyperlinks of one story's scanned items with the smallest item
/// that owns each, in physical source order.
fn ordered_story_link_spans(
    xml: &[u8],
    story: &StoryId,
    items: Vec<StoryItemSpan>,
    item_scopes: &HashMap<usize, BTreeMap<String, String>>,
) -> Result<Vec<(ContentLocation, StoryLinkSpan)>> {
    let mut links = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let scope = item_scopes.get(&item.scan.start).ok_or_else(|| {
            Error::Other("story item namespace scope was not inventoried".to_owned())
        })?;
        let owner_width = item.full.end - item.full.start;
        for link in scan_story_item_links_with_scope(xml, item, scope)? {
            let location = ContentLocation {
                story: story.clone(),
                item_kind: item.kind,
                index_path: vec![index],
                is_end: false,
            };
            links.push((owner_width, location, link));
        }
    }
    links.sort_by_key(|(owner_width, _, link)| (link.full.start, *owner_width));
    links.dedup_by(|left, right| left.2.full == right.2.full);
    Ok(links
        .into_iter()
        .map(|(_, location, link)| (location, link))
        .collect())
}

fn content_fragment_root_is_section_properties(xml: &[u8], item: &StoryItemSpan) -> Result<bool> {
    let mut scope = story_namespace_scope_at(xml, item.full.start)?;
    let mut reader = quick_xml::Reader::from_reader(&xml[item.full.clone()]);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("content fragment scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                update_story_namespace_scope(&mut scope, &element)?;
                let name = element.name();
                let bytes = name.as_ref();
                let (prefix, local) =
                    if let Some(colon) = bytes.iter().position(|byte| *byte == b':') {
                        (
                            std::str::from_utf8(&bytes[..colon]).map_err(|error| {
                                Error::Other(format!(
                                    "content fragment namespace prefix is invalid: {error}"
                                ))
                            })?,
                            &bytes[colon + 1..],
                        )
                    } else {
                        ("", bytes)
                    };
                return Ok(local == b"sectPr"
                    && scope
                        .get(prefix)
                        .is_some_and(|value| value == WORD_NAMESPACE));
            }
            Event::Eof => {
                return Err(Error::Other(
                    "content fragment has no root element".to_owned(),
                ));
            }
            _ => buffer.clear(),
        }
    }
}

fn direct_story_content_items(xml: &[u8], owner: &StoryOwnerSpan) -> Result<Vec<StoryItemSpan>> {
    direct_story_content_items_in(xml, owner, &scan_story_items(xml, owner, None)?)
}

fn direct_story_content_items_in(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    story_items: &[StoryItemSpan],
) -> Result<Vec<StoryItemSpan>> {
    let mut items = Vec::new();
    for item in story_items {
        if !item.direct_owner_child {
            continue;
        }
        if owner.kind == StoryKind::Body
            && item.kind == StoryItemKind::PreservedNode
            && content_fragment_root_is_section_properties(xml, item)?
        {
            continue;
        }
        items.push(item.clone());
    }
    Ok(items)
}

pub(crate) fn validate_fragment_block_content(content: &[u8]) -> Result<()> {
    let mut xml = format!(
        r#"<w:document xmlns:w="{}"><w:body>"#,
        oxoffice_docx_oxml::namespace::W_NS
    )
    .into_bytes();
    xml.extend_from_slice(content);
    xml.extend_from_slice(b"</w:body></w:document>");
    let owner = scan_story_owners(&xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("fragment content has no body owner".to_owned()))?;
    for item in direct_story_content_items(&xml, &owner)? {
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let item_xml = close_content_fragment_namespaces(&xml[item.full], &scope)?;
        validate_fragment_block_item(&item_xml)?;
    }
    Ok(())
}

fn validate_fragment_block_item(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    if let (namespace, Event::Start(element) | Event::Empty(element)) = reader
        .read_resolved_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid document fragment item: {error}")))?
        && word_element(&namespace)
    {
        match element.local_name().as_ref() {
            b"sdt" => validate_serialized_block_content_control(xml)?,
            b"r" | b"hyperlink" | b"fldSimple" | b"drawing" | b"pict" => {
                return Err(Error::Other(
                    "document fragment boundaries admit block content only".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn story_owner_content_end(xml: &[u8], owner: &StoryOwnerSpan) -> Result<usize> {
    let items = if owner.kind == StoryKind::Body {
        scan_story_items(xml, owner, None)?
    } else {
        Vec::new()
    };
    story_owner_content_end_in(xml, owner, &items)
}

fn story_owner_content_end_in(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    items: &[StoryItemSpan],
) -> Result<usize> {
    if owner.kind == StoryKind::Body {
        for item in items {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, item)?
            {
                return Ok(item.full.start);
            }
        }
    }
    let mut reader = quick_xml::Reader::from_reader(&xml[owner.full.clone()]);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story owner boundary scan failed: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return (owner.full.end >= owner.full.start + 2)
                    .then_some(owner.full.end - 2)
                    .ok_or_else(|| Error::Other("empty story owner is incomplete".to_owned()));
            }
            Event::End(_) if depth == 1 => return Ok(owner.full.start + before),
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => {
                return Err(Error::Other(
                    "story owner has no closing boundary".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn package_authoritative_body_fragment(
    xml: &[u8],
    include_final_section_properties: bool,
    destination_scope: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let owner = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("document fragment has no main body".to_owned()))?;
    let mut output = Vec::new();
    let items = direct_story_content_items(xml, &owner)?;
    let mut cursor = items
        .first()
        .map(|item| item.full.start)
        .ok_or_else(|| Error::Other("document fragment main body is empty".to_owned()))?;
    for item in items {
        output.extend_from_slice(&xml[cursor..item.full.start]);
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        output.extend(close_content_fragment_namespaces(
            &xml[item.full.clone()],
            &required,
        )?);
        cursor = item.full.end;
    }
    output.extend_from_slice(&xml[cursor..story_owner_content_end(xml, &owner)?]);
    if include_final_section_properties {
        let mut section = None;
        for item in scan_story_items(xml, &owner, None)? {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, &item)?
            {
                section = Some(item);
                break;
            }
        }
        let section = section.ok_or_else(|| {
            Error::Other("section-inclusive document fragment has no final section".to_owned())
        })?;
        let scope = story_namespace_scope_at(xml, section.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let section = close_content_fragment_namespaces(&xml[section.full], &required)?;
        output.extend_from_slice(format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:pPr>"#).as_bytes());
        output.extend(section);
        output.extend_from_slice(b"</w:pPr></w:p>");
    }
    Ok(output)
}

pub(crate) fn package_authoritative_body_source(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: &oxoffice_docx_oxml::document::ScopedWordSource,
    include_final_section_properties: bool,
    destination_scope: &BTreeMap<String, String>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    // The same package-authoritative selectors and namespace policy as the
    // ordinary byte producer above, retaining their actual selected ranges.
    let xml = source.as_bytes();
    let owner = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("document fragment has no main body".into()))?;
    let items = direct_story_content_items(xml, &owner)?;
    let mut cursor = items
        .first()
        .map(|item| item.full.start)
        .ok_or_else(|| Error::Other("document fragment main body is empty".into()))?;
    let mut output = Vec::new();
    for item in items {
        output.push(operation.compose_sources(&[(Some(source), cursor..item.full.start, &[])])?);
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let selected = operation.compose_sources(&[(Some(source), item.full.clone(), &[])])?;
        output.push(operation.close_source_namespaces(selected, &required)?);
        cursor = item.full.end;
    }
    output.push(operation.compose_sources(&[(
        Some(source),
        cursor..story_owner_content_end(xml, &owner)?,
        &[],
    )])?);
    if include_final_section_properties {
        let mut section = None;
        for item in scan_story_items(xml, &owner, None)? {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, &item)?
            {
                section = Some(item);
                break;
            }
        }
        let section = section.ok_or_else(|| {
            Error::Other("section-inclusive document fragment has no final section".into())
        })?;
        let scope = story_namespace_scope_at(xml, section.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let selected = operation.compose_sources(&[(Some(source), section.full, &[])])?;
        let selected = operation.close_source_namespaces(selected, &required)?;
        let open = format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:pPr>"#);
        output.push(operation.compose_sources(&[
            (None, 0..0, open.as_bytes()),
            (Some(&selected), 0..selected.as_bytes().len(), &[]),
            (None, 0..0, b"</w:pPr></w:p>"),
        ])?);
    }
    let pieces = output
        .iter()
        .map(|part| (Some(part), 0..part.as_bytes().len(), &[][..]))
        .collect::<Vec<_>>();
    Ok(operation.compose_sources(&pieces)?)
}

/// Whether a fragment uses the `w14` prefix: in a qualified name, as the
/// part root declaration finds it, or in a markup compatibility attribute
/// that lists prefixes or qualified names.
fn fragment_uses_w14(xml: &[u8]) -> Result<bool> {
    let named = xml.windows(5).any(|window| {
        matches!(window[0], b'<' | b'/' | b' ' | b'\t' | b'\r' | b'\n') && &window[1..] == b"w14:"
    });
    if named {
        return Ok(true);
    }
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("fragment scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("fragment attribute scan failed: {error}"))
                    })?;
                    let compatibility = matches!(
                        attribute.key.local_name().as_ref(),
                        b"Requires"
                            | b"Ignorable"
                            | b"ProcessContent"
                            | b"MustUnderstand"
                            | b"PreserveElements"
                            | b"PreserveAttributes"
                    );
                    if compatibility
                        && attribute
                            .value
                            .split(|byte| byte.is_ascii_whitespace())
                            .any(|token| token == b"w14" || token.starts_with(b"w14:"))
                    {
                        return Ok(true);
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

/// The body item span of a part written by
/// `CT_Document::standalone_body_item_xml`.
fn standalone_body_item_span(part: &[u8]) -> Result<(usize, usize)> {
    // Attribute values escape `<`, so the first body tag follows the root.
    let start = part
        .windows(b"<w:body>".len())
        .position(|window| window == b"<w:body>")
        .map(|body| body + b"<w:body>".len())
        .ok_or_else(|| Error::Other("standalone body item has no body".into()))?;
    let end = part
        .len()
        .checked_sub(b"</w:body></w:document>".len())
        .filter(|end| start <= *end)
        .ok_or_else(|| Error::Other("standalone body item is incomplete".into()))?;
    Ok((start, end))
}

/// The physical cell owner of `(row, cell)` in the table `item` of `xml`.
fn physical_table_cell_owner(
    xml: &[u8],
    root_kind: StoryKind,
    item: &StoryItemSpan,
    row: usize,
    cell: usize,
) -> Result<StoryOwnerSpan> {
    let table = Document::story_table_from_source(xml, item)?;
    TableRef { inner: &table }
        .cell(row, cell)
        .ok_or_else(|| Error::Other("rich table cell index is out of range".to_owned()))?;
    let mut owners = scan_story_owners(xml, root_kind)?
        .into_iter()
        .filter(|owner| {
            owner.kind == StoryKind::TableCell
                && item.full.start < owner.full.start
                && owner.full.end < item.full.end
        })
        .collect::<Vec<_>>();
    let (_, modeled) = physical_table_cells(&table);
    if modeled.len() < owners.len() {
        return Err(Error::Other(
            "rich table cell source correspondence is incomplete".into(),
        ));
    }
    if modeled.len() > owners.len() {
        return Err(Error::Other(
            "rich table has unqualified modeled cells".into(),
        ));
    }
    let ordinal = modeled
        .iter()
        .position(|(table, coordinate)| *table == 0 && *coordinate == Some((row, cell)))
        .ok_or_else(|| Error::Other("rich table cell has no unique physical owner".into()))?;
    Ok(owners.swap_remove(ordinal))
}

pub(crate) fn close_content_fragment_namespaces(
    xml: &[u8],
    scope: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let closed = oxoffice_docx_oxml::document::close_content_fragment_namespaces(xml, scope)
        .map_err(|error| match error {
            oxoffice_docx_oxml::OxmlError::InvalidValue(message) => Error::Other(message),
            other => Error::from(other),
        })?;
    Ok(closed)
}

fn reject_section_owning_content_fragment(fragment: &ContentFragment) -> Result<()> {
    let xml = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
    let mut wrapped = format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
    wrapped.extend_from_slice(&xml);
    wrapped.extend_from_slice(b"</w:body></w:document>");
    let document = CT_Document::from_xml(&wrapped)?;
    let mut owns_section = false;
    visit_body_paragraphs(&document.body.content, false, &mut |paragraph| {
        owns_section |= paragraph
            .properties
            .as_ref()
            .is_some_and(|properties| properties.sect_pr.is_some());
    });
    if owns_section {
        return Err(Error::Other(
            "generic content operations cannot mutate a section-owning paragraph".to_owned(),
        ));
    }
    Ok(())
}

fn serialize_content_fragment(content: BodyContent) -> Result<Vec<u8>> {
    let mut document = CT_Document::new();
    document.body.content = vec![content];
    document.body.sect_pr = None;
    let xml = document.to_xml()?;
    let owner = scan_story_owners(&xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("serialized content fragment has no body owner".to_owned()))?;
    let items = direct_story_content_items(&xml, &owner)?;
    let [item] = items.as_slice() else {
        return Err(Error::Other(
            "serialized content fragment must contain exactly one direct item".to_owned(),
        ));
    };
    let scope = story_namespace_scope_at(&xml, item.full.start)?;
    close_content_fragment_namespaces(&xml[item.full.clone()], &scope)
}

const BLOCK_SDT_PROPERTIES_SLOT: u8 = 1;
const BLOCK_SDT_END_PROPERTIES_SLOT: u8 = 2;
const BLOCK_SDT_CONTENT_SLOT: u8 = 4;

fn validate_serialized_block_content_control(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut controls: Vec<(usize, Option<usize>, u8, u8)> = Vec::new();

    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(element) => {
                validate_block_control_element(
                    &namespace,
                    &element,
                    depth,
                    false,
                    &mut root_seen,
                    &mut controls,
                )?;
                depth += 1;
            }
            Event::Empty(element) => validate_block_control_element(
                &namespace,
                &element,
                depth,
                true,
                &mut root_seen,
                &mut controls,
            )?,
            Event::End(element) => {
                if depth == 0 {
                    return Err(Error::Other(
                        "block content control has an unmatched end element".to_owned(),
                    ));
                }
                if controls.last().is_some_and(|control| {
                    control.0 == depth
                        && word_element(&namespace)
                        && matches_local_name(element.name().as_ref(), b"sdt")
                }) {
                    controls.pop();
                }
                depth -= 1;
            }
            Event::Text(text) => {
                let bytes: &[u8] = text.as_ref();
                if block_control_text_is_direct(depth, &controls)
                    && !block_control_bytes_are_xml_whitespace(bytes)?
                {
                    return Err(Error::Other(
                        "block content control has direct non-whitespace text".to_owned(),
                    ));
                }
            }
            Event::CData(text) => {
                let bytes: &[u8] = text.as_ref();
                if block_control_text_is_direct(depth, &controls)
                    && !block_control_bytes_are_xml_whitespace(bytes)?
                {
                    return Err(Error::Other(
                        "block content control has direct non-whitespace CDATA".to_owned(),
                    ));
                }
            }
            Event::GeneralRef(reference) if block_control_text_is_direct(depth, &controls) => {
                if !block_control_reference_is_whitespace(&reference)? {
                    return Err(Error::Other(
                        "block content control has a direct non-whitespace reference".to_owned(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    if !root_seen || depth != 0 || !controls.is_empty() {
        return Err(Error::Other(
            "block content control is structurally incomplete".to_owned(),
        ));
    }
    Ok(())
}

fn block_control_reference_is_whitespace(
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<bool> {
    if let Some(character) = reference.resolve_char_ref().map_err(|error| {
        Error::Other(format!(
            "block content control has an invalid character reference: {error}"
        ))
    })? {
        if !xml_1_0_character_is_valid(character) {
            return Err(Error::Other(
                "block content control has an XML 1.0-forbidden character reference".to_owned(),
            ));
        }
        return Ok(xml_1_0_whitespace(character));
    }
    let name = reference.decode().map_err(|error| {
        Error::Other(format!(
            "block content control has an invalid entity reference: {error}"
        ))
    })?;
    let value = resolve_xml_entity(&name).ok_or_else(|| {
        Error::Other(format!(
            "block content control has an unresolved entity reference: &{name};"
        ))
    })?;
    if !value.chars().all(xml_1_0_character_is_valid) {
        return Err(Error::Other(format!(
            "block content control entity reference contains an XML 1.0-forbidden character: &{name};"
        )));
    }
    Ok(value.chars().all(xml_1_0_whitespace))
}

fn block_control_bytes_are_xml_whitespace(bytes: &[u8]) -> Result<bool> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        Error::Other(format!(
            "block content control has invalid UTF-8 text: {error}"
        ))
    })?;
    Ok(text.chars().all(xml_1_0_whitespace))
}

fn xml_1_0_whitespace(character: char) -> bool {
    matches!(character, '\u{20}' | '\u{9}' | '\u{A}' | '\u{D}')
}

fn xml_1_0_character_is_valid(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r')
        || ('\u{20}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

fn validate_block_control_element(
    namespace: &ResolveResult<'_>,
    element: &BytesStart<'_>,
    parent_depth: usize,
    empty: bool,
    root_seen: &mut bool,
    controls: &mut Vec<(usize, Option<usize>, u8, u8)>,
) -> Result<()> {
    let is_word = word_element(namespace);
    let name = element.name();
    let local = name.as_ref();

    if controls.is_empty() {
        if *root_seen || !is_word || !matches_local_name(local, b"sdt") {
            return Err(Error::Other(
                "block content control must contain exactly one Word sdt root".to_owned(),
            ));
        }
        *root_seen = true;
        if !empty {
            controls.push((parent_depth + 1, None, 0, 0));
        }
        return Ok(());
    }

    let (root_depth, content_depth) = {
        let control = controls
            .last()
            .expect("active block content control was checked above");
        (control.0, control.1)
    };
    if parent_depth == root_depth {
        if !is_word {
            return Ok(());
        }
        let (slot, order) = if matches_local_name(local, b"sdtPr") {
            (BLOCK_SDT_PROPERTIES_SLOT, 0)
        } else if matches_local_name(local, b"sdtEndPr") {
            (BLOCK_SDT_END_PROPERTIES_SLOT, 1)
        } else if matches_local_name(local, b"sdtContent") {
            (BLOCK_SDT_CONTENT_SLOT, 2)
        } else {
            return Err(Error::Other(
                "block content control has an invalid Word root child".to_owned(),
            ));
        };
        let control = controls
            .last_mut()
            .expect("active block content control was checked above");
        if control.3 & slot != 0 || control.2 > order {
            return Err(Error::Other(
                "block content control root children are duplicated or out of order".to_owned(),
            ));
        }
        control.2 = order;
        control.3 |= slot;
        if slot == BLOCK_SDT_CONTENT_SLOT && !empty {
            control.1 = Some(parent_depth + 1);
        }
        return Ok(());
    }

    if content_depth != Some(parent_depth) {
        return Ok(());
    }
    if !is_word {
        return Ok(());
    }
    if matches_local_name(local, b"p") || matches_local_name(local, b"tbl") {
        return Ok(());
    }
    if matches_local_name(local, b"sdt") {
        if !empty {
            controls.push((parent_depth + 1, None, 0, 0));
        }
        return Ok(());
    }
    Err(Error::Other(
        "block content control has a non-block Word content child".to_owned(),
    ))
}

fn block_control_text_is_direct(depth: usize, controls: &[(usize, Option<usize>, u8, u8)]) -> bool {
    controls
        .last()
        .is_some_and(|control| depth == control.0 || Some(depth) == control.1)
}

fn validated_direct_content_item(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<StoryItemSpan> {
    if location.index_path.len() != 1 {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let items = scan_story_items(xml, owner, None)?;
    let index = location.index_path[0];
    let item = items.get(index).cloned().ok_or(StoryError::OutOfBounds {
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
    if !item.direct_owner_child
        || owner.kind == StoryKind::Body && content_fragment_root_is_section_properties(xml, &item)?
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    Ok(item)
}

// Nested paragraph paths use the enclosing block control's content as the
// effective block owner while keeping the underlying part's relationship scope.
fn fragment_content_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(StoryOwnerSpan, usize, usize, usize)> {
    if location.is_end || location.index_path.len() == 1 {
        let (boundary, index, count) = validated_content_boundary(xml, owner, location)?;
        return Ok((owner.clone(), boundary, index, count));
    }
    let [control_index, paragraph_index] = location.index_path.as_slice() else {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    };
    if location.item_kind != StoryItemKind::Paragraph {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let items = scan_story_items(xml, owner, None)?;
    let control = items.get(*control_index).ok_or(StoryError::OutOfBounds {
        index: *control_index,
        len: items.len(),
    })?;
    if control.kind != StoryItemKind::ContentControl
        || control.sdt_context != Some(StorySdtContext::Block)
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let paragraphs = scan_story_control_paragraphs(xml, control)?;
    let paragraph = paragraphs
        .get(*paragraph_index)
        .ok_or(StoryError::OutOfBounds {
            index: *paragraph_index,
            len: paragraphs.len(),
        })?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut enclosing = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("fragment control owner scan failed: {error}")))?
        {
            Event::Start(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                stack.push((
                    word_element(&namespace) && local.as_ref() == b"sdtContent",
                    before,
                ));
            }
            Event::End(_) => {
                if let Some((true, start)) = stack.pop() {
                    let end = reader.buffer_position() as usize;
                    if start >= control.full.start
                        && end <= control.full.end
                        && start < paragraph.start
                        && end > paragraph.end
                    {
                        enclosing.push(start..end);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let full = enclosing
        .into_iter()
        .min_by_key(|span| span.end - span.start)
        .ok_or_else(|| Error::Other("fragment paragraph has no block control owner".to_owned()))?;
    let effective = StoryOwnerSpan {
        full,
        ..owner.clone()
    };
    let direct = direct_story_content_items(xml, &effective)?;
    let index = direct
        .iter()
        .position(|item| item.kind == StoryItemKind::Paragraph && item.full == *paragraph)
        .ok_or_else(|| {
            Error::Other("fragment paragraph is not a direct block control child".to_owned())
        })?;
    Ok((effective, paragraph.start, index, direct.len()))
}

fn validated_content_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(usize, usize, usize)> {
    let items = direct_story_content_items(xml, owner)?;
    if location.is_end {
        return Ok((
            story_owner_content_end(xml, owner)?,
            items.len(),
            items.len(),
        ));
    }
    if location.index_path.len() != 1 {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let projected = scan_story_items(xml, owner, None)?;
    let index = location.index_path[0];
    let item = projected.get(index).ok_or(StoryError::OutOfBounds {
        index,
        len: projected.len(),
    })?;
    if item.kind != location.item_kind {
        return Err(StoryError::KindMismatch {
            expected: location.item_kind,
            actual: item.kind,
        }
        .into());
    }
    if !item.direct_owner_child
        || owner.kind == StoryKind::Body && content_fragment_root_is_section_properties(xml, item)?
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let direct_index = items
        .iter()
        .position(|candidate| candidate.full == item.full)
        .ok_or_else(|| Error::Other("direct content destination was not found".to_owned()))?;
    Ok((item.full.start, direct_index, items.len()))
}

fn insert_story_fragment(
    source: &mut Vec<u8>,
    owner: &StoryOwnerSpan,
    boundary: usize,
    fragment: Vec<u8>,
) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(&source[owner.full.clone()]);
    let mut buffer = Vec::new();
    let empty_name = loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story owner insertion scan failed: {error}")))?
        {
            Event::Start(_) => break None,
            Event::Empty(element) => break Some(element.name().as_ref().to_vec()),
            Event::Eof => {
                return Err(Error::Other(
                    "story owner has no insertion boundary".to_owned(),
                ));
            }
            _ => buffer.clear(),
        }
    };
    if let Some(name) = empty_name {
        if boundary + 2 != owner.full.end {
            return Err(Error::Other(
                "empty story owner insertion boundary is invalid".to_owned(),
            ));
        }
        let mut replacement = Vec::with_capacity(fragment.len() + name.len() + 4);
        replacement.push(b'>');
        replacement.extend(fragment);
        replacement.extend_from_slice(b"</");
        replacement.extend(name);
        replacement.push(b'>');
        source.splice(boundary..owner.full.end, replacement);
    } else {
        source.splice(boundary..boundary, fragment);
    }
    Ok(())
}

fn story_scope_matches_fragment(destination: &StoryId, fragment: &ContentFragment) -> bool {
    fragment.source_part_name.as_deref() == Some(destination.part_name.as_str())
        && fragment.source_story_kind == Some(destination.kind)
        && fragment.source_owner_index == Some(destination.owner_index)
}

/// Concrete typed owners used only while qualifying replacement XML.
/// Raw preservation payloads never acquire relationship type authority.
enum WordXmlOwner<'a> {
    Paragraph(&'a CT_P),
    Run(&'a CT_R),
    Table(&'a CT_Tbl),
    Row(&'a CT_Row),
    Cell(&'a CT_Tc),
    Control(&'a CT_Sdt),
}

/// Qualify changed bookmark and comment identities through existing physical
/// source inventories. Unchanged selected identities retain their owner proof.
fn validate_element_xml_identity_change(
    document: &Document,
    story: &StoryId,
    previous: &[u8],
    incoming: &[u8],
) -> Result<()> {
    let old_comments = crate::comments::comment_source_markers(previous)?;
    let new_comments = crate::comments::comment_source_markers(incoming)?;
    if !old_comments
        .iter()
        .map(|marker| (marker.id, marker.family))
        .eq(new_comments.iter().map(|marker| (marker.id, marker.family)))
    {
        let counts = |markers: &[crate::comments::CommentSourceMarker]| -> Result<BTreeMap<i32, [usize; 3]>> {
            let mut counts = BTreeMap::new();
            for marker in markers {
                let entry = counts.entry(marker.id).or_insert([0usize; 3]);
                entry[marker.family] = entry[marker.family].checked_add(1).ok_or_else(|| Error::Other("element XML comment count overflow".into()))?;
            }
            Ok(counts)
        };
        let before = counts(&old_comments)?;
        let after = counts(&new_comments)?;
        let mut old_order = BTreeMap::<i32, Vec<usize>>::new();
        let mut new_order = BTreeMap::<i32, Vec<usize>>::new();
        for marker in &old_comments {
            old_order.entry(marker.id).or_default().push(marker.family);
        }
        for marker in &new_comments {
            new_order.entry(marker.id).or_default().push(marker.family);
        }
        let changed = old_order
            .keys()
            .chain(new_order.keys())
            .copied()
            .filter(|id| old_order.get(id) != new_order.get(id))
            .collect::<HashSet<_>>();
        let mut prepared = document.clone_for_staging();
        prepared.flush_to_package()?;
        let inventory = prepared.comment_source_inventory_at(&prepared.doc_part_name)?;
        if !inventory.contains_key(&story.part_name) {
            return Err(Error::Other(
                "element XML comment relationship scope is unsupported".into(),
            ));
        }
        let mut total = BTreeMap::<i32, [usize; 3]>::new();
        for markers in inventory.values() {
            for (id, counts) in counts(markers)? {
                let entry = total.entry(id).or_default();
                for family in 0..3 {
                    entry[family] = entry[family].checked_add(counts[family]).ok_or_else(|| {
                        Error::Other("element XML comment inventory overflow".into())
                    })?;
                }
            }
        }
        for id in &changed {
            let id = *id;
            let old = before.get(&id).copied().unwrap_or_default();
            let new = after.get(&id).copied().unwrap_or_default();
            if new.iter().any(|count| *count > 1) || new[0] != new[1] {
                return Err(Error::Other(
                    "element XML changed comment span outside the selected owner is unsupported"
                        .into(),
                ));
            }
            if new != [0; 3] {
                prepared.fragment_comment_dependency(id)?;
            }
            let all = total.get(&id).copied().unwrap_or_default();
            let mut combined = [0usize; 3];
            for family in 0..3 {
                combined[family] = all[family]
                    .checked_sub(old[family])
                    .and_then(|outside| outside.checked_add(new[family]))
                    .ok_or_else(|| {
                        Error::Other("element XML comment owner correspondence is invalid".into())
                    })?;
            }
            if combined.iter().any(|count| *count > 1) || combined[0] != combined[1] {
                return Err(Error::Other(
                    "element XML comment ID collides or leaves an unmatched physical owner".into(),
                ));
            }
        }
        let mut started = HashSet::new();
        for marker in &new_comments {
            if !changed.contains(&marker.id) {
                continue;
            }
            if marker.family == 0 {
                started.insert(marker.id);
            } else if marker.family == 1 && !started.contains(&marker.id) {
                return Err(Error::Other(
                    "element XML comment end precedes its start".into(),
                ));
            }
        }
    }
    let counts = |markers: &[BookmarkMarkerSpan]| -> Result<BTreeMap<i32, [usize; 2]>> {
        let mut counts = BTreeMap::new();
        for marker in markers {
            let id = marker
                .id
                .parse::<i32>()
                .map_err(|_| Error::Other("element XML bookmark has an invalid ID".into()))?;
            let count = counts.entry(id).or_insert([0usize; 2]);
            let family = usize::from(!marker.start);
            count[family] = count[family]
                .checked_add(1)
                .ok_or_else(|| Error::Other("element XML bookmark count overflow".into()))?;
        }
        Ok(counts)
    };
    let previous_markers = bookmark_marker_spans(previous)?;
    let incoming_markers = bookmark_marker_spans(incoming)?;
    let before = counts(&previous_markers)?;
    let after = counts(&incoming_markers)?;
    let mut old_order = BTreeMap::<i32, Vec<bool>>::new();
    let mut new_order = BTreeMap::<i32, Vec<bool>>::new();
    for marker in &previous_markers {
        old_order
            .entry(marker.id.parse().unwrap())
            .or_default()
            .push(marker.start);
    }
    for marker in &incoming_markers {
        new_order
            .entry(marker.id.parse().unwrap())
            .or_default()
            .push(marker.start);
    }
    let changed = old_order
        .keys()
        .chain(new_order.keys())
        .copied()
        .filter(|id| old_order.get(id) != new_order.get(id))
        .collect::<HashSet<_>>();
    if changed.is_empty() {
        return Ok(());
    }
    let (source, _) = document.story_source_and_owner(story)?;
    let total = counts(&bookmark_marker_spans(&source.xml)?)?;
    for id in &changed {
        let id = *id;
        let old = before.get(&id).copied().unwrap_or_default();
        let new = after.get(&id).copied().unwrap_or_default();
        if new != [0, 0] && new != [1, 1] {
            return Err(Error::Other(
                "element XML changed bookmark span outside the selected owner is unsupported"
                    .into(),
            ));
        }
        let total = total.get(&id).copied().unwrap_or_default();
        let mut combined = [0usize; 2];
        for family in 0..2 {
            combined[family] = total[family]
                .checked_sub(old[family])
                .and_then(|outside| outside.checked_add(new[family]))
                .ok_or_else(|| {
                    Error::Other("element XML bookmark owner correspondence is invalid".into())
                })?;
        }
        if combined != [0, 0] && combined != [1, 1] {
            return Err(Error::Other(
                "element XML bookmark ID collides or leaves an unmatched physical owner".into(),
            ));
        }
    }
    let mut started = HashSet::new();
    for marker in &incoming_markers {
        let id = marker
            .id
            .parse::<i32>()
            .map_err(|_| Error::Other("element XML bookmark has an invalid ID".into()))?;
        if !changed.contains(&id) {
            continue;
        }
        if marker.start {
            started.insert(id);
        } else if !started.contains(&id) {
            return Err(Error::Other(
                "element XML bookmark end precedes its start".into(),
            ));
        }
    }
    Ok(())
}

fn validate_element_owner_relationships(
    document: &Document,
    story: &StoryId,
    root: WordXmlOwner<'_>,
) -> Result<()> {
    let mut pending = vec![root];
    while let Some(owner) = pending.pop() {
        match owner {
            WordXmlOwner::Paragraph(paragraph) => {
                for hyperlink in &paragraph.hyperlinks {
                    if let Some(id) = &hyperlink.rel_id {
                        document.hyperlink_url_for_story(story, id)?;
                    }
                }
                pending.extend(paragraph.runs.iter().map(WordXmlOwner::Run));
                pending.extend(
                    paragraph
                        .content_controls
                        .iter()
                        .map(|(_, _, _, control)| WordXmlOwner::Control(control)),
                );
                for (_, _, revision) in &paragraph.revisions {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordXmlOwner::Paragraph(paragraph));
                    }
                }
                for ruby in &paragraph.rubies {
                    pending.extend(ruby.ruby_text.iter().map(WordXmlOwner::Run));
                }
            }
            WordXmlOwner::Run(run) => {
                for content in &run.content {
                    match content {
                        RunContent::Field(field) => {
                            field.validate_for_attachment()?;
                            // A new whole raw owner has no retained-program authority.
                            oxoffice_docx_oxml::text::Field::from_raw(
                                &field.effective_instruction_text(),
                                field.form(),
                                Vec::new(),
                            )?;
                            let instruction = field.effective_instruction();
                            oxoffice_docx_oxml::text::FieldInstruction::new(
                                &instruction.name,
                                instruction.arguments,
                                instruction.switches,
                            )?;
                            if let Some(owner) = field.cached_result_owner_projection()? {
                                validate_element_owner_relationships(
                                    document,
                                    story,
                                    WordXmlOwner::Paragraph(&owner),
                                )?;
                            } else if let Some(runs) = field.cached_result_runs() {
                                pending.extend(runs.iter().map(WordXmlOwner::Run));
                            }
                        }
                        RunContent::FootnoteRef { id, .. } | RunContent::EndnoteRef { id, .. } => {
                            if document.glossary_part_name.as_deref()
                                == Some(story.part_name.as_str())
                            {
                                return Err(Error::Other(
                                    "element XML glossary note reference scope is unsupported"
                                        .into(),
                                ));
                            }
                            let kind = if matches!(content, RunContent::FootnoteRef { .. }) {
                                StoryKind::Footnote
                            } else {
                                StoryKind::Endnote
                            };
                            document.fragment_note_dependency(kind, *id)?;
                        }
                        RunContent::Drawing(drawing) => {
                            if drawing.inline.is_some() && drawing.anchor.is_some() {
                                return Err(Error::Other(
                                    "element XML drawing has two modeled owners".into(),
                                ));
                            }
                            let relationships = drawing
                                .inline
                                .as_ref()
                                .map(|inline| {
                                    (
                                        &inline.embed_id,
                                        inline.link_id.as_deref(),
                                        inline.chart_rel_id.as_deref(),
                                    )
                                })
                                .or_else(|| {
                                    drawing.anchor.as_ref().map(|anchor| {
                                        (
                                            &anchor.embed_id,
                                            anchor.link_id.as_deref(),
                                            anchor.chart_rel_id.as_deref(),
                                        )
                                    })
                                });
                            if let Some((embed, link, chart)) = relationships {
                                if !embed.is_empty() {
                                    let target = document
                                        .validate_internal_relationship_for_story(
                                            story,
                                            embed,
                                            rel_types::IMAGE,
                                        )?;
                                    if !document
                                        .package
                                        .content_types
                                        .content_type_for(&target)
                                        .is_some_and(|kind| kind.starts_with("image/"))
                                    {
                                        return Err(Error::Other("element XML image relationship targets a non-image part".into()));
                                    }
                                }
                                if let Some(id) = link {
                                    let relationship = document
                                        .package
                                        .get_part_rels(&story.part_name)
                                        .and_then(|relationships| relationships.get_by_id(id))
                                        .ok_or_else(|| {
                                            Error::Other(
                                                "element XML linked image relationship is missing"
                                                    .into(),
                                            )
                                        })?;
                                    if relationship.rel_type != rel_types::IMAGE
                                        || relationship.target_mode.as_deref() != Some("External")
                                    {
                                        return Err(Error::Other("element XML linked image requires an external image relationship".into()));
                                    }
                                }
                                if let Some(id) = chart {
                                    let target = document
                                        .validate_internal_relationship_for_story(
                                            story,
                                            id,
                                            rel_types::CHART,
                                        )?;
                                    let source =
                                        document.package.get_part(&target).ok_or_else(|| {
                                            Error::Other(
                                                "element XML chart target is missing".into(),
                                            )
                                        })?;
                                    if document.package.content_types.content_type_for(&target)
                                        != Some(content_types::CHART)
                                    {
                                        return Err(Error::Other("element XML chart relationship targets a non-chart part".into()));
                                    }
                                    CT_ChartSpace::from_xml(source).map_err(|error| {
                                        Error::Other(format!("element XML chart target: {error}"))
                                    })?;
                                }
                            }
                            if let Some(shape) = drawing
                                .anchor
                                .as_ref()
                                .and_then(|anchor| anchor.shape.as_ref())
                            {
                                if let Some(body) = &shape.text_body {
                                    for content in &body.content {
                                        match content {
                                            BodyContent::Paragraph(paragraph) => {
                                                pending.push(WordXmlOwner::Paragraph(paragraph))
                                            }
                                            BodyContent::Table(table) => {
                                                pending.push(WordXmlOwner::Table(table))
                                            }
                                            BodyContent::ContentControl(control) => {
                                                pending.push(WordXmlOwner::Control(control))
                                            }
                                            BodyContent::RawXml(_) => {}
                                        }
                                    }
                                } else {
                                    pending.extend(shape.text.iter().map(WordXmlOwner::Paragraph));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WordXmlOwner::Table(table) => {
                validate_table_topology(table)?;
                pending.extend(table.rows.iter().map(WordXmlOwner::Row));
                pending.extend(
                    table
                        .content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Row(row) => {
                pending.extend(row.cells.iter().map(WordXmlOwner::Cell));
                pending.extend(
                    row.content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Cell(cell) => {
                for content in &cell.content {
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            pending.push(WordXmlOwner::Paragraph(paragraph))
                        }
                        CellContent::Table(table) => pending.push(WordXmlOwner::Table(table)),
                        CellContent::ContentControl(control) => {
                            pending.push(WordXmlOwner::Control(control))
                        }
                    }
                }
            }
            WordXmlOwner::Control(control) => {
                for content in &control.content {
                    match content {
                        SdtContent::Paragraph(paragraph) => {
                            pending.push(WordXmlOwner::Paragraph(paragraph))
                        }
                        SdtContent::Run(run) => pending.push(WordXmlOwner::Run(run)),
                        SdtContent::Table(table) => pending.push(WordXmlOwner::Table(table)),
                        SdtContent::Row(row) => pending.push(WordXmlOwner::Row(row)),
                        SdtContent::Cell(cell) => pending.push(WordXmlOwner::Cell(cell)),
                        SdtContent::ContentControl(control) => {
                            pending.push(WordXmlOwner::Control(control))
                        }
                        SdtContent::RawXml(_) => {}
                    }
                }
                for (_, revision) in control.revisions() {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordXmlOwner::Paragraph(paragraph));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_fragment_relationships(
    document: &Document,
    destination: &StoryId,
    fragment: &ContentFragment,
) -> Result<()> {
    let inherited = fragment
        .namespace_scope
        .iter()
        .map(|(prefix, namespace)| {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            (name, namespace.clone())
        })
        .collect::<Vec<_>>();
    let relationship_ids = xml_relationship_ids_in_order_with_bindings(&fragment.xml, &inherited)?;
    if relationship_ids.is_empty() {
        return Ok(());
    }
    if !story_scope_matches_fragment(destination, fragment) {
        return Err(Error::Other(
            "content fragment relationship references require the unchanged story owner".to_owned(),
        ));
    }
    let relationships = document
        .package
        .get_part_rels(&destination.part_name)
        .ok_or_else(|| {
            Error::Other(format!(
                "content fragment owner {} has no relationship scope",
                destination.part_name
            ))
        })?;
    for id in relationship_ids {
        if relationships.get_by_id(&id).is_none() {
            return Err(Error::Other(format!(
                "content fragment references missing relationship {id} in {}",
                destination.part_name
            )));
        }
    }
    Ok(())
}

fn content_fragment_for_insertion(
    document: &Document,
    destination: &StoryId,
    fragment: &ContentFragment,
) -> Result<Vec<u8>> {
    validate_fragment_relationships(document, destination, fragment)?;
    if fragment.source_part_name.is_none() || story_scope_matches_fragment(destination, fragment) {
        Ok(fragment.xml.clone())
    } else {
        close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)
    }
}

fn freshen_content_fragment_identities(
    document: &mut Document,
    fragment: &ContentFragment,
) -> Result<Vec<u8>> {
    let mut wrapper_prefix = "rdocxw".to_owned();
    let mut suffix = 1usize;
    while fragment.namespace_scope.contains_key(&wrapper_prefix) {
        wrapper_prefix = format!("rdocxw{suffix}");
        suffix = suffix
            .checked_add(1)
            .ok_or_else(|| Error::Other("content wrapper prefix space exhausted".to_owned()))?;
    }
    let mut prefix =
        format!(r#"<{wrapper_prefix}:document xmlns:{wrapper_prefix}="{WORD_NAMESPACE}""#)
            .into_bytes();
    for (namespace_prefix, namespace) in &fragment.namespace_scope {
        if namespace_prefix == "xml" || namespace_prefix == &wrapper_prefix {
            continue;
        }
        prefix.extend_from_slice(b" xmlns");
        if !namespace_prefix.is_empty() {
            prefix.push(b':');
            prefix.extend_from_slice(namespace_prefix.as_bytes());
        }
        prefix.extend_from_slice(b"=\"");
        prefix.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
        prefix.push(b'"');
    }
    prefix.extend_from_slice(format!("><{wrapper_prefix}:body>").as_bytes());
    let suffix = format!("</{wrapper_prefix}:body></{wrapper_prefix}:document>");
    let mut wrapped = prefix;
    wrapped.extend_from_slice(&fragment.xml);
    wrapped.extend_from_slice(suffix.as_bytes());
    let updated = crate::field::freshen_content_fragment_identities(document, &wrapped)?;
    let updated = remove_comment_anchors_from_fragment(&updated)?;
    let owner = scan_story_owners(&updated, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("rewritten content wrapper has no body".to_owned()))?;
    let items = direct_story_content_items(&updated, &owner)?;
    let [item] = items.as_slice() else {
        return Err(Error::Other(
            "rewritten content wrapper must contain exactly one item".to_owned(),
        ));
    };
    Ok(updated[item.full.clone()].to_vec())
}

fn remove_comment_anchors_from_fragment(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut ranges = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comment anchor scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Empty(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
                    ) =>
            {
                ranges.push(before..after);
            }
            Event::Start(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
                    ) =>
            {
                ranges.push(before..story_element_end(xml, before)?);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = Vec::with_capacity(xml.len());
    let mut cursor = 0usize;
    for range in ranges {
        result.extend_from_slice(&xml[cursor..range.start]);
        cursor = range.end;
    }
    result.extend_from_slice(&xml[cursor..]);
    Ok(result)
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

pub(crate) fn set_story_source_xml(
    document: &mut Document,
    part_name: &str,
    xml: Vec<u8>,
) -> Result<()> {
    set_story_source_xml_in(document, part_name, xml, None)
}

fn set_story_source_xml_in(
    document: &mut Document,
    part_name: &str,
    xml: Vec<u8>,
    mut scoped: Option<(
        &mut TemplateMainWriter<'_, '_>,
        oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<()> {
    if part_name == document.doc_part_name {
        // The main-part story source is canonical XML. It drops a root
        // default namespace and every body declaration, and binds the root
        // `w`, `r` and `mc` prefixes to their canonical URIs. The save path's
        // check on those declarations keeps retained content of the replaced
        // part in its namespace. A checked paragraph write emits canonical
        // `w14:paraId` without a local declaration, so the part root binds
        // that prefix before this source is parsed again.
        let rewritten_root_declarations: Vec<_> = document
            .root_namespace_declarations
            .iter()
            .filter(|(name, _)| {
                matches!(name.as_str(), "xmlns" | "xmlns:w" | "xmlns:r" | "xmlns:mc")
            })
            .cloned()
            .collect();
        if let Some(prefix) = unsafe_serializer_namespace_prefix(
            &rewritten_root_declarations,
            &document.body_namespace_declarations,
            document.package.get_part(part_name),
        ) {
            return Err(Error::Other(format!(
                "cannot serialize a modified document with a shadowed `{prefix}` namespace"
            )));
        }
        let (xml, mut main_scoped) = if let Some((template, source)) = scoped.take() {
            let source = template.operation.declare_source_w14_on_root(source)?;
            let retained = template.operation.repeat_source(&source)?;
            (
                template.operation.finish_source(retained)?,
                Some((template, source)),
            )
        } else {
            let mut xml = xml;
            declare_w14_on_part_root(&mut xml)?;
            (xml, None)
        };
        let namespace_scopes = document_namespace_scopes(&xml)?;
        let xml = Arc::new(xml);
        let rebuilt = if main_scoped.is_none() {
            document.source_blocks(&xml)
        } else {
            None
        };
        let (parsed, blocks) = match rebuilt {
            Some((parsed, blocks)) => (parsed, Some(blocks)),
            None => {
                let parsed = if let Some((template, source)) = main_scoped.take() {
                    template.sources.invalidate_model();
                    let parsed = template
                        .sources
                        .read_document(template.operation, Arc::new(source))?;
                    template.sources.published_main = Some((
                        Arc::clone(&xml),
                        Arc::clone(
                            template
                                .sources
                                .reader_source
                                .as_ref()
                                .expect("actual main reader input"),
                        ),
                    ));
                    parsed
                } else {
                    CT_Document::from_xml(&xml)?
                };
                (parsed, None)
            }
        };
        #[cfg(debug_assertions)]
        if blocks.is_some() {
            {
                let whole = CT_Document::from_xml(&xml).expect("proved source parses whole");
                assert_eq!(parsed, whole, "source pieces changed the parsed model");
                assert_eq!(
                    parsed.to_xml().ok(),
                    whole.to_xml().ok(),
                    "source normalization changed"
                );
            }
        }
        let nested_namespace_owners = OnceLock::new();
        if let Some(owners) = blocks
            .as_ref()
            .and_then(|blocks| blocks.retained_nested_owners(&xml))
        {
            let _ = nested_namespace_owners.set(Some(owners));
        }
        let (blocked, unsavable) = plain_local_edit_facts(
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            &xml,
            &nested_namespace_owners,
        );
        document.plain_local_edit_blocked = blocked;
        document.unsavable_body_edit_cache = unsavable;
        document.document = crate::document::DocumentModel::unproved(parsed);
        document.main_part_blocks = blocks;
        document.package.set_part(part_name, xml);
        document.root_namespace_declarations = namespace_scopes.root_declarations;
        document.body_namespace_declarations = namespace_scopes.body_declarations;
        document.body_namespace_bindings = namespace_scopes.body_bindings;
        document.local_cell_source = LocalCellSource::classified(
            &document.package,
            &document.doc_part_name,
            &document.root_namespace_declarations,
            &document.body_namespace_declarations,
            !blocked,
        );
    } else if document.comments_part_name.as_deref() == Some(part_name) {
        let mut comments = if let Some((writer, source)) = scoped.as_mut() {
            let comments = writer.operation.source_comments(source)?;
            // The operation reader and package publication below are the same producer handback.
            comments
        } else {
            oxoffice_docx_oxml::comments::CT_Comments::from_xml(&xml)?
        };
        let changed = document.reconcile_comment_paragraph_publication(&mut comments)?;
        document.comments = Some(comments);
        document.package.set_part(part_name, xml);
        // Existing ordinary/scoped model writer emits the generated attributes.
        document.comments_dirty = changed;
    } else if document.footnotes_part_name.as_deref() == Some(part_name) {
        document.footnotes = if let Some((template, source)) = scoped.as_mut() {
            let notes = template.operation.source_notes(source)?;
            template.sources.notes_model_read = true;
            template.sources.read_note_count = notes.footnotes.len();
            notes
        } else {
            oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&xml)?
        };
        document.package.set_part(part_name, xml);
        document.footnotes_dirty = false;
    } else if document.glossary_part_name.as_deref() == Some(part_name) {
        document.glossary = Some(oxoffice_docx_oxml::glossary::CT_GlossaryDocument::from_xml(
            &xml,
        )?);
        document.package.set_part(part_name, xml);
        document.glossary_dirty = false;
    } else {
        document.package.set_part(part_name, xml);
    }
    if let Some((template, source)) = scoped {
        let bytes = document
            .package
            .get_part_shared(part_name)
            .ok_or_else(|| Error::Other("published template part disappeared".into()))?;
        let source = Arc::new(source);
        if document.comments_part_name.as_deref() == Some(part_name) {
            template.sources.comments_reader_source = Some(Arc::clone(&source));
        }
        template
            .sources
            .put_related_source(part_name, bytes, source);
    }
    Ok(())
}

// Source-qualified types, rather than the legacy typed unknown-type fallback,
// decide whether an imported part already owns an authored special record.
fn missing_note_specials(
    existing: &oxoffice_docx_oxml::footnotes::CT_Footnotes,
    source: Option<&[u8]>,
    kind: StoryKind,
    pending_normal_id: Option<i32>,
) -> Result<Vec<oxoffice_docx_oxml::footnotes::CT_Footnote>> {
    use crate::comparison::{RawNoteType, raw_note_attribute_in_scope, raw_note_type_in_scope};
    use oxoffice_docx_oxml::footnotes::{CT_Footnote, NoteType};
    let mut separator = false;
    let mut continuation = false;
    let mut occupied = existing
        .footnotes
        .iter()
        .map(|note| note.id)
        .collect::<HashSet<_>>();
    if let Some(id) = pending_normal_id {
        occupied.insert(id);
    }
    if let Some(source) = source {
        for owner in scan_story_owners_with_special(source, kind, true)? {
            if owner.kind != kind {
                continue;
            }
            let scope = story_namespace_scope_at(source, owner.full.start)?;
            let xml = std::str::from_utf8(&source[owner.full])
                .map_err(|error| Error::Other(format!("note source is not UTF-8: {error}")))?;
            let bindings = scope
                .into_iter()
                .map(|(prefix, uri)| {
                    (
                        if prefix.is_empty() {
                            "xmlns".into()
                        } else {
                            format!("xmlns:{prefix}")
                        },
                        uri,
                    )
                })
                .collect::<Vec<_>>();
            if let Some(id) = raw_note_attribute_in_scope(xml, &bindings, "id")?
                .and_then(|id| id.parse::<i32>().ok())
            {
                occupied.insert(id);
            }
            match raw_note_type_in_scope(xml, &bindings)? {
                RawNoteType::Separator => separator = true,
                RawNoteType::ContinuationSeparator => continuation = true,
                _ => {}
            }
        }
    }
    let mut additions = Vec::new();
    for (id, note_type, present, child) in [
        (
            -1,
            NoteType::Separator,
            separator,
            b"<w:separator/>".as_slice(),
        ),
        (
            0,
            NoteType::ContinuationSeparator,
            continuation,
            b"<w:continuationSeparator/>".as_slice(),
        ),
    ] {
        if present || occupied.contains(&id) {
            continue;
        }
        let mut run = CT_R::new("");
        run.content.clear();
        run.extra_xml.push(child.to_vec());
        run.extra_xml_positions.push(0);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(run);
        additions.push(CT_Footnote {
            id,
            note_type,
            paragraphs: vec![paragraph],
        });
    }
    Ok(additions)
}

fn owners_reference_hyperlink_style(mut pending: Vec<WordXmlOwner<'_>>) -> bool {
    fn styled(run: &CT_R) -> bool {
        run.properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref())
            == Some("Hyperlink")
    }
    fn revision_styled(revision: &CT_Revision) -> bool {
        if let Some(paragraph) = revision.content_paragraph() {
            return paragraph.source_runs().into_iter().any(styled);
        }
        matches!(revision.content(), oxoffice_docx_oxml::revision::RevisionContent::Runs(runs) if runs.iter().any(styled))
            || revision
                .nested_revisions()
                .iter()
                .any(|(_, nested)| revision_styled(nested))
    }
    while let Some(owner) = pending.pop() {
        match owner {
            WordXmlOwner::Paragraph(paragraph) => {
                if paragraph.source_runs().into_iter().any(styled) {
                    return true;
                }
            }
            WordXmlOwner::Run(run) => {
                if styled(run) {
                    return true;
                }
            }
            WordXmlOwner::Table(table) => {
                pending.extend(table.rows.iter().map(WordXmlOwner::Row));
                pending.extend(
                    table
                        .content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Row(row) => {
                pending.extend(row.cells.iter().map(WordXmlOwner::Cell));
                pending.extend(
                    row.content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Cell(cell) => {
                pending.extend(cell.content.iter().map(|item| match item {
                    CellContent::Paragraph(paragraph) => WordXmlOwner::Paragraph(paragraph),
                    CellContent::Table(table) => WordXmlOwner::Table(table),
                    CellContent::ContentControl(control) => WordXmlOwner::Control(control),
                }));
            }
            WordXmlOwner::Control(control) => {
                if control
                    .revisions()
                    .iter()
                    .any(|(_, revision)| revision_styled(revision))
                {
                    return true;
                }
                for item in &control.content {
                    match item {
                        SdtContent::Paragraph(paragraph) => {
                            pending.push(WordXmlOwner::Paragraph(paragraph))
                        }
                        SdtContent::Run(run) => pending.push(WordXmlOwner::Run(run)),
                        SdtContent::Table(table) => pending.push(WordXmlOwner::Table(table)),
                        SdtContent::Row(row) => pending.push(WordXmlOwner::Row(row)),
                        SdtContent::Cell(cell) => pending.push(WordXmlOwner::Cell(cell)),
                        SdtContent::ContentControl(control) => {
                            pending.push(WordXmlOwner::Control(control))
                        }
                        SdtContent::RawXml(_) => {}
                    }
                }
            }
        }
    }
    false
}

fn serialized_note_fragment(
    note: &oxoffice_docx_oxml::footnotes::CT_Footnote,
    kind: StoryKind,
) -> Result<Vec<u8>> {
    let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes {
        footnotes: vec![note.clone()],
    };
    let serialized = match kind {
        StoryKind::Footnote => notes.to_xml_footnotes()?,
        StoryKind::Endnote => notes.to_xml_endnotes()?,
        _ => return Err(Error::Other("invalid note story kind".to_owned())),
    };
    let owner = scan_story_owners_with_special(&serialized, kind, true)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("serialized note has no story owner".to_owned()))?;
    let scope = story_namespace_scope_at(&serialized, owner.full.start)?;
    close_content_fragment_namespaces(&serialized[owner.full], &scope)
}

fn serialized_note_fragment_in(
    note: &oxoffice_docx_oxml::footnotes::CT_Footnote,
    kind: StoryKind,
    note_index: usize,
    read_source: Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
    template: &mut TemplateMainWriter<'_, '_>,
) -> Result<Option<oxoffice_docx_oxml::document::ScopedWordSource>> {
    let endnotes = match kind {
        StoryKind::Footnote => false,
        StoryKind::Endnote => true,
        _ => return Err(Error::Other("invalid note story kind".into())),
    };
    let emitted = if let Some(source) = read_source {
        template
            .operation
            .source_model_note_from_read(note, endnotes, source, note_index)?
    } else {
        if template.sources.notes_model_read {
            return Ok(None);
        }
        // Preserve the existing one-note clone producer. Only actual initial
        // clone slots can be paired, never a note ID or matching text value.
        for paragraph in &note.paragraphs {
            paragraph.validate_resource_limits()?;
        }
        let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes {
            footnotes: vec![note.clone()],
        };
        let Some(inputs) = template
            .lineage
            .note_writer_inputs(note_index, &notes.footnotes[0])
        else {
            return Ok(None);
        };
        template.operation.source_model_notes(
            &notes,
            endnotes,
            &inputs.texts,
            &inputs.fields,
            &inputs.raw,
            false,
        )?
    };
    let owner = scan_story_owners_with_special(emitted.as_bytes(), kind, true)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("serialized note has no story owner".into()))?;
    let scope = story_namespace_scope_at(emitted.as_bytes(), owner.full.start)?;
    let selected = template
        .operation
        .compose_sources(&[(Some(&emitted), owner.full, &[])])?;
    Ok(Some(
        template
            .operation
            .close_source_namespaces(selected, &scope)?,
    ))
}

fn serialized_footnote_fragment(
    footnote: &oxoffice_docx_oxml::footnotes::CT_Footnote,
) -> Result<Vec<u8>> {
    serialized_note_fragment(footnote, StoryKind::Footnote)
}

fn remove_note_references(xml: &[u8], id: i32, kind: StoryKind) -> Result<Vec<u8>> {
    let local_name = match kind {
        StoryKind::Footnote => b"footnoteReference".as_slice(),
        StoryKind::Endnote => b"endnoteReference".as_slice(),
        _ => return Err(Error::Other("invalid note story kind".to_owned())),
    };
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut ranges = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("footnote reference scan failed: {error}")))?;
        let start = match &event {
            Event::Start(start) | Event::Empty(start)
                if word_element(&namespace) && start.local_name().as_ref() == local_name =>
            {
                start
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let mut reference_id = None;
        let mut custom_mark_follows = false;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| {
                Error::Other(format!("footnote reference attribute scan failed: {error}"))
            })?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if !word_element(&namespace) {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!("footnote reference attribute is invalid: {error}"))
                })?;
            match local.as_ref() {
                b"id" => reference_id = value.parse::<i32>().ok(),
                b"customMarkFollows" => {
                    custom_mark_follows = matches!(value.as_ref(), "1" | "true" | "on");
                }
                _ => {}
            }
        }
        if reference_id == Some(id) {
            let end = if matches!(event, Event::Empty(_)) {
                reader.buffer_position() as usize
            } else {
                story_element_end(xml, before)?
            };
            let event = event.into_owned();
            let mut mark_range = None;
            if custom_mark_follows {
                // The custom mark is the text, deleted text or symbol that
                // follows the reference in its run, as Word reads it: it
                // goes with the note. Whitespace, XML comments, processing
                // instructions and a rendered page break between them are
                // passed over and kept.
                if let Event::Start(reference) = &event {
                    let name = reference.name().as_ref().to_vec();
                    reader
                        .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("custom mark scan failed: {error}"))
                        })?;
                }
                let mut next_buffer = Vec::new();
                loop {
                    let offset = reader.buffer_position() as usize;
                    let (namespace, next) = reader
                        .read_resolved_event_into(&mut next_buffer)
                        .map_err(|error| {
                            Error::Other(format!("custom mark scan failed: {error}"))
                        })?;
                    match next {
                        Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
                        Event::Comment(_) | Event::PI(_) => {}
                        Event::Empty(hint)
                            if word_element(&namespace)
                                && hint.local_name().as_ref() == b"lastRenderedPageBreak" => {}
                        Event::Start(hint)
                            if word_element(&namespace)
                                && hint.local_name().as_ref() == b"lastRenderedPageBreak" =>
                        {
                            let name = hint.name().as_ref().to_vec();
                            reader
                                .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                                .map_err(|error| {
                                    Error::Other(format!("custom mark scan failed: {error}"))
                                })?;
                        }
                        Event::Start(mark)
                            if word_element(&namespace)
                                && matches!(
                                    mark.local_name().as_ref(),
                                    b"t" | b"delText" | b"sym"
                                ) =>
                        {
                            mark_range = Some(offset..story_element_end(xml, offset)?);
                            let name = mark.name().as_ref().to_vec();
                            reader
                                .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                                .map_err(|error| {
                                    Error::Other(format!("custom mark scan failed: {error}"))
                                })?;
                            break;
                        }
                        Event::Empty(mark)
                            if word_element(&namespace)
                                && matches!(
                                    mark.local_name().as_ref(),
                                    b"t" | b"delText" | b"sym"
                                ) =>
                        {
                            mark_range = Some(offset..reader.buffer_position() as usize);
                            break;
                        }
                        _ => break,
                    }
                    next_buffer.clear();
                }
            }
            ranges.push(before..end);
            ranges.extend(mark_range);
        }
        buffer.clear();
    }
    let mut updated = xml.to_vec();
    for range in ranges.into_iter().rev() {
        updated.drain(range);
    }
    Ok(updated)
}

enum StoryAppendPosition {
    At(usize),
    Empty {
        insertion: usize,
        after: usize,
        name: Vec<u8>,
    },
}

fn story_append_position(
    xml: &[u8],
    root_local_name: &[u8],
    item_local_name: &[u8],
) -> Result<StoryAppendPosition> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut after_last_item = None;
    let boundary = loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid footnotes XML: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                if !saw_root {
                    if is_word && element.local_name().as_ref() == root_local_name {
                        saw_root = true;
                        depth = 1;
                    }
                } else {
                    if depth == 1 && is_word && element.local_name().as_ref() == item_local_name {
                        after_last_item = Some(story_element_end(xml, before)?);
                    }
                    depth += 1;
                }
            }
            Event::Empty(element) => {
                if !saw_root && is_word && element.local_name().as_ref() == root_local_name {
                    let name = element.name().as_ref().to_vec();
                    let insertion = after.checked_sub(2).ok_or_else(|| {
                        Error::Other("empty story root opening tag is incomplete".to_owned())
                    })?;
                    return Ok(StoryAppendPosition::Empty {
                        insertion,
                        after,
                        name,
                    });
                }
                if saw_root
                    && depth == 1
                    && is_word
                    && element.local_name().as_ref() == item_local_name
                {
                    after_last_item = Some(after);
                }
            }
            Event::End(element) => {
                if saw_root
                    && depth == 1
                    && is_word
                    && element.local_name().as_ref() == root_local_name
                {
                    break after_last_item.unwrap_or(before);
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => {
                return Err(Error::Other(format!(
                    "story XML has no closing {} root element",
                    String::from_utf8_lossy(root_local_name)
                )));
            }
            _ => {}
        }
        buffer.clear();
    };
    Ok(StoryAppendPosition::At(boundary))
}

fn append_story_fragments_to_root(
    xml: &[u8],
    root_local_name: &[u8],
    item_local_name: &[u8],
    fragments: &[Vec<u8>],
) -> Result<Vec<u8>> {
    if fragments.is_empty() {
        return Ok(xml.to_vec());
    }
    match story_append_position(xml, root_local_name, item_local_name)? {
        StoryAppendPosition::At(boundary) => {
            let mut updated = xml.to_vec();
            updated.splice(
                boundary..boundary,
                fragments
                    .iter()
                    .flat_map(|fragment| fragment.iter().copied()),
            );
            Ok(updated)
        }
        StoryAppendPosition::Empty {
            insertion,
            after,
            name,
        } => {
            let mut updated = Vec::new();
            updated.extend_from_slice(&xml[..insertion]);
            updated.push(b'>');
            updated.extend(
                fragments
                    .iter()
                    .flat_map(|fragment| fragment.iter().copied()),
            );
            updated.extend_from_slice(b"</");
            updated.extend_from_slice(&name);
            updated.push(b'>');
            updated.extend_from_slice(&xml[after..]);
            Ok(updated)
        }
    }
}

fn append_story_sources_to_root(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: impl Into<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    fragments: &[Arc<oxoffice_docx_oxml::document::ScopedWordSource>],
    root_local_name: &[u8],
    item_local_name: &[u8],
) -> Result<Arc<oxoffice_docx_oxml::document::ScopedWordSource>> {
    let source = source.into();
    if fragments.is_empty() {
        return Ok(source);
    }
    let position = story_append_position(source.as_bytes(), root_local_name, item_local_name)?;
    let mut chunks = Vec::new();
    let close;
    match position {
        StoryAppendPosition::At(boundary) => {
            chunks.push((Some(source.as_ref()), 0..boundary, &[][..]));
            for fragment in fragments {
                chunks.push((
                    Some(fragment.as_ref()),
                    0..fragment.as_bytes().len(),
                    &[][..],
                ));
            }
            chunks.push((
                Some(source.as_ref()),
                boundary..source.as_bytes().len(),
                &[][..],
            ));
        }
        StoryAppendPosition::Empty {
            insertion,
            after,
            name,
        } => {
            close = [b"</".as_slice(), name.as_slice(), b">".as_slice()].concat();
            chunks.push((Some(source.as_ref()), 0..insertion, &[][..]));
            chunks.push((None, 0..0, b">".as_slice()));
            for fragment in fragments {
                chunks.push((
                    Some(fragment.as_ref()),
                    0..fragment.as_bytes().len(),
                    &[][..],
                ));
            }
            chunks.push((None, 0..0, close.as_slice()));
            chunks.push((
                Some(source.as_ref()),
                after..source.as_bytes().len(),
                &[][..],
            ));
        }
    }
    Ok(Arc::new(operation.compose_sources(&chunks)?))
}

fn update_story_namespace_scope(
    scope: &mut BTreeMap<String, String>,
    element: &BytesStart<'_>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace scope attribute failed: {error}"))
        })?;
        let Some((prefix, value)) = decoded_story_namespace_declaration(element, &attribute)?
        else {
            continue;
        };
        if value.is_empty() {
            scope.remove(&prefix);
        } else {
            scope.insert(prefix, value);
        }
    }
    Ok(())
}

fn decoded_story_namespace_declaration(
    element: &BytesStart<'_>,
    attribute: &quick_xml::events::attributes::Attribute<'_>,
) -> Result<Option<(String, String)>> {
    let name = attribute.key.as_ref();
    let prefix = if name == b"xmlns" {
        ""
    } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
        std::str::from_utf8(prefix).map_err(|error| {
            Error::Other(format!("story namespace scope prefix is invalid: {error}"))
        })?
    } else {
        return Ok(None);
    };
    let value = attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
        .map_err(|error| Error::Other(format!("story namespace scope value is invalid: {error}")))?
        .into_owned();
    Ok(Some((prefix.to_owned(), value)))
}

fn story_word_prefix_for_scope(scope: &BTreeMap<String, String>) -> String {
    scope
        .iter()
        .find_map(|(prefix, namespace)| (namespace == WORD_NAMESPACE).then_some(prefix.clone()))
        .unwrap_or_else(|| {
            let mut suffix = 0usize;
            loop {
                let candidate = if suffix == 0 {
                    "rdocxw".to_owned()
                } else {
                    format!("rdocxw{suffix}")
                };
                if !scope.contains_key(&candidate) {
                    break candidate;
                }
                suffix += 1;
            }
        })
}

fn complex_story_field_xml(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<u8>> {
    let scope = story_namespace_scope_at(xml, item.scan.start)?;
    let word_prefix = story_word_prefix_for_scope(&scope);
    let mut bindings = scope;
    bindings
        .entry(word_prefix.clone())
        .or_insert_with(|| WORD_NAMESPACE.to_owned());
    let qualified_paragraph = if word_prefix.is_empty() {
        "p".to_owned()
    } else {
        format!("{word_prefix}:p")
    };
    let mut projection = format!("<{qualified_paragraph}").into_bytes();
    for (prefix, namespace) in bindings {
        projection.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            projection.push(b':');
            projection.extend_from_slice(prefix.as_bytes());
        }
        projection.extend_from_slice(b"=\"");
        projection.extend_from_slice(quick_xml::escape::escape(&namespace).as_bytes());
        projection.push(b'"');
    }
    projection.push(b'>');
    projection.extend_from_slice(&complex_story_field_runs(xml, item)?);
    projection.extend_from_slice(format!("</{qualified_paragraph}>").as_bytes());
    Ok(projection)
}

struct StoryRunShell {
    open_end: usize,
    close_start: usize,
    close_end: usize,
}

fn story_run_shell(xml: &[u8], start: usize) -> Result<StoryRunShell> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut active = false;
    let mut depth = 0usize;
    let mut open_end = None;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story run shell scan failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        if !active {
            if before != start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            if !matches!(event, Event::Start(_)) {
                return Err(Error::Other(
                    "complex story field does not begin in a run".to_owned(),
                ));
            }
            active = true;
            depth = 1;
            open_end = Some(after);
        } else {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(StoryRunShell {
                            open_end: open_end.unwrap_or(after),
                            close_start: before,
                            close_end: after,
                        });
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        buffer.clear();
    }
    Err(Error::Other(
        "complex story field run has no matching close".to_owned(),
    ))
}

fn complex_story_field_runs(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<u8>> {
    let first = story_run_shell(xml, item.scan.start)?;
    let mut last_start = item.scan.start;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("complex story run scan failed: {error}")))?;
        let is_word_namespace = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                if depth == 0 && is_word_namespace && element.local_name().as_ref() == b"r" {
                    last_start = before;
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
        if after == item.scan.end {
            break;
        }
        buffer.clear();
    }
    let last = story_run_shell(xml, last_start)?;
    let mut result = Vec::new();
    if last_start == item.scan.start {
        result.extend_from_slice(&xml[item.scan.start..first.open_end]);
        result.extend_from_slice(&xml[item.full.clone()]);
        result.extend_from_slice(&xml[first.close_start..first.close_end]);
        return Ok(result);
    }
    result.extend_from_slice(&xml[item.scan.start..first.open_end]);
    result.extend_from_slice(&xml[item.full.start..first.close_end]);
    result.extend_from_slice(&xml[first.close_end..last_start]);
    result.extend_from_slice(&xml[last_start..last.open_end]);
    result.extend_from_slice(&xml[last.open_end..item.full.end]);
    result.extend_from_slice(&xml[last.close_start..last.close_end]);
    Ok(result)
}

// Match CT_Sdt's actual first nonempty content slot. Empty content and
// subsequent content are retained raw, never visible modeled descendants.
fn admit_story_sdt_content_slot(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    empty: bool,
) -> bool {
    if namespace != StoryNamespace::Word || local_name != b"sdtContent" {
        return true;
    }
    let Some(parent) = stack.last_mut() else {
        return true;
    };
    if parent.namespace != StoryNamespace::Word || parent.local_name.as_ref() != b"sdt" {
        return true;
    }
    if empty || parent.typed_sdt_content_seen {
        return false;
    }
    parent.typed_sdt_content_seen = true;
    true
}

fn modeled_story_event(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    element: &BytesStart<'_>,
    empty: bool,
    xml: &[u8],
    before: usize,
    after: usize,
) -> Result<bool> {
    if !modeled_story_child(stack, namespace, local_name) {
        return Ok(false);
    }
    if !admit_story_sdt_content_slot(stack, namespace, local_name, empty) {
        return Ok(false);
    }
    if namespace == StoryNamespace::MarkupCompatibility && local_name == b"Choice" {
        // The text box is read from the first Choice that holds one, as layout
        // draws the first Choice drawing. Any other Choice stays opaque, as the
        // Fallback does.
        let Some(alternate) = stack.last() else {
            return Ok(false);
        };
        let Some(text_box) = first_text_box_start(xml, alternate.full_start)? else {
            return Ok(false);
        };
        return Ok(!empty && before < text_box && text_box < story_element_end(xml, before)?);
    }
    if namespace != StoryNamespace::Word {
        return Ok(true);
    }
    if matches!(
        local_name,
        b"sdt" | b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"fldSimple"
    ) {
        let inherited = stack
            .last()
            .map(|frame| Arc::clone(&frame.word_prefixes))
            .unwrap_or_else(|| Arc::new(Vec::new()));
        let word_prefixes = story_word_prefixes_at(element, &inherited, true)?;
        let raw_end = if empty {
            after
        } else {
            story_element_end(xml, before)?
        };
        let raw = xml
            .get(before..raw_end)
            .ok_or_else(|| Error::Other("story element lies outside its source part".to_owned()))?;
        if local_name == b"sdt" {
            let owner = match story_sdt_context(stack) {
                Some(StorySdtContext::Block) => StorySdtOwner::Block,
                Some(StorySdtContext::Table) => StorySdtOwner::Table,
                Some(StorySdtContext::Row) => StorySdtOwner::Row,
                Some(StorySdtContext::Inline) => StorySdtOwner::Inline,
                None => return Ok(false),
            };
            return Ok(CT_Sdt::story_raw_is_typed(raw, &word_prefixes, owner));
        }
        if matches!(local_name, b"ins" | b"del" | b"moveFrom" | b"moveTo") {
            return Ok(CT_Revision::story_raw_is_typed(raw, &word_prefixes));
        }
        return Ok(!empty && story_simple_field_is_typed(raw, &word_prefixes));
    }
    if empty && matches!(local_name, b"drawing" | b"pict" | b"sdt" | b"hyperlink") {
        return Ok(false);
    }
    Ok(true)
}

/// Where the first `txbxContent` element inside the element that starts at
/// `start` begins, whatever its prefix.
fn first_text_box_start(xml: &[u8], start: usize) -> Result<Option<usize>> {
    let fragment = xml
        .get(start..)
        .ok_or_else(|| Error::Other("story element begins outside its source part".to_owned()))?;
    let mut reader = quick_xml::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = start + reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story text box scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element)
                if depth > 0 && element.local_name().as_ref() == b"txbxContent" =>
            {
                return Ok(Some(before));
            }
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => return Ok(None),
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(None);
                }
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

struct PairedTextBoxCopy {
    alternate: usize,
    branch: usize,
    range: Range<usize>,
}

fn scoped_alternate_text_box_copies(xml: &[u8]) -> Result<Vec<PairedTextBoxCopy>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut copies = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("alternate text-box scan failed: {error}")))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                // Inspect both representations using the same qualified structural
                // routes as story discovery, without selecting or rewriting Fallback.
                let alternate_branch = namespace == StoryNamespace::MarkupCompatibility
                    && matches!(local, b"Choice" | b"Fallback")
                    && stack.last().is_some_and(|parent| {
                        parent.namespace == StoryNamespace::MarkupCompatibility
                            && parent.local_name.as_ref() == b"AlternateContent"
                    });
                let branch_content = namespace == StoryNamespace::Word
                    && matches!(local, b"drawing" | b"pict")
                    && stack.last().is_some_and(|parent| {
                        parent.namespace == StoryNamespace::MarkupCompatibility
                            && matches!(parent.local_name.as_ref(), b"Choice" | b"Fallback")
                    });
                let modeled = stack.last().is_none_or(|parent| !parent.opaque)
                    && (alternate_branch
                        || branch_content
                        || modeled_story_child(&stack, namespace, local))
                    && admit_story_sdt_content_slot(&mut stack, namespace, local, empty);
                if modeled && namespace == StoryNamespace::Word && local == b"txbxContent" {
                    let end = if empty {
                        reader.buffer_position() as usize
                    } else {
                        story_element_end(xml, before)?
                    };
                    for (index, alternate) in stack.iter().enumerate() {
                        if alternate.namespace != StoryNamespace::MarkupCompatibility
                            || alternate.local_name.as_ref() != b"AlternateContent"
                            || alternate.opaque
                        {
                            continue;
                        }
                        if let Some(branch) = stack.get(index + 1)
                            && branch.namespace == StoryNamespace::MarkupCompatibility
                            && matches!(branch.local_name.as_ref(), b"Choice" | b"Fallback")
                        {
                            copies.push(PairedTextBoxCopy {
                                alternate: alternate.full_start,
                                branch: branch.full_start,
                                range: before..end,
                            });
                        }
                    }
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes: Arc::clone(&empty_word_prefixes),
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(copies)
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

fn paired_text_box_projection(
    xml: &[u8],
    range: Range<usize>,
    mut operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
) -> Result<PairedTextBoxProjection> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut paths = Vec::<Option<Vec<usize>>>::new();
    let mut children = Vec::<usize>::new();
    let mut signature = Vec::new();
    let mut slots = Vec::new();
    let mut active = false;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("paired text-box projection failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        if !active {
            if before != range.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                let inherited = if stack.is_empty() {
                    Arc::new(
                        story_namespace_scope_at(xml, before)?
                            .into_iter()
                            .filter(|(_, uri)| uri == oxoffice_docx_oxml::namespace::W_NS)
                            .map(|(prefix, _)| prefix)
                            .collect::<Vec<_>>(),
                    )
                } else {
                    stack.last().unwrap().word_prefixes.clone()
                };
                let word_prefixes = story_word_prefixes_at(
                    &element,
                    &inherited,
                    namespace == StoryNamespace::Word,
                )?;
                let modeled = !stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, local, &element, empty, xml, before, after,
                    )?;
                if !stack.is_empty()
                    && modeled
                    && ((namespace == StoryNamespace::Word && local == b"txbxContent")
                        || namespace == StoryNamespace::MarkupCompatibility)
                {
                    return Err(Error::Other(
                        "nested alternate/text-box correspondence is unsupported".into(),
                    ));
                }
                let structural = modeled
                    && namespace == StoryNamespace::Word
                    && matches!(
                        local,
                        b"txbxContent" | b"p" | b"tbl" | b"tr" | b"tc" | b"sdt" | b"sdtContent"
                    );
                let path = if structural {
                    let parent = paths.iter().rposition(Option::is_some);
                    let mut path = parent.and_then(|i| paths[i].clone()).unwrap_or_default();
                    if let Some(parent) = parent {
                        path.push(children[parent]);
                        children[parent] += 1;
                    }
                    let end = if empty {
                        after
                    } else {
                        story_element_end(xml, before)?
                    };
                    slots.push((path.clone(), before..end));
                    signature.push(format!("+{}", String::from_utf8_lossy(local)));
                    if local == b"tbl" {
                        let scope = story_namespace_scope_at(xml, before)?;
                        let parsed = if let Some(operation) = operation.as_deref_mut() {
                            operation.document_block(before..end, &scope)?
                        } else {
                            let closed =
                                close_content_fragment_namespaces(&xml[before..end], &scope)?;
                            let mut wrapped =
                                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#)
                                    .into_bytes();
                            wrapped.extend_from_slice(&closed);
                            wrapped.extend_from_slice(b"</w:body></w:document>");
                            CT_Document::from_xml(&wrapped)?
                        };
                        let Some(BodyContent::Table(table)) = parsed.body.content.first() else {
                            return Err(Error::Other(
                                "paired table is not a qualified table".into(),
                            ));
                        };
                        validate_table_topology(table)?;
                        let columns = table.grid.as_ref().map_or(0, |grid| grid.columns.len());
                        signature.push(format!("grid:{columns}"));
                        for row in &table.rows {
                            let ranges = crate::table::row_cell_ranges(row, columns)?;
                            signature.push(format!("row:{:?}", ranges));
                            signature.push(format!(
                                "omissions:{:?}:{:?}",
                                row.properties.as_ref().and_then(|p| p.grid_before),
                                row.properties.as_ref().and_then(|p| p.grid_after)
                            ));
                            for cell in &row.cells {
                                signature.push(format!(
                                    "merge:{:?}:{:?}",
                                    cell.properties.as_ref().and_then(|p| p.v_merge),
                                    cell.properties.as_ref().and_then(|p| p.h_merge.as_ref())
                                ));
                            }
                        }
                    }
                    if local == b"p" {
                        let scope = story_namespace_scope_at(xml, before)?;
                        let paragraph = if let Some(operation) = operation.as_deref_mut() {
                            operation.paragraph(before..end, &scope)?
                        } else {
                            let closed =
                                close_content_fragment_namespaces(&xml[before..end], &scope)?;
                            CT_P::from_xml_fragment(&closed)?
                        };
                        signature.push(paragraph.accepted_text());
                    }
                    Some(path)
                } else {
                    None
                };
                if modeled
                    && namespace == StoryNamespace::Word
                    && matches!(
                        local,
                        b"gridCol"
                            | b"gridBefore"
                            | b"gridAfter"
                            | b"gridSpan"
                            | b"vMerge"
                            | b"hMerge"
                    )
                {
                    let mut fact = String::from_utf8_lossy(local).into_owned();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (attribute_namespace, attribute_local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        if word_element(&attribute_namespace) && attribute_local.as_ref() == b"val"
                        {
                            fact.push(':');
                            fact.push_str(
                                &attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?,
                            );
                        }
                    }
                    signature.push(fact);
                }
                if empty {
                    if structural {
                        signature.push("-".into());
                    }
                } else {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                    paths.push(path);
                    children.push(0);
                }
            }
            Event::End(_) => {
                if paths.pop().flatten().is_some() {
                    signature.push("-".into());
                }
                children.pop();
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        if after == range.end {
            return Ok(PairedTextBoxProjection { signature, slots });
        }
        buffer.clear();
    }
    Err(Error::Other("paired text-box source is incomplete".into()))
}

struct PairedReplacement<'a> {
    span: &'a Range<usize>,
    paragraph: Option<usize>,
    placeholder: &'a str,
    replacement: &'a str,
    literal_input: &'a Cell<bool>,
    count: usize,
}

// The same modeled event admission used by the existing paired projection.
// This finite projection adds only direct table children, not another owner map.
#[allow(clippy::type_complexity)] // One direct-child range plus its row flag, keyed by table.
fn template_direct_table_children(
    xml: &[u8],
) -> Result<BTreeMap<usize, Vec<(Range<usize>, bool)>>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut buffer = Vec::new();
    let mut tables = BTreeMap::<usize, Vec<(Range<usize>, bool)>>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let after = reader.buffer_position() as usize;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes =
                    story_word_prefixes_at(&element, inherited, namespace == StoryNamespace::Word)?;
                let modeled = !stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, local, &element, empty, xml, before, after,
                    )?;
                if modeled && namespace == StoryNamespace::Word && local == b"tbl" {
                    tables.entry(before).or_default();
                }
                if let Some(parent) = stack.last().filter(|parent| {
                    !parent.opaque
                        && parent.namespace == StoryNamespace::Word
                        && parent.local_name.as_ref() == b"tbl"
                }) && !(namespace == StoryNamespace::Word
                    && matches!(local, b"tblPr" | b"tblGrid"))
                {
                    let end = if empty {
                        after
                    } else {
                        story_element_end(xml, before)?
                    };
                    tables.entry(parent.full_start).or_default().push((
                        before..end,
                        modeled && namespace == StoryNamespace::Word && local == b"tr",
                    ));
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(tables)
}

fn prepare_template_source_programs(
    source: &StorySource<'_>,
) -> Result<Vec<crate::template::TemplateProgram>> {
    prepare_template_source_programs_in(source, None)
}

fn copy_template_source_range(
    source: &StorySource<'_>,
    range: Range<usize>,
    scoped: &mut Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Vec<u8>> {
    if let Some((operation, parent)) = scoped {
        let copied = operation.compose_sources(&[(Some(*parent), range, &[])])?;
        Ok(operation.finish_source(copied)?)
    } else {
        Ok(source.xml[range].to_vec())
    }
}

fn retain_template_source_range(
    range: Range<usize>,
    scoped: &mut Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>> {
    if let Some((operation, parent)) = scoped {
        Ok(Some(Arc::new(operation.compose_sources(&[(
            Some(*parent),
            range,
            &[],
        )])?)))
    } else {
        Ok(None)
    }
}

fn prepare_template_source_programs_in(
    source: &StorySource<'_>,
    mut scoped: Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Vec<crate::template::TemplateProgram>> {
    use crate::template::{TemplateItemKind, TemplateProgram, TemplateProgramItem};
    let mut roots = Vec::new();
    let owners = scan_story_owners(&source.xml, source.root_kind)?;
    let scopes =
        story_namespace_scopes_at(&source.xml, owners.iter().map(|owner| owner.full.start))?;
    let mut inventories = Vec::new();
    let mut template_fields = Vec::new();
    let mut qualified = Vec::new();
    for owner in &owners {
        let scope = scopes
            .get(&owner.full.start)
            .ok_or_else(|| Error::Other("template owner scope missing".into()))?;
        let mut ranges = Vec::new();
        inventories.push(scan_story_items_with_scope(
            &source.xml,
            owner,
            scope,
            Some(&mut ranges),
        )?);
        ranges.extend(
            inventories
                .last()
                .unwrap()
                .iter()
                .filter(|item| item.kind == StoryItemKind::Field)
                .map(|item| item.full.clone()),
        );
        ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        let mut merged = Vec::<Range<usize>>::new();
        for range in ranges {
            if let Some(last) = merged.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        template_fields.push(merged);
        qualified.push((
            StoryId {
                kind: owner.kind,
                part_name: source.part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            },
            owner.clone(),
        ));
    }
    let paragraphs = story_range_paragraph_spans(&source.xml, &qualified)?;
    let paragraph_locations = paragraphs
        .into_iter()
        .flatten()
        .map(|(location, range)| (range.start, location))
        .collect::<HashMap<_, _>>();
    let mut programs = Vec::<TemplateProgram>::new();
    let mut suppressed_aliases = BTreeMap::<usize, usize>::new();
    let mut active = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        if suppressed_aliases
            .range(..=owner.full.start)
            .next_back()
            .is_some_and(|(_, end)| owner.full.end <= *end)
        {
            continue;
        }
        let paired = if owner.kind == StoryKind::TextBox {
            paired_scoped_ranges(&source.xml, owner, &owner.full, None)?
        } else {
            None
        };
        let mut ranges = vec![owner.full.clone()];
        for range in paired
            .into_iter()
            .flatten()
            .filter(|range| *range != owner.full)
        {
            suppressed_aliases.insert(range.start, range.end);
            ranges.push(range);
        }
        active.push(index);
        programs.push(TemplateProgram {
            story: qualified[index].0.clone(),
            part: source.part_name.clone(),
            ranges,
            raw: copy_template_source_range(source, owner.full.clone(), &mut scoped)?,
            source: retain_template_source_range(owner.full.clone(), &mut scoped)?,
            scope: scopes[&owner.full.start].clone(),
            required_paragraph: owner.kind == StoryKind::TableCell,
            table_rows: false,
            items: Vec::new(),
        });
    }
    let table_children = template_direct_table_children(&source.xml)?;
    let mut item_scopes = story_namespace_scopes_at(
        &source.xml,
        inventories
            .iter()
            .flatten()
            .map(|item| item.full.start)
            .chain(
                table_children
                    .values()
                    .flatten()
                    .map(|(range, _)| range.start),
            ),
    )?;
    // Only the existing admitted block-control slots introduce another
    // sibling grammar container. Inline wrappers retain mapper semantics.
    for &index in &active {
        for item in &inventories[index] {
            if item.kind == StoryItemKind::ContentControl
                && item.sdt_context == Some(StorySdtContext::Block)
            {
                programs.push(TemplateProgram {
                    story: qualified[index].0.clone(),
                    part: source.part_name.clone(),
                    ranges: vec![item.full.clone()],
                    raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                    source: retain_template_source_range(item.full.clone(), &mut scoped)?,
                    scope: item_scopes[&item.full.start].clone(),
                    required_paragraph: false,
                    table_rows: false,
                    items: Vec::new(),
                });
            }
        }
    }
    let mut prepared = Vec::new();
    for &owner_index in &active {
        let story = &qualified[owner_index].0;
        // A block control is its own grammar container. Its direct paragraphs
        // are not story items of the enclosing owner, so the container would
        // otherwise publish with no scalar occurrences.
        let mut items = inventories[owner_index].clone();
        let mut extra_starts = Vec::new();
        for item in &inventories[owner_index] {
            if item.kind != StoryItemKind::ContentControl
                || item.sdt_context != Some(StorySdtContext::Block)
            {
                continue;
            }
            let scope = item_scopes
                .get(&item.full.start)
                .ok_or_else(|| Error::Other("template block control scope missing".to_owned()))?;
            for paragraph in scan_story_control_paragraphs_with_scope(&source.xml, item, scope)? {
                if items.iter().any(|existing| existing.full == paragraph) {
                    continue;
                }
                extra_starts.push(paragraph.start);
                items.push(StoryItemSpan {
                    kind: StoryItemKind::Paragraph,
                    full: paragraph.clone(),
                    scan: paragraph,
                    direct_owner_child: false,
                    complex_field: false,
                    complex_ancestors: Vec::new(),
                    sdt_context: None,
                });
            }
        }
        if !extra_starts.is_empty() {
            item_scopes.extend(story_namespace_scopes_at(&source.xml, extra_starts)?);
        }
        items.sort_by_key(|item| (item.full.start, item.full.end));
        let fields = &template_fields[owner_index];
        let mut blocked_until = 0;
        for (index, item) in items.iter().enumerate() {
            if item.full.start < blocked_until {
                continue;
            }
            if !matches!(
                item.kind,
                StoryItemKind::Paragraph
                    | StoryItemKind::Table
                    | StoryItemKind::ContentControl
                    | StoryItemKind::PreservedNode
            ) {
                continue;
            }
            if item.kind == StoryItemKind::ContentControl
                && item.sdt_context != Some(StorySdtContext::Block)
            {
                continue;
            }
            if matches!(item.kind, StoryItemKind::Paragraph | StoryItemKind::Table) {
                blocked_until = item.full.end;
            }
            let location = paragraph_locations
                .get(&item.full.start)
                .cloned()
                .unwrap_or_else(|| ContentLocation::new(story.clone(), item.kind, vec![index]));
            let field_owned = item.complex_field
                || !item.complex_ancestors.is_empty()
                || fields
                    .get(fields.partition_point(|range| range.end <= item.full.start))
                    .is_some_and(|range| range.start < item.full.end);
            if item.kind == StoryItemKind::Table {
                let rows = table_children.get(&item.full.start).ok_or_else(|| {
                    Error::Other("template table lacks its admitted physical projection".into())
                })?;
                let closed = if let Some((operation, parent)) = &mut scoped {
                    let selected =
                        operation.compose_sources(&[(Some(*parent), item.full.clone(), &[])])?;
                    let closed = operation
                        .close_source_namespaces(selected, &item_scopes[&item.full.start])?;
                    operation.finish_source(closed)?
                } else {
                    close_content_fragment_namespaces(
                        &source.xml[item.full.clone()],
                        &item_scopes[&item.full.start].clone(),
                    )?
                };
                let table = CT_Tbl::from_xml_fragment_checked(&closed)?;
                if rows.iter().filter(|(_, row)| *row).count() != table.rows.len() {
                    return Err(Error::Other(
                        "template direct row source does not match the existing table model".into(),
                    ));
                }
                let mut row_index = 0;
                let mut row_items = Vec::new();
                for (range, is_row) in rows {
                    let marker_unsafe = !*is_row
                        || table.extra_xml.iter().any(|(at, _)| *at == row_index)
                        || table
                            .content_controls
                            .iter()
                            .any(|(at, _, _)| *at == row_index);
                    row_items.push(TemplateProgramItem {
                        location: location.clone(),
                        range: range.clone(),
                        kind: if *is_row {
                            TemplateItemKind::TableRow
                        } else {
                            TemplateItemKind::Source(StoryItemKind::PreservedNode)
                        },
                        field_owned,
                        marker_unsafe,
                        raw: copy_template_source_range(source, range.clone(), &mut scoped)?,
                        scope: item_scopes[&range.start].clone(),
                        children: Vec::new(),
                    });
                    row_index += usize::from(*is_row);
                }
                programs.push(TemplateProgram {
                    story: story.clone(),
                    part: source.part_name.clone(),
                    ranges: vec![item.full.clone()],
                    raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                    source: retain_template_source_range(item.full.clone(), &mut scoped)?,
                    scope: item_scopes[&item.full.start].clone(),
                    required_paragraph: false,
                    table_rows: true,
                    items: Vec::new(),
                });
                prepared.extend(row_items);
            }
            prepared.push(TemplateProgramItem {
                location,
                range: item.full.clone(),
                kind: TemplateItemKind::Source(item.kind),
                field_owned,
                marker_unsafe: false,
                raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                scope: item_scopes[&item.full.start].clone(),
                children: Vec::new(),
            });
        }
    }
    // The existing physical spans are laminar. Two ordered containment
    // sweeps join items to containers and containers to enclosing items,
    // without searching every owner for every paragraph.
    let mut order = (0..programs.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| {
        (
            programs[*index].ranges[0].start,
            std::cmp::Reverse(programs[*index].ranges[0].end),
        )
    });
    prepared.sort_by_key(|item| (item.range.start, std::cmp::Reverse(item.range.end)));
    let mut stack: Vec<usize> = Vec::new();
    let mut cursor = 0;
    let mut item_slots = Vec::new();
    for item in prepared {
        while let Some(&index) = order
            .get(cursor)
            .filter(|index| programs[**index].ranges[0].start <= item.range.start)
        {
            let span = &programs[index].ranges[0];
            while stack.last().is_some_and(|parent| {
                programs[*parent].ranges[0].end < span.end
                    || programs[*parent].ranges[0].end <= span.start
            }) {
                stack.pop();
            }
            stack.push(index);
            cursor += 1;
        }
        while stack.last().is_some_and(|index| {
            programs[*index].ranges[0].end < item.range.end
                || programs[*index].ranges[0].end <= item.range.start
        }) {
            stack.pop();
        }
        let container = stack
            .iter()
            .rev()
            .find(|index| programs[**index].ranges[0] != item.range)
            .copied()
            .ok_or_else(|| {
                Error::Other("template item has no qualified original container".into())
            })?;
        let slot = programs[container].items.len();
        item_slots.push((item.range.clone(), container, slot));
        programs[container].items.push(item);
    }
    item_slots.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut parents = vec![None; programs.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut cursor = 0;
    for &index in &order {
        let span = &programs[index].ranges[0];
        while let Some((range, _, _)) = item_slots
            .get(cursor)
            .filter(|(range, _, _)| range.start <= span.start)
        {
            while stack.last().is_some_and(|parent| {
                item_slots[*parent].0.end < range.end || item_slots[*parent].0.end <= range.start
            }) {
                stack.pop();
            }
            stack.push(cursor);
            cursor += 1;
        }
        while stack.last().is_some_and(|item| {
            item_slots[*item].0.end < span.end || item_slots[*item].0.end <= span.start
        }) {
            stack.pop();
        }
        if let Some(&item) = stack.last() {
            parents[index] = Some((item_slots[item].1, item_slots[item].2));
        }
    }
    order.sort_by_key(|index| programs[*index].ranges[0].len());
    let mut programs = programs.into_iter().map(Some).collect::<Vec<_>>();
    let mut source_roots = Vec::new();
    for index in order {
        let mut program = programs[index]
            .take()
            .ok_or_else(|| Error::Other("template grammar program was consumed twice".into()))?;
        for item in &mut program.items {
            item.children.sort_by_key(|child| child.ranges[0].start);
        }
        if let Some((parent, slot)) = parents[index] {
            programs[parent]
                .as_mut()
                .ok_or_else(|| {
                    Error::Other("template parent was consumed before its child".into())
                })?
                .items[slot]
                .children
                .push(program);
        } else {
            source_roots.push(program);
        }
    }
    source_roots.sort_by_key(|program| program.ranges[0].start);
    roots.extend(source_roots);
    Ok(roots)
}

fn paired_scoped_ranges(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    span: &Range<usize>,
    mut operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
) -> Result<Option<Vec<Range<usize>>>> {
    let copies = scoped_alternate_text_box_copies(xml)?;
    let selected = copies
        .iter()
        .filter(|copy| copy.range.start <= owner.full.start && owner.full.end <= copy.range.end)
        .max_by_key(|copy| copy.alternate);
    let Some(selected) = selected else {
        return Ok(None);
    };
    let mut branches = BTreeMap::<usize, Vec<&PairedTextBoxCopy>>::new();
    for copy in &copies {
        if copy.alternate == selected.alternate {
            branches.entry(copy.branch).or_default().push(copy);
        }
    }
    if branches.len() < 2 {
        return Ok(None);
    }
    let selected_branch = &branches[&selected.branch];
    let selected_slot = selected_branch
        .iter()
        .position(|copy| copy.range == selected.range)
        .ok_or_else(|| Error::Other("paired selected box disappeared".into()))?;
    let baseline = selected_branch
        .iter()
        .map(|copy| paired_text_box_projection(xml, copy.range.clone(), operation.as_deref_mut()))
        .collect::<Result<Vec<_>>>()?;
    let selected_signature = &baseline[selected_slot].signature;
    if selected_branch.len() > 1
        && baseline
            .iter()
            .filter(|projection| &projection.signature == selected_signature)
            .count()
            != 1
    {
        return Err(Error::Other(
            "indistinguishable sibling text boxes cannot be paired".into(),
        ));
    }
    let path = baseline[selected_slot]
        .slots
        .iter()
        .find(|(_, range)| range == span)
        .map(|(path, _)| path.clone())
        .ok_or_else(|| Error::Other("selected paired content has no structural slot".into()))?;
    let mut ranges = Vec::new();
    for branch in branches.values() {
        if branch.len() != baseline.len() {
            return Err(Error::Other(
                "paired text-box branches have unequal cardinality".into(),
            ));
        }
        let projections = branch
            .iter()
            .map(|copy| {
                paired_text_box_projection(xml, copy.range.clone(), operation.as_deref_mut())
            })
            .collect::<Result<Vec<_>>>()?;
        if projections
            .iter()
            .zip(&baseline)
            .any(|(actual, expected)| actual.signature != expected.signature)
            || (branch.len() > 1
                && projections
                    .iter()
                    .filter(|p| &p.signature == selected_signature)
                    .count()
                    != 1)
        {
            return Err(Error::Other(
                "paired alternate text-box topology or accepted text is asymmetric".into(),
            ));
        }
        let range = projections[selected_slot]
            .slots
            .iter()
            .find(|(actual_path, _)| actual_path == &path)
            .map(|(_, range)| range.clone())
            .ok_or_else(|| Error::Other("paired content structural slot is absent".into()))?;
        ranges.push(range);
    }
    ranges.sort_by_key(|range| range.start);
    if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other("paired replacement sources overlap".into()));
    }
    Ok(Some(ranges))
}

fn paired_scoped_edits(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    replacement: &PairedReplacement<'_>,
    selected_raw: &mut Vec<u8>,
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
) -> Result<Option<Vec<PairedTextEdit>>> {
    let PairedReplacement {
        span,
        paragraph,
        placeholder,
        replacement,
        literal_input,
        count,
    } = *replacement;
    let Some(ranges) = paired_scoped_ranges(xml, owner, span, Some(operation))? else {
        return Ok(None);
    };
    let mut edits = Vec::new();
    for range in ranges {
        if range == *span {
            edits.push(PairedTextEdit {
                range,
                raw: std::mem::take(selected_raw),
            });
            continue;
        }
        let scope = story_namespace_scope_at(xml, range.start)?;
        let counterpart = operation.replace_block_with_literal_input(
            range.clone(),
            &scope,
            paragraph,
            placeholder,
            replacement,
            literal_input,
        );
        let (raw, actual_count) = counterpart?;
        if actual_count != count {
            return Err(Error::Other(
                "paired replacement physical counts are unequal".into(),
            ));
        }
        edits.push(PairedTextEdit { range, raw });
    }
    Ok(Some(edits))
}

fn opaque_story_event(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    element: &BytesStart<'_>,
    empty: bool,
    xml: &[u8],
    before: usize,
    after: usize,
) -> Result<bool> {
    Ok(stack.last().is_some_and(|frame| frame.opaque)
        || !modeled_story_event(
            stack, namespace, local_name, element, empty, xml, before, after,
        )?)
}

fn direct_story_item_kind(is_word: bool, local_name: &[u8]) -> StoryItemKind {
    if !is_word {
        return StoryItemKind::PreservedNode;
    }
    match local_name {
        b"p" => StoryItemKind::Paragraph,
        b"tbl" => StoryItemKind::Table,
        b"sdt" => StoryItemKind::ContentControl,
        _ => StoryItemKind::PreservedNode,
    }
}

fn owner_kind(local_name: &[u8]) -> Option<StoryKind> {
    match local_name {
        b"body" => Some(StoryKind::Body),
        b"tc" => Some(StoryKind::TableCell),
        b"hdr" => Some(StoryKind::Header),
        b"ftr" => Some(StoryKind::Footer),
        b"footnote" => Some(StoryKind::Footnote),
        b"endnote" => Some(StoryKind::Endnote),
        b"comment" => Some(StoryKind::Comment),
        b"txbxContent" => Some(StoryKind::TextBox),
        _ => None,
    }
}

fn editable_note_owner(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<bool> {
    let mut id = None;
    let mut separator = false;
    let mut normal_type = true;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story owner attribute scan failed: {error}")))?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) {
            continue;
        }
        match local_name.as_ref() {
            b"type" => {
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        Error::Other(format!("story owner type decode failed: {error}"))
                    })?;
                separator = matches!(
                    value.as_ref(),
                    "separator" | "continuationSeparator" | "continuationNotice"
                );
                normal_type = value == "normal";
            }
            b"id" => {
                id = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        Error::Other(format!("story owner id decode failed: {error}"))
                    })?
                    .parse::<i32>()
                    .ok();
            }
            _ => {}
        }
    }
    Ok(!separator && id.is_some_and(|id| normal_type || id > 0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComplexFieldMarker {
    Begin,
    Separate,
    End,
}

struct StoryFieldRun {
    range: Range<usize>,
    parent_start: Option<usize>,
}

struct ComplexFieldScan {
    marker_start: usize,
    run_start: usize,
    run_parent_start: Option<usize>,
    ancestors: Vec<usize>,
    separators: usize,
    instruction: String,
    leading_nested: bool,
    valid: bool,
}

impl ComplexFieldScan {
    fn accepted(&self) -> bool {
        self.valid
            && self.separators <= 1
            && !self.leading_nested
            && field_instruction_has_name(&self.instruction)
    }
}

fn complex_field_marker(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<ComplexFieldMarker>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("field marker attribute scan failed: {error}"))
        })?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) || local_name.as_ref() != b"fldCharType" {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Other(format!("field marker type decode failed: {error}")))?;
        return Ok(match value.as_ref() {
            "begin" => Some(ComplexFieldMarker::Begin),
            "separate" => Some(ComplexFieldMarker::Separate),
            "end" => Some(ComplexFieldMarker::End),
            _ => None,
        });
    }
    Ok(None)
}

fn story_enclosing_run_source(
    xml: &[u8],
    stack: &[XmlElementFrame],
) -> Result<Option<StoryFieldRun>> {
    let Some((index, frame)) = stack.iter().enumerate().rev().find(|(_, frame)| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && frame.local_name.as_ref() == b"r"
    }) else {
        return Ok(None);
    };
    Ok(Some(StoryFieldRun {
        range: frame.full_start..story_element_end(xml, frame.full_start)?,
        parent_start: index.checked_sub(1).map(|parent| stack[parent].full_start),
    }))
}

fn complex_field_scan_context(stack: &[XmlElementFrame]) -> bool {
    stack.iter().any(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && frame.local_name.as_ref() == b"r"
    }) && !stack.iter().any(|frame| {
        frame.namespace == StoryNamespace::Word && frame.local_name.as_ref() == b"fldSimple"
    })
}

fn scan_complex_field_marker(
    fields: &mut Vec<ComplexFieldScan>,
    items: &mut Vec<StoryItemSpan>,
    marker: Option<ComplexFieldMarker>,
    before: usize,
    after: usize,
    run: StoryFieldRun,
) {
    match marker {
        Some(ComplexFieldMarker::Begin) => {
            let ancestors = fields.iter().map(|field| field.marker_start).collect();
            fields.push(ComplexFieldScan {
                marker_start: before,
                run_start: run.range.start,
                run_parent_start: run.parent_start,
                ancestors,
                separators: 0,
                instruction: String::new(),
                leading_nested: false,
                valid: true,
            });
        }
        Some(ComplexFieldMarker::Separate) => {
            if let Some(field) = fields.last_mut() {
                field.separators += 1;
                if field.separators > 1 {
                    field.valid = false;
                }
            }
        }
        Some(ComplexFieldMarker::End) => {
            let Some(field) = fields.pop() else {
                return;
            };
            let accepted = field.accepted();
            if let Some(parent) = fields.last_mut() {
                if !accepted {
                    parent.valid = false;
                } else if parent.separators == 0 && !field_instruction_has_name(&parent.instruction)
                {
                    parent.leading_nested = true;
                }
            }
            // A first-to-last run excerpt is a balanced sibling sequence only
            // under the same physical parent. This is typed eligibility, not
            // intrinsic field validity or a reason to poison an outer field.
            if accepted && field.run_parent_start == run.parent_start {
                items.push(StoryItemSpan {
                    kind: StoryItemKind::Field,
                    full: field.marker_start..after,
                    scan: field.run_start..run.range.end,
                    direct_owner_child: false,
                    complex_field: true,
                    complex_ancestors: field.ancestors,
                    sdt_context: None,
                });
            }
        }
        None => {}
    }
}

fn push_complex_field_instruction(
    fields: &mut [ComplexFieldScan],
    stack: &[XmlElementFrame],
    text: &str,
) {
    let modeled_instruction = stack.last().is_some_and(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && frame.local_name.as_ref() == b"instrText"
    });
    if modeled_instruction
        && let Some(field) = fields.last_mut()
        && field.separators == 0
    {
        field.instruction.push_str(text);
    }
}

fn update_complex_field_results(results: &mut Vec<bool>, marker: Option<ComplexFieldMarker>) {
    match marker {
        Some(ComplexFieldMarker::Begin) => results.push(false),
        Some(ComplexFieldMarker::Separate) => {
            if let Some(result) = results.last_mut() {
                *result = true;
            }
        }
        Some(ComplexFieldMarker::End) => {
            results.pop();
        }
        None => {}
    }
}

/// Return whether text at this point is visible: no open complex field is
/// still in its instruction part, and a field item is inside its own field.
fn complex_field_text_visible(item: &StoryItemSpan, results: &[bool]) -> bool {
    (!item.complex_field || !results.is_empty()) && results.iter().all(|result| *result)
}

/// The fields open where a piece of story text was read, by index into
/// [`StoryFieldTracker`], each with whether it had reached its result.
type StoryFieldContext = Vec<(usize, bool)>;

/// The complex fields an item other than a field item opens, in source order.
///
/// The typed model reads a field as its cached result only when the field
/// closes within the paragraph. An open field, such as the first paragraph of
/// a table of contents, reads as plain runs. Whether a field hides text is
/// only known at its end marker, so text records the [`Self::context`] it was
/// read in, and [`Self::hides`] decides once the item is scanned.
#[derive(Default)]
struct StoryFieldTracker {
    /// The fields open now, by index, each with whether it has separated.
    open: StoryFieldContext,
    closed: Vec<bool>,
}

impl StoryFieldTracker {
    fn mark(&mut self, marker: Option<ComplexFieldMarker>) {
        match marker {
            Some(ComplexFieldMarker::Begin) => {
                self.open.push((self.closed.len(), false));
                self.closed.push(false);
            }
            Some(ComplexFieldMarker::Separate) => {
                if let Some((_, separated)) = self.open.last_mut() {
                    *separated = true;
                }
            }
            Some(ComplexFieldMarker::End) => {
                if let Some((field, _)) = self.open.pop() {
                    self.closed[field] = true;
                }
            }
            None => {}
        }
    }

    /// The open fields, each with whether it has reached its result.
    fn context(&self) -> StoryFieldContext {
        self.open.clone()
    }

    /// Whether a closed field was still in its instruction part.
    fn hides(&self, context: &[(usize, bool)]) -> bool {
        context
            .iter()
            .any(|(field, separated)| self.closed[*field] && !separated)
    }

    /// Whether the text lies in the result of a closed field.
    fn in_result(&self, context: &[(usize, bool)]) -> bool {
        context.iter().any(|(field, _)| self.closed[*field])
    }
}

/// Return whether a field character at `position` opens, separates or closes
/// a field that hides text of this item. A field item tracks the markers of
/// its own field. Any other item tracks every accepted marker of its own
/// story, so a field's instruction part stays out of its text as in
/// `Paragraph::text`.
fn story_field_marker_tracked(
    item: &StoryItemSpan,
    position: usize,
    nested_owner_depth: usize,
    stack: &[XmlElementFrame],
) -> bool {
    if item.complex_field {
        item.full.start <= position && position < item.full.end
    } else {
        nested_owner_depth == 0 && accepted_story_text_visible(stack)
    }
}

/// One piece of story text, kept with the fields open where it was read.
enum StoryTextPiece {
    Text(String),
    /// A tab or break, as read outside and inside a field result.
    Character(Option<char>, Option<char>),
}

/// The text a visible run `w:br` or `w:cr` adds, as the typed model reads it.
/// Direct runs and flattened simple fields read breaks as line feeds.
/// Inside a closed complex field's cached result, page and column breaks
/// read as form feeds and vertical tabs. CR remains a line feed in both views.
fn story_break_text(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    in_field_result: bool,
) -> Result<Option<char>> {
    if element.local_name().as_ref() == b"cr" {
        return Ok(Some('\n'));
    }
    if !in_field_result {
        return Ok(Some('\n'));
    }
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("break attribute scan failed: {error}")))?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) || local_name.as_ref() != b"type" {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Other(format!("break type decode failed: {error}")))?;
        return Ok(Some(match value.as_ref() {
            "page" => '\u{000c}',
            "column" => '\u{000b}',
            _ => '\n',
        }));
    }
    Ok(Some('\n'))
}

fn accepted_story_text_visible(stack: &[XmlElementFrame]) -> bool {
    !stack.iter().any(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && matches!(frame.local_name.as_ref(), b"del" | b"moveFrom")
    })
}

pub(crate) fn scan_story_owners(xml: &[u8], root_kind: StoryKind) -> Result<Vec<StoryOwnerSpan>> {
    scan_story_owners_with_special(xml, root_kind, false)
}

// Read-only physical paragraph membership for the existing custody convention.
// These ranges belong only to this input, and confer no edit authority.
pub(crate) fn admitted_story_paragraph_spans(
    xml: &[u8],
    root_kind: StoryKind,
) -> Result<Vec<Range<usize>>> {
    let owners = scan_story_owners_with_special(xml, root_kind, true)?
        .into_iter()
        .map(|owner| {
            let story = StoryId {
                kind: owner.kind,
                part_name: String::new(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            };
            (story, owner)
        })
        .collect::<Vec<_>>();
    let mut paragraphs = story_range_paragraph_spans(xml, &owners)?
        .into_iter()
        .flatten()
        .map(|(_, span)| span)
        .collect::<Vec<_>>();
    paragraphs.sort_by_key(|span| (span.start, span.end));
    paragraphs.dedup();
    Ok(paragraphs)
}

pub(crate) fn scan_story_owners_with_special(
    xml: &[u8],
    root_kind: StoryKind,
    include_special: bool,
) -> Result<Vec<StoryOwnerSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut owners = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story XML scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let is_word = is_word_namespace;
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let sdt_context = story_element_sdt_context(&stack, namespace_kind, &local_name);
                let direct_owner_child =
                    stack.last().is_some_and(|frame| frame.owner_kind.is_some());
                let mut opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    &local_name,
                    &element,
                    false,
                    xml,
                    before,
                    after,
                )? || (direct_owner_child
                    && direct_story_item_kind(is_word, &local_name)
                        == StoryItemKind::PreservedNode);
                let mut candidate = (!opaque && is_word)
                    .then(|| owner_kind(&local_name))
                    .flatten();
                if matches!(candidate, Some(StoryKind::Footnote | StoryKind::Endnote))
                    && !include_special
                    && !editable_note_owner(&reader, &element)?
                {
                    candidate = None;
                    opaque = true;
                }
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word,
                    full_start: before,
                    owner_kind: candidate.filter(|kind| {
                        *kind == root_kind
                            || matches!(kind, StoryKind::TableCell | StoryKind::TextBox)
                    }),
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context,
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let direct_owner_child =
                    stack.last().is_some_and(|frame| frame.owner_kind.is_some());
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    xml,
                    before,
                    after,
                )? || (direct_owner_child
                    && direct_story_item_kind(is_word_namespace, local_name.as_ref())
                        == StoryItemKind::PreservedNode);
                if is_word_namespace
                    && !opaque
                    && let Some(kind) = owner_kind(local_name.as_ref()).filter(|kind| {
                        *kind == root_kind
                            || matches!(kind, StoryKind::TableCell | StoryKind::TextBox)
                    })
                    && (!matches!(kind, StoryKind::Footnote | StoryKind::Endnote)
                        || include_special
                        || editable_note_owner(&reader, &element)?)
                {
                    owners.push(StoryOwnerSpan {
                        kind,
                        owner_index: 0,
                        full: before..after,
                        fingerprint: structural_fingerprint(&xml[before..after])?,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if let Some(kind) = frame.owner_kind {
                    owners.push(StoryOwnerSpan {
                        kind,
                        owner_index: 0,
                        full: frame.full_start..after,
                        fingerprint: structural_fingerprint(&xml[frame.full_start..after])?,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    owners.sort_by_key(|owner| owner.full.start);
    let mut counts = HashMap::new();
    for owner in &mut owners {
        let count = counts.entry(owner.kind).or_insert(0usize);
        owner.owner_index = *count;
        *count += 1;
    }
    Ok(owners)
}

/// Pair the non-cell story owners of `scanned`, the bytes revision resolution
/// scans, with the identities [`Document::stories`] reports from `reported`,
/// its own bytes for the same part.
///
/// Owners pair in order when both byte streams have the same owner kinds.
/// Otherwise the two disagree, for example because the typed serialization
/// dropped the namespace binding of a text box, so root owners pair by index
/// and text boxes by identical bytes. An owner without a partner is left
/// out, which folds its revisions into the owner around it.
fn paired_story_owners(
    part_name: &str,
    root_kind: StoryKind,
    scanned: &[u8],
    reported: Option<&[u8]>,
) -> Result<Vec<(Range<usize>, StoryId)>> {
    let non_cell_owners = |xml: &[u8]| -> Result<Vec<StoryOwnerSpan>> {
        Ok(scan_story_owners(xml, root_kind)?
            .into_iter()
            .filter(|owner| owner.kind != StoryKind::TableCell)
            .collect())
    };
    let scanned_owners = non_cell_owners(scanned)?;
    let reported_owners = match reported {
        Some(xml) if xml == scanned => scanned_owners.clone(),
        Some(xml) => non_cell_owners(xml)?,
        None => Vec::new(),
    };
    let story = |owner: &StoryOwnerSpan| StoryId {
        kind: owner.kind,
        part_name: part_name.to_owned(),
        owner_index: owner.owner_index,
        fingerprint: owner.fingerprint,
    };
    let same_kinds = scanned_owners.len() == reported_owners.len()
        && scanned_owners
            .iter()
            .zip(&reported_owners)
            .all(|(scanned, reported)| scanned.kind == reported.kind);
    if same_kinds {
        return Ok(scanned_owners
            .iter()
            .zip(&reported_owners)
            .map(|(scanned, reported)| (scanned.full.clone(), story(reported)))
            .collect());
    }
    let mut unpaired = reported_owners.iter().collect::<Vec<_>>();
    Ok(scanned_owners
        .iter()
        .filter_map(|scanned| {
            let position = unpaired.iter().position(|reported| {
                reported.kind == scanned.kind
                    && if scanned.kind == root_kind {
                        reported.owner_index == scanned.owner_index
                    } else {
                        reported.fingerprint == scanned.fingerprint
                    }
            })?;
            Some((scanned.full.clone(), story(unpaired.remove(position))))
        })
        .collect())
}

// Anchor capture uses trim_text(true). Apply precisely that parser projection
// to the original span, while retaining element names, attributes and order.
fn drawing_capture_key(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut writer = quick_xml::Writer::new(Vec::new());
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("drawing capture scan failed: {error}")))?;
        if matches!(event, Event::Eof) {
            break;
        }
        writer
            .write_event(event)
            .map_err(|error| Error::Other(error.to_string()))?;
        buffer.clear();
    }
    Ok(writer.into_inner())
}

/// Bind modeled selected anchors to actual physical story owners before shaping.
fn bind_text_box_source_owners(
    body: &mut oxoffice_docx_oxml::document::CT_Body,
    xml: &[u8],
    root_kind: StoryKind,
    scope: Range<usize>,
) -> Result<()> {
    let owners = scan_story_owners(xml, root_kind)?;
    let mut anchors = Vec::new();
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("text-box owner scan failed: {error}")))?;
        if let Event::Start(element) = event {
            if story_namespace(&namespace) == StoryNamespace::WordDrawing
                && element.local_name().as_ref() == b"anchor"
                && scope.contains(&before)
            {
                let end = story_element_end(xml, before)?;
                let contained = owners
                    .iter()
                    .filter(|owner| {
                        owner.kind == StoryKind::TextBox
                            && before <= owner.full.start
                            && owner.full.end <= end
                    })
                    .collect::<Vec<_>>();
                // Nested anchors own their own boxes. This anchor's direct box
                // is the first selected owner in its captured source span.
                anchors.push((
                    drawing_capture_key(&xml[before..end])?,
                    contained.first().map(|owner| owner.owner_index),
                    false,
                ));
            }
        } else if matches!(event, Event::Eof) {
            break;
        }
        buffer.clear();
    }
    prepare_physical_story_projection(body, &mut anchors)
}

// These are read/layout clones. No raw drawing scan or whole-body clone is
// needed to make already-admitted revision fields available to semantic readers.
fn prepare_semantic_paragraph_projection(value: &mut CT_P) -> Result<()> {
    for (_, _, revision) in &mut value.revisions {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for (_, _, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for run in &mut value.runs {
        prepare_semantic_run_projection(run)?;
    }
    Ok(())
}

fn prepare_semantic_run_projection(value: &mut CT_R) -> Result<()> {
    for drawing in value
        .content
        .iter_mut()
        .filter_map(|content| {
            if let RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .chain(value.alt_drawings.iter_mut())
    {
        let shape = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.shape.as_mut());
        if let Some(shape) = shape {
            if let Some(body) = &mut shape.text_body {
                prepare_semantic_body_projection(&mut body.content)?;
            } else {
                for paragraph in &mut shape.text {
                    prepare_semantic_paragraph_projection(paragraph)?;
                }
            }
        }
    }
    Ok(())
}

fn prepare_semantic_control_projection(value: &mut CT_Sdt) -> Result<()> {
    for (_, revision) in value.physical_revisions_mut() {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for child in &mut value.content {
        match child {
            SdtContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            SdtContent::Table(t) => prepare_semantic_table_projection(t)?,
            SdtContent::Row(r) => prepare_semantic_row_projection(r)?,
            SdtContent::Cell(c) => prepare_semantic_cell_projection(c)?,
            SdtContent::Run(r) => prepare_semantic_run_projection(r)?,
            SdtContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn prepare_semantic_table_projection(value: &mut CT_Tbl) -> Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for row in &mut value.rows {
        prepare_semantic_row_projection(row)?;
    }
    Ok(())
}

fn prepare_semantic_row_projection(value: &mut CT_Row) -> Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for cell in &mut value.cells {
        prepare_semantic_cell_projection(cell)?;
    }
    Ok(())
}

fn prepare_semantic_cell_projection(value: &mut CT_Tc) -> Result<()> {
    for content in &mut value.content {
        match content {
            CellContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            CellContent::Table(t) => prepare_semantic_table_projection(t)?,
            CellContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
        }
    }
    Ok(())
}

pub(crate) fn prepare_semantic_body_projection(values: &mut [BodyContent]) -> Result<()> {
    for value in values {
        match value {
            BodyContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            BodyContent::Table(t) => prepare_semantic_table_projection(t)?,
            BodyContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            BodyContent::RawXml(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn prepare_physical_story_projection(
    body: &mut oxoffice_docx_oxml::document::CT_Body,
    anchors: &mut [(Vec<u8>, Option<usize>, bool)],
) -> Result<()> {
    fn consume_raw(raw: &[u8], anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        let captured = drawing_capture_key(raw)?;
        let mut reader = quick_xml::Reader::from_reader(captured.as_slice());
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let event = reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(error.to_string()))?;
            if let Event::Start(element) = &event
                && element.local_name().as_ref() == b"anchor"
            {
                let end = story_element_end(&captured, before)?;
                let key = &captured[before..end];
                if let Some((_, _, used)) = anchors
                    .iter_mut()
                    .find(|(source, _, used)| !*used && source == key)
                {
                    *used = true;
                }
            }
            if matches!(event, Event::Eof) {
                break;
            }
            buffer.clear();
        }
        Ok(())
    }
    fn drawing(
        value: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        let Some(anchor) = value.anchor.as_mut() else {
            return Ok(());
        };
        let Some(shape) = anchor.shape.as_mut() else {
            return Ok(());
        };
        shape.source_text_box_owner = if let Some(raw) = &anchor.raw_xml {
            let key = drawing_capture_key(raw)?;
            anchors
                .iter_mut()
                .find(|(source, _, used)| !*used && *source == key)
                .and_then(|(_, owner, used)| {
                    *used = true;
                    *owner
                })
        } else {
            None
        };
        shape_content(shape, anchors)
    }
    fn shape_content(
        shape: &mut oxoffice_docx_oxml::drawing::CT_Shape,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        if let Some(body) = &mut shape.text_body {
            body_content(&mut body.content, anchors)?;
        } else {
            for p in &mut shape.text {
                paragraph(p, anchors)?;
            }
        }
        Ok(())
    }
    fn selected_raw(
        raw: &[u8],
        selected: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        let selected_key = selected
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.raw_xml.as_deref())
            .map(drawing_capture_key)
            .transpose()?;
        let captured = drawing_capture_key(raw)?;
        let mut reader = quick_xml::Reader::from_reader(captured.as_slice());
        let mut buffer = Vec::new();
        let mut bound = false;
        loop {
            let before = reader.buffer_position() as usize;
            let event = reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(error.to_string()))?;
            if let Event::Start(element) = &event
                && element.local_name().as_ref() == b"anchor"
            {
                let end = story_element_end(&captured, before)?;
                let key = &captured[before..end];
                let owner = anchors
                    .iter_mut()
                    .find(|(source, _, used)| !*used && source == key)
                    .and_then(|(_, owner, used)| {
                        *used = true;
                        *owner
                    });
                if !bound && owner.is_some() && selected_key.as_deref() == Some(key) {
                    if let Some(shape) = selected
                        .anchor
                        .as_mut()
                        .and_then(|anchor| anchor.shape.as_mut())
                    {
                        shape.source_text_box_owner = owner;
                        shape_content(shape, anchors)?;
                    }
                    bound = true;
                    let name = element.name().as_ref().to_vec();
                    reader
                        .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                        .map_err(|error| Error::Other(error.to_string()))?;
                }
            }
            if matches!(event, Event::Eof) {
                break;
            }
            buffer.clear();
        }
        if !bound
            && let Some(shape) = selected
                .anchor
                .as_mut()
                .and_then(|anchor| anchor.shape.as_mut())
        {
            shape.source_text_box_owner = None;
        }
        Ok(())
    }
    fn run(value: &mut CT_R, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        let properties = usize::from(value.properties.is_some());
        let mut selected_index = 0;
        let mut children = value
            .extra_xml
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                let encoded = value.extra_xml_positions.get(index).copied();
                if encoded.is_some_and(CT_R::raw_child_is_root_attributes) {
                    return None;
                }
                let selected = if encoded.is_some_and(CT_R::raw_child_has_alternate_content_drawing)
                {
                    let index = selected_index;
                    selected_index += 1;
                    Some(index)
                } else {
                    None
                };
                Some((
                    encoded.map_or(properties + value.content.len(), CT_R::raw_child_position),
                    0u8,
                    index,
                    selected,
                ))
            })
            .chain(
                value
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(index, content)| {
                        matches!(content, RunContent::Drawing(_)).then_some((
                            properties + index,
                            1u8,
                            index,
                            None,
                        ))
                    }),
            )
            .collect::<Vec<_>>();
        children.sort_unstable();
        for (_, kind, index, selected) in children {
            if kind == 1 {
                if let RunContent::Drawing(value) = &mut value.content[index] {
                    drawing(value, anchors)?;
                }
            } else if let Some(selected) =
                selected.and_then(|index| value.alt_drawings.get_mut(index))
            {
                selected_raw(&value.extra_xml[index], selected, anchors)?;
            } else {
                consume_raw(&value.extra_xml[index], anchors)?;
            }
        }
        Ok(())
    }
    fn paragraph(value: &mut CT_P, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        // Consume every physical revision owner before selecting accepted runs.
        // Projection metadata is installed in place, never serialized/reparsed.
        for boundary in 0..=value.runs.len() {
            let mut owners = value
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _, _))| *at == boundary)
                .map(|(index, (_, slot, _, _))| (1u8, *slot, 0u8, index))
                .chain(
                    value
                        .revisions
                        .iter()
                        .enumerate()
                        .filter(|(_, (at, _, _))| *at == boundary)
                        .map(|(index, (_, slot, _))| {
                            let (order, slot) = if let Some(link_index) =
                                oxoffice_docx_oxml::text::hyperlink_revision_index(*slot)
                            {
                                match value.hyperlinks.get(link_index) {
                                    Some(link) if link.preserved_raw_before.is_some() => {
                                        (1, link.preserved_raw_before.unwrap())
                                    }
                                    Some(link) if boundary == link.run_end => (0, 0),
                                    _ => (2, 0),
                                }
                            } else {
                                (1, *slot)
                            };
                            (order, slot, 1u8, index)
                        }),
                )
                .collect::<Vec<_>>();
            for (raw_slot, (index, (at, _))) in value
                .extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == boundary)
                .enumerate()
            {
                let projected = value
                    .revisions
                    .iter()
                    .any(|(position, slot, _)| position == at && *slot == raw_slot)
                    || value.hyperlinks.iter().any(|link| {
                        link.run_start == boundary && link.preserved_raw_before == Some(raw_slot)
                    });
                if !projected {
                    owners.push((1, raw_slot, 2, index));
                }
            }
            owners.sort_unstable();
            for (_, _, kind, index) in owners {
                if kind == 1 {
                    if let Some(p) = value.revisions[index].2.physical_content_paragraph_mut()? {
                        paragraph(p, anchors)?;
                    }
                } else if kind == 0 {
                    control(&mut value.content_controls[index].3, anchors)?;
                } else {
                    consume_raw(&value.extra_xml[index].1, anchors)?;
                }
            }
            if let Some(value) = value.runs.get_mut(boundary) {
                run(value, anchors)?;
            }
        }
        Ok(())
    }
    fn control(value: &mut CT_Sdt, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.content.len() {
            for (_, revision) in value
                .physical_revisions_mut()
                .iter_mut()
                .filter(|(at, _)| *at == boundary)
            {
                if let Some(p) = revision.physical_content_paragraph_mut()? {
                    paragraph(p, anchors)?;
                }
            }
            if let Some(content) = value.content.get_mut(boundary) {
                match content {
                    SdtContent::Paragraph(p) => paragraph(p, anchors)?,
                    SdtContent::Run(value) => run(value, anchors)?,
                    SdtContent::Table(value) => table(value, anchors)?,
                    SdtContent::Row(value) => row(value, anchors)?,
                    SdtContent::Cell(value) => cell(value, anchors)?,
                    SdtContent::ContentControl(value) => control(value, anchors)?,
                    SdtContent::RawXml(raw) => consume_raw(raw, anchors)?,
                }
            }
        }
        Ok(())
    }
    fn table(value: &mut CT_Tbl, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.rows.len() {
            for (_, _, value) in value
                .content_controls
                .iter_mut()
                .filter(|(at, _, _)| *at == boundary)
            {
                control(value, anchors)?;
            }
            if let Some(value) = value.rows.get_mut(boundary) {
                row(value, anchors)?;
            }
        }
        Ok(())
    }
    fn row(value: &mut CT_Row, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.cells.len() {
            for (_, _, value) in value
                .content_controls
                .iter_mut()
                .filter(|(at, _, _)| *at == boundary)
            {
                control(value, anchors)?;
            }
            if let Some(value) = value.cells.get_mut(boundary) {
                cell(value, anchors)?;
            }
        }
        Ok(())
    }
    fn cell(value: &mut CT_Tc, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for content in &mut value.content {
            match content {
                CellContent::Paragraph(value) => paragraph(value, anchors)?,
                CellContent::Table(value) => table(value, anchors)?,
                CellContent::ContentControl(value) => control(value, anchors)?,
            }
        }
        Ok(())
    }
    fn body_content(
        values: &mut [BodyContent],
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        for value in values {
            match value {
                BodyContent::Paragraph(value) => paragraph(value, anchors)?,
                BodyContent::Table(value) => table(value, anchors)?,
                BodyContent::ContentControl(value) => control(value, anchors)?,
                BodyContent::RawXml(raw) => consume_raw(raw, anchors)?,
            }
        }
        Ok(())
    }
    let mut candidate = body.clone();
    body_content(&mut candidate.content, anchors)?;
    *body = candidate;
    Ok(())
}

fn structural_fingerprint(xml: &[u8]) -> Result<u64> {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in xml {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    Ok(hash)
}

fn typed_complex_field_candidate_is_admitted(
    xml: &[u8],
    paragraph: &Range<usize>,
    candidate: &StoryItemSpan,
    inherited_word_prefixes: &[String],
) -> Result<bool> {
    let paragraph_xml = &xml[paragraph.clone()];
    let mut reader = quick_xml::Reader::from_reader(paragraph_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let paragraph_local_name = loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("typed story field paragraph scan failed: {error}"))
        })? {
            Event::Start(element) => {
                break element.local_name().as_ref().to_vec();
            }
            Event::Eof => {
                return Err(Error::Other(
                    "typed story field paragraph has no root element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };
    let scanned = &xml[candidate.scan.clone()];
    let marker_start = candidate
        .full
        .start
        .checked_sub(candidate.scan.start)
        .filter(|start| *start <= scanned.len())
        .ok_or_else(|| {
            Error::Other("complex story field starts outside its run span".to_owned())
        })?;
    let marker_end = candidate
        .full
        .end
        .checked_sub(candidate.scan.start)
        .filter(|end| marker_start <= *end && *end <= scanned.len())
        .ok_or_else(|| Error::Other("complex story field ends outside its run span".to_owned()))?;
    let mut run_reader = quick_xml::Reader::from_reader(scanned);
    run_reader.config_mut().trim_text(false);
    let mut run_buffer = Vec::new();
    let mut run_opening_end = None;
    let mut run_closing_start = None;
    loop {
        let before = run_reader.buffer_position() as usize;
        let event = run_reader
            .read_event_into(&mut run_buffer)
            .map_err(|error| Error::Other(format!("typed story field run scan failed: {error}")))?;
        let after = run_reader.buffer_position() as usize;
        if run_opening_end.is_none() && matches!(event, Event::Start(_)) {
            run_opening_end = Some(after);
        }
        if after == scanned.len() && matches!(event, Event::End(_)) {
            run_closing_start = Some(before);
            break;
        }
        if matches!(event, Event::Eof) {
            break;
        }
        run_buffer.clear();
    }
    let Some(run_opening_end) = run_opening_end.filter(|end| *end <= marker_start) else {
        return Ok(false);
    };
    let Some(run_closing_start) = run_closing_start.filter(|start| marker_end <= *start) else {
        return Ok(false);
    };
    let mut isolated = Vec::new();
    isolated.extend_from_slice(&scanned[..run_opening_end]);
    isolated.extend_from_slice(&scanned[marker_start..marker_end]);
    isolated.extend_from_slice(&scanned[run_closing_start..]);
    let mut scope = story_namespace_scope_at(xml, candidate.scan.start)?;
    let word_prefix = story_word_prefix_for_scope(&scope);
    scope
        .entry(word_prefix.clone())
        .or_insert_with(|| WORD_NAMESPACE.to_owned());
    let mut paragraph_name = Vec::new();
    if !word_prefix.is_empty() {
        paragraph_name.extend_from_slice(word_prefix.as_bytes());
        paragraph_name.push(b':');
    }
    paragraph_name.extend_from_slice(&paragraph_local_name);
    let mut fragment = Vec::new();
    fragment.push(b'<');
    fragment.extend_from_slice(&paragraph_name);
    for (prefix, namespace) in scope {
        fragment.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            fragment.push(b':');
            fragment.extend_from_slice(prefix.as_bytes());
        }
        fragment.extend_from_slice(b"=\"");
        fragment.extend_from_slice(quick_xml::escape::escape(&namespace).as_bytes());
        fragment.push(b'"');
    }
    fragment.push(b'>');
    fragment.extend_from_slice(&isolated);
    fragment.extend_from_slice(b"</");
    fragment.extend_from_slice(&paragraph_name);
    fragment.push(b'>');
    let Ok(sources) = CT_P::story_complex_field_sources(&fragment, inherited_word_prefixes) else {
        return Ok(false);
    };
    Ok(sources.iter().any(|source| source == &isolated))
}

fn scan_story_control_paragraphs(xml: &[u8], control: &StoryItemSpan) -> Result<Vec<Range<usize>>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut paragraphs = Vec::new();
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!(
                    "control paragraph scan failed: {error}"
                )));
            }
        };
        let namespace = story_namespace(&namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != control.full.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = story_frame_local_name(element.local_name().as_ref());
                let empty = matches!(event, Event::Empty(_));
                let is_word = namespace == StoryNamespace::Word;
                let root = stack.is_empty();
                if root && (!is_word || local.as_ref() != b"sdt") {
                    return Err(Error::Other(
                        "control paragraph source has no qualified selected control".into(),
                    ));
                }
                // Only this control's direct paragraphs belong to this inventory.
                // Descendants of another control cannot contribute, typed or raw.
                let opaque = !root
                    && ((is_word && local.as_ref() == b"sdt")
                        || opaque_story_event(
                            &mut stack, namespace, &local, element, empty, xml, before, after,
                        )?);
                let direct_content = stack.last().is_some_and(|parent| {
                    !parent.opaque
                        && parent.namespace == StoryNamespace::Word
                        && parent.local_name.as_ref() == b"sdtContent"
                });
                if !opaque && is_word && local.as_ref() == b"p" && direct_content {
                    paragraphs.push(
                        before..if empty {
                            after
                        } else {
                            story_element_end(xml, before)?
                        },
                    );
                }
                if !empty {
                    let inherited = if root {
                        Arc::new(
                            story_namespace_scope_at(xml, before)?
                                .into_iter()
                                .filter_map(|(prefix, uri)| {
                                    (uri == WORD_NAMESPACE).then_some(prefix)
                                })
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        stack.last().map_or_else(
                            || Arc::clone(&empty_word_prefixes),
                            |parent| Arc::clone(&parent.word_prefixes),
                        )
                    };
                    let word_prefixes = story_word_prefixes_at(element, &inherited, is_word)?;
                    let sdt_context = if root {
                        control.sdt_context
                    } else {
                        story_element_sdt_context(&stack, namespace, &local)
                    };
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: local,
                        is_word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        if after >= control.full.end {
            break;
        }
        buffer.clear();
    }
    Ok(paragraphs)
}

fn scan_story_control_paragraphs_with_scope(
    xml: &[u8],
    control: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<Vec<Range<usize>>> {
    let (closed, added) = scoped_story_fragment(xml, control.full.clone(), scope)?;
    let mut local_control = control.clone();
    local_control.full = 0..closed.len();
    let map = |position: usize| -> Result<usize> {
        let mapped = if position == 0 {
            Some(control.full.start)
        } else {
            position
                .checked_sub(added)
                .and_then(|offset| control.full.start.checked_add(offset))
        };
        mapped
            .filter(|offset| *offset <= control.full.end)
            .ok_or_else(|| {
                Error::Other("control paragraph excerpt has an invalid source offset".to_owned())
            })
    };
    scan_story_control_paragraphs(&closed, &local_control)?
        .into_iter()
        .map(|paragraph| Ok(map(paragraph.start)?..map(paragraph.end)?))
        .collect()
}

fn scan_story_items(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    mut template_field_ranges: Option<&mut Vec<Range<usize>>>,
) -> Result<Vec<StoryItemSpan>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut items = Vec::new();
    let mut complex_fields = Vec::<ComplexFieldScan>::new();
    let mut template_open_fields = Vec::<usize>::new();
    let mut paragraphs = Vec::<(Range<usize>, Arc<Vec<String>>)>::new();
    let mut active = false;
    let mut buffer = Vec::new();
    let mut ancestor_prefixes = Arc::new(Vec::new());
    let mut ancestor_frames: Vec<Arc<Vec<String>>> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story item scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != owner.full.start {
                match &event {
                    Event::Start(element) => {
                        let next = story_word_prefixes_at(element, &ancestor_prefixes, false)?;
                        ancestor_frames.push(std::mem::replace(&mut ancestor_prefixes, next));
                    }
                    Event::End(_) => {
                        if let Some(previous) = ancestor_frames.pop() {
                            ancestor_prefixes = previous;
                        }
                    }
                    Event::Eof => break,
                    _ => {}
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let is_word = is_word_namespace;
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let sdt_context =
                    if stack.is_empty() && is_word && local_name.as_ref() == b"sdtContent" {
                        Some(StorySdtContext::Block)
                    } else {
                        story_element_sdt_context(&stack, namespace_kind, &local_name)
                    };
                let depth = stack.len();
                let in_nested_owner = stack.iter().skip(1).any(|frame| {
                    frame.is_word && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent")
                });
                let child_opaque = in_nested_owner
                    || opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        &local_name,
                        &element,
                        false,
                        xml,
                        before,
                        after,
                    )?;
                let item_kind = if depth == 1 && !in_nested_owner {
                    Some(if child_opaque {
                        StoryItemKind::PreservedNode
                    } else {
                        direct_story_item_kind(is_word, &local_name)
                    })
                } else if !in_nested_owner && !child_opaque && is_word {
                    match local_name.as_ref() {
                        b"sdt" => Some(StoryItemKind::ContentControl),
                        b"fldSimple" => Some(StoryItemKind::Field),
                        // A drawing in `mc:Choice` is read only for the text
                        // boxes it holds, and lists no item of its own.
                        b"drawing"
                            if stack.last().is_some_and(|parent| {
                                parent.namespace == StoryNamespace::MarkupCompatibility
                            }) =>
                        {
                            None
                        }
                        b"drawing" | b"pict" => Some(StoryItemKind::Drawing),
                        _ => None,
                    }
                } else {
                    None
                };
                let opaque = child_opaque || item_kind == Some(StoryItemKind::PreservedNode);
                if !opaque
                    && !in_nested_owner
                    && is_word
                    && local_name.as_ref() == b"fldChar"
                    && complex_field_scan_context(&stack)
                    && let Some(run) = story_enclosing_run_source(xml, &stack)?
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if let Some(ranges) = template_field_ranges.as_mut() {
                        match marker {
                            Some(ComplexFieldMarker::Begin) => template_open_fields.push(before),
                            Some(ComplexFieldMarker::End) => ranges.push(
                                template_open_fields.pop().unwrap_or(owner.full.start)..after,
                            ),
                            Some(ComplexFieldMarker::Separate)
                                if template_open_fields.is_empty() =>
                            {
                                ranges.push(owner.full.start..owner.full.end)
                            }
                            _ => {}
                        }
                    }
                    scan_complex_field_marker(
                        &mut complex_fields,
                        &mut items,
                        marker,
                        before,
                        after,
                        run,
                    );
                }
                let inherited = stack
                    .last()
                    .map(|frame| &frame.word_prefixes)
                    .unwrap_or(&ancestor_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word,
                    full_start: before,
                    owner_kind: None,
                    item_kind,
                    direct_owner_child: depth == 1 && !in_nested_owner,
                    sdt_context,
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let is_word = is_word_namespace;
                let local_name = element.local_name();
                let sdt_context =
                    story_element_sdt_context(&stack, namespace_kind, local_name.as_ref());
                let depth = stack.len();
                let in_nested_owner = stack.iter().skip(1).any(|frame| {
                    frame.is_word && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent")
                });
                let opaque = in_nested_owner
                    || opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        local_name.as_ref(),
                        &element,
                        true,
                        xml,
                        before,
                        after,
                    )?;
                let kind = if depth == 1 && !in_nested_owner {
                    Some(if opaque {
                        StoryItemKind::PreservedNode
                    } else {
                        direct_story_item_kind(is_word, local_name.as_ref())
                    })
                } else if !in_nested_owner && !opaque && is_word {
                    match local_name.as_ref() {
                        b"sdt" => Some(StoryItemKind::ContentControl),
                        b"fldSimple" => Some(StoryItemKind::Field),
                        b"drawing" | b"pict" => Some(StoryItemKind::Drawing),
                        _ => None,
                    }
                } else {
                    None
                };
                if !opaque
                    && !in_nested_owner
                    && is_word
                    && local_name.as_ref() == b"fldChar"
                    && complex_field_scan_context(&stack)
                    && let Some(run) = story_enclosing_run_source(xml, &stack)?
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if let Some(ranges) = template_field_ranges.as_mut() {
                        match marker {
                            Some(ComplexFieldMarker::Begin) => template_open_fields.push(before),
                            Some(ComplexFieldMarker::End) => ranges.push(
                                template_open_fields.pop().unwrap_or(owner.full.start)..after,
                            ),
                            Some(ComplexFieldMarker::Separate)
                                if template_open_fields.is_empty() =>
                            {
                                ranges.push(owner.full.start..owner.full.end)
                            }
                            _ => {}
                        }
                    }
                    scan_complex_field_marker(
                        &mut complex_fields,
                        &mut items,
                        marker,
                        before,
                        after,
                        run,
                    );
                }
                if let Some(kind) = kind {
                    items.push(StoryItemSpan {
                        kind,
                        full: before..after,
                        scan: before..after,
                        direct_owner_child: depth == 1 && !in_nested_owner,
                        complex_field: false,
                        complex_ancestors: Vec::new(),
                        sdt_context,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story item XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if !frame.opaque
                    && frame.namespace == StoryNamespace::Word
                    && frame.local_name.as_ref() == b"p"
                {
                    paragraphs.push((frame.full_start..after, frame.word_prefixes.clone()));
                    complex_fields.clear();
                }
                if let Some(kind) = frame.item_kind {
                    items.push(StoryItemSpan {
                        kind,
                        full: frame.full_start..after,
                        scan: frame.full_start..after,
                        direct_owner_child: frame.direct_owner_child,
                        complex_field: false,
                        complex_ancestors: Vec::new(),
                        sdt_context: frame.sdt_context,
                    });
                }
                if after == owner.full.end {
                    break;
                }
            }
            Event::Text(value) => {
                let decoded = value.decode().map_err(|error| {
                    Error::Other(format!("field instruction decode failed: {error}"))
                })?;
                let value = quick_xml::escape::unescape(&decoded).map_err(|error| {
                    Error::Other(format!("field instruction unescape failed: {error}"))
                })?;
                push_complex_field_instruction(&mut complex_fields, &stack, &value);
            }
            Event::CData(value) => {
                let value = value.decode().map_err(|error| {
                    Error::Other(format!("field instruction CDATA decode failed: {error}"))
                })?;
                push_complex_field_instruction(&mut complex_fields, &stack, &value);
            }
            Event::GeneralRef(value) => push_complex_field_instruction(
                &mut complex_fields,
                &stack,
                &oxoffice_core::xml_text::resolve_entity(&value),
            ),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut typed_complex_admission = HashMap::<usize, bool>::new();
    if items.iter().any(|item| item.complex_field) {
        for (paragraph, word_prefixes) in &paragraphs {
            for candidate in items.iter().filter(|item| {
                item.complex_field
                    && paragraph.start <= item.scan.start
                    && item.scan.end <= paragraph.end
            }) {
                let admitted = typed_complex_field_candidate_is_admitted(
                    xml,
                    paragraph,
                    candidate,
                    word_prefixes,
                )?;
                typed_complex_admission.insert(candidate.full.start, admitted);
            }
        }
    }
    items.retain(|item| {
        !item.complex_field
            || (typed_complex_admission
                .get(&item.full.start)
                .copied()
                .unwrap_or(false)
                && item.complex_ancestors.iter().all(|ancestor| {
                    typed_complex_admission
                        .get(ancestor)
                        .copied()
                        .unwrap_or(false)
                }))
    });
    if let Some(ranges) = template_field_ranges {
        ranges.extend(
            template_open_fields
                .into_iter()
                .map(|start| start..owner.full.end),
        );
    }
    items.sort_by_key(|item| (item.full.start, item.full.end));
    items.dedup_by(|left, right| left.kind == right.kind && left.full == right.full);
    Ok(items)
}

fn scan_story_item_links(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<StoryLinkSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut open_links = Vec::new();
    let mut links = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let depth = stack.len();
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let opaque = if stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                    false
                } else {
                    opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        &local_name,
                        &element,
                        false,
                        xml,
                        before,
                        after,
                    )?
                };
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent" | b"sdt")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"hyperlink"
                {
                    let (rel_id, anchor) = story_hyperlink_attributes(&reader, &element)?;
                    open_links.push((before, rel_id, anchor));
                }
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    xml,
                    before,
                    after,
                )?;
                if !opaque
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"hyperlink"
                {
                    let (rel_id, anchor) = story_hyperlink_attributes(&reader, &element)?;
                    links.push(StoryLinkSpan {
                        full: before..after,
                        rel_id,
                        anchor,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story hyperlink XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if !frame.opaque
                    && nested_owner_depth == 0
                    && frame.namespace == StoryNamespace::Word
                    && frame.local_name.as_ref() == b"hyperlink"
                {
                    let (start, rel_id, anchor) = open_links.pop().ok_or_else(|| {
                        Error::Other("story hyperlink has no matching start".to_owned())
                    })?;
                    links.push(StoryLinkSpan {
                        full: start..after,
                        rel_id,
                        anchor,
                    });
                }
                if !frame.opaque
                    && !stack.is_empty()
                    && frame.namespace == StoryNamespace::Word
                    && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent" | b"sdt")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if after == item.scan.end {
            break;
        }
        buffer.clear();
    }
    if !open_links.is_empty() {
        return Err(Error::Other(
            "story hyperlink has no matching end".to_owned(),
        ));
    }
    Ok(links)
}

fn story_hyperlink_attributes(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<(Option<String>, Option<String>)> {
    let mut rel_id = None;
    let mut anchor = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story hyperlink attribute scan failed: {error}"))
        })?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        let target = if matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == drawing_ns::R.as_bytes()
        ) && local_name.as_ref() == b"id"
        {
            &mut rel_id
        } else if word_element(&namespace) && local_name.as_ref() == b"anchor" {
            &mut anchor
        } else {
            continue;
        };
        *target = Some(
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!("story hyperlink attribute decode failed: {error}"))
                })?
                .into_owned(),
        );
    }
    Ok((rel_id, anchor))
}

/// Return the start tag range of one hyperlink element and, unless it is
/// empty, its end tag range.
fn story_hyperlink_tags(
    xml: &[u8],
    link: &Range<usize>,
) -> Result<(Range<usize>, Option<Range<usize>>)> {
    let mut reader = quick_xml::Reader::from_reader(&xml[link.clone()]);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    loop {
        let before = link.start + reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
        let after = link.start + reader.buffer_position() as usize;
        match event {
            Event::Empty(_) if depth == 0 => return Ok((before..after, None)),
            Event::Start(_) => {
                if depth == 0 {
                    start = Some(before..after);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let start = start.ok_or_else(|| {
                        Error::Other("story hyperlink has no start tag".to_owned())
                    })?;
                    return Ok((start, Some(before..after)));
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story hyperlink has no closing tag".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

/// Rebuild a hyperlink start tag so that it names `relationship_id` and has no
/// anchor, keeping every other attribute.
fn external_story_hyperlink_start(
    tag: &[u8],
    scope: &BTreeMap<String, String>,
    relationship_id: &str,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(tag);
    let mut buffer = Vec::new();
    let (element, empty) = match reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?
    {
        Event::Start(element) => (element, false),
        Event::Empty(element) => (element, true),
        _ => {
            return Err(Error::Other(
                "story hyperlink start tag is invalid".to_owned(),
            ));
        }
    };
    let mut rewritten = Vec::with_capacity(tag.len() + relationship_id.len() + 16);
    rewritten.push(b'<');
    rewritten.extend_from_slice(element.name().as_ref());
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story hyperlink attribute scan failed: {error}"))
        })?;
        let key = attribute.key.as_ref();
        let resolved = key.iter().position(|byte| *byte == b':').and_then(|colon| {
            let prefix = std::str::from_utf8(&key[..colon]).ok()?;
            Some((scope.get(prefix)?.as_str(), &key[colon + 1..]))
        });
        if matches!(resolved, Some((namespace, b"id")) if namespace == drawing_ns::R)
            || matches!(resolved, Some((namespace, b"anchor")) if namespace == WORD_NAMESPACE)
        {
            continue;
        }
        let quote = if attribute.value.contains(&b'"') {
            b'\''
        } else {
            b'"'
        };
        rewritten.push(b' ');
        rewritten.extend_from_slice(key);
        rewritten.push(b'=');
        rewritten.push(quote);
        rewritten.extend_from_slice(&attribute.value);
        rewritten.push(quote);
    }
    let bound = scope
        .iter()
        .find(|(prefix, namespace)| !prefix.is_empty() && namespace.as_str() == drawing_ns::R)
        .map(|(prefix, _)| prefix.clone());
    let prefix = match bound {
        Some(prefix) => prefix,
        None => {
            let mut suffix = 0usize;
            let prefix = loop {
                let candidate = if suffix == 0 {
                    "r".to_owned()
                } else {
                    format!("r{suffix}")
                };
                if !scope.contains_key(&candidate) {
                    break candidate;
                }
                suffix += 1;
            };
            rewritten
                .extend_from_slice(format!(" xmlns:{prefix}=\"{}\"", drawing_ns::R).as_bytes());
            prefix
        }
    };
    rewritten.extend_from_slice(
        format!(
            " {prefix}:id=\"{}\"",
            quick_xml::escape::escape(relationship_id)
        )
        .as_bytes(),
    );
    rewritten.extend_from_slice(if empty { b"/>" } else { b">" });
    Ok(rewritten)
}

/// Remove one hyperlink element and keep its content in place.
///
/// Namespace declarations on the hyperlink move to each child element, and
/// `cleared_styles` are removed from the `w:rStyle` of the runs it held.
fn unwrap_story_hyperlink(
    xml: &[u8],
    link: &Range<usize>,
    scope: &BTreeMap<String, String>,
    cleared_styles: &HashSet<String>,
) -> Result<Vec<u8>> {
    let (start, end) = story_hyperlink_tags(xml, link)?;
    let mut edits = vec![(start.clone(), Vec::new())];
    if let Some(end) = end {
        let mut declarations = Vec::new();
        let mut reader = quick_xml::Reader::from_reader(&xml[start.clone()]);
        let mut buffer = Vec::new();
        if let Event::Start(element) = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?
        {
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| {
                    Error::Other(format!("story hyperlink attribute scan failed: {error}"))
                })?;
                let key = attribute.key.as_ref();
                if key == b"xmlns" || key.starts_with(b"xmlns:") {
                    declarations.push((key.to_vec(), attribute.value.into_owned()));
                }
            }
        }
        let (closed, added) = scoped_story_fragment(xml, link.clone(), scope)?;
        let map = |position: usize| link.start + position - added;
        let mut reader = NsReader::from_reader(closed.as_slice());
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let mut stack: Vec<(bool, Vec<u8>, Option<usize>)> = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
            let is_word = word_element(&namespace);
            drop(namespace);
            let after = reader.buffer_position() as usize;
            match event {
                Event::Start(ref element) | Event::Empty(ref element) => {
                    if stack.len() == 1 {
                        let insert_at = map(before + 1 + element.name().as_ref().len());
                        let mut inserted = Vec::new();
                        for (key, value) in &declarations {
                            let redeclared = element
                                .attributes()
                                .flatten()
                                .any(|attribute| attribute.key.as_ref() == key.as_slice());
                            if !redeclared {
                                inserted.push(b' ');
                                inserted.extend_from_slice(key);
                                inserted.extend_from_slice(b"=\"");
                                inserted.extend_from_slice(value);
                                inserted.push(b'"');
                            }
                        }
                        if !inserted.is_empty() {
                            edits.push((insert_at..insert_at, inserted));
                        }
                    }
                    let local = element.local_name().as_ref().to_vec();
                    let in_run_properties = is_word
                        && local == b"rStyle"
                        && stack.len() >= 2
                        && matches!(&stack[stack.len() - 2..], [(true, run, _), (true, properties, _)]
                            if run == b"r" && properties == b"rPr")
                        && !stack
                            .iter()
                            .any(|(word, name, _)| *word && name == b"txbxContent");
                    let cleared = in_run_properties
                        && element.attributes().flatten().any(|attribute| {
                            let (namespace, name) =
                                reader.resolver().resolve_attribute(attribute.key);
                            word_element(&namespace)
                                && name.as_ref() == b"val"
                                && attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .is_ok_and(|value| cleared_styles.contains(value.as_ref()))
                        });
                    if matches!(event, Event::Start(_)) {
                        stack.push((is_word, local, cleared.then(|| map(before))));
                    } else if cleared {
                        edits.push((map(before)..map(after), Vec::new()));
                    }
                }
                Event::End(_) => {
                    if let Some((_, _, Some(removed_from))) = stack.pop() {
                        edits.push((removed_from..map(after), Vec::new()));
                    }
                    if stack.is_empty() {
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        edits.push((end, Vec::new()));
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut updated = xml.to_vec();
    for (mut range, replacement) in edits.into_iter().rev() {
        // A removed tag takes its indentation with it: whitespace between
        // WordprocessingML elements outside `w:t` carries no content.
        if replacement.is_empty() && !range.is_empty() {
            let indented = xml[..range.start]
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace());
            if let Some(previous) = indented.filter(|previous| xml[*previous] == b'>') {
                range.start = previous + 1;
            }
        }
        updated.splice(range, replacement);
    }
    Ok(updated)
}

/// One admitted bookmark marker and its physical source range.
struct BookmarkMarkerSpan {
    start: bool,
    id: String,
    span: Range<usize>,
}

fn bookmark_marker_spans(xml: &[u8]) -> Result<Vec<BookmarkMarkerSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut markers = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("bookmark scan failed: {error}")))?;
        let namespace = story_namespace(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = story_frame_local_name(element.local_name().as_ref());
                let empty = matches!(event, Event::Empty(_));
                let modeled = stack.last().is_none_or(|parent| !parent.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, &local, element, empty, xml, before, after,
                    )?;
                if modeled
                    && accepted_story_text_visible(&stack)
                    && namespace == StoryNamespace::Word
                    && matches!(local.as_ref(), b"bookmarkStart" | b"bookmarkEnd")
                {
                    let mut id = None;
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if word_element(&namespace) && local.as_ref() == b"id" {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?;
                            if id.replace(value.into_owned()).is_some() {
                                return Err(Error::Other(
                                    "bookmark marker has duplicate owned id".to_owned(),
                                ));
                            }
                        }
                    }
                    if let Some(id) = id {
                        let end = if empty {
                            after
                        } else {
                            story_element_end(xml, before)?
                        };
                        if !empty {
                            let mut child = quick_xml::Reader::from_reader(&xml[after..end]);
                            if !matches!(
                                child
                                    .read_event()
                                    .map_err(|error| Error::Other(error.to_string()))?,
                                Event::End(_)
                            ) {
                                return Err(Error::Other(
                                    "a bookmark marker with child content cannot be removed"
                                        .to_owned(),
                                ));
                            }
                        }
                        markers.push(BookmarkMarkerSpan {
                            start: local.as_ref() == b"bookmarkStart",
                            id,
                            span: before..end,
                        });
                    }
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, &local);
                    let inherited = stack
                        .last()
                        .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                    let word_prefixes = story_word_prefixes_at(
                        element,
                        inherited,
                        namespace == StoryNamespace::Word,
                    )?;
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: local,
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(markers)
}

/// Resize every `pic:pic` drawing in `xml` whose blip names `relationship_id`,
/// returning the edited XML and the number of drawings resized.
///
/// `wp:extent` and the `a:ext` of `pic:spPr/a:xfrm` take the new size, and
/// `wp:effectExtent` scales with the extent on each axis.
fn resize_story_pictures(
    xml: &[u8],
    relationship_id: &str,
    cx: i64,
    cy: i64,
) -> Result<(Vec<u8>, usize)> {
    struct Drawing {
        depth: usize,
        extent: Option<Range<usize>>,
        effect_extent: Option<Range<usize>>,
        shape_extent: Option<Range<usize>>,
        shows_image: bool,
    }
    const A: &str = drawing_ns::A;
    const PIC: &str = drawing_ns::PIC;
    let on_path = |path: &[(String, Vec<u8>)], expected: &[(&str, &[u8])]| {
        path.len() == expected.len()
            && path.iter().zip(expected).all(
                |((namespace, local), (want_namespace, want_local))| {
                    namespace == want_namespace && local.as_slice() == *want_local
                },
            )
    };
    let attribute_value = |tag: Range<usize>, name: &[u8]| -> Result<(Range<usize>, i64)> {
        let (start, end) =
            story_attribute_value_span(&xml[tag.clone()], name).ok_or_else(|| {
                Error::Other(format!(
                    "picture drawing has no {} attribute",
                    String::from_utf8_lossy(name)
                ))
            })?;
        let range = tag.start + start..tag.start + end;
        let value = std::str::from_utf8(&xml[range.clone()])
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .ok_or_else(|| {
                Error::Other(format!(
                    "picture drawing {} is not an integer",
                    String::from_utf8_lossy(name)
                ))
            })?;
        Ok((range, value))
    };
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<(String, Vec<u8>)> = Vec::new();
    let mut drawings: Vec<Drawing> = Vec::new();
    let mut edits = Vec::new();
    let mut resized = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("picture drawing scan failed: {error}")))?;
        let namespace = match namespace {
            ResolveResult::Bound(Namespace(uri)) => String::from_utf8_lossy(uri).into_owned(),
            _ => String::new(),
        };
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name().as_ref().to_vec();
                if let Some(drawing) = drawings.last_mut() {
                    let path = &stack[drawing.depth + 1..];
                    if path.is_empty() && namespace == drawing_ns::WP {
                        match local.as_slice() {
                            b"extent" => drawing.extent = Some(before..after),
                            b"effectExtent" => drawing.effect_extent = Some(before..after),
                            _ => {}
                        }
                    } else if namespace == A
                        && local == b"blip"
                        && on_path(
                            path,
                            &[
                                (A, b"graphic"),
                                (A, b"graphicData"),
                                (PIC, b"pic"),
                                (PIC, b"blipFill"),
                            ],
                        )
                    {
                        drawing.shows_image |= element.attributes().flatten().any(|attribute| {
                            let (attribute_namespace, name) =
                                reader.resolver().resolve_attribute(attribute.key);
                            matches!(attribute_namespace, ResolveResult::Bound(Namespace(uri)) if uri == drawing_ns::R.as_bytes())
                                && matches!(name.as_ref(), b"embed" | b"link")
                                && attribute.value.as_ref() == relationship_id.as_bytes()
                        });
                    } else if namespace == A
                        && local == b"ext"
                        && on_path(
                            path,
                            &[
                                (A, b"graphic"),
                                (A, b"graphicData"),
                                (PIC, b"pic"),
                                (PIC, b"spPr"),
                                (A, b"xfrm"),
                            ],
                        )
                    {
                        drawing.shape_extent = Some(before..after);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    if namespace == drawing_ns::WP
                        && matches!(local.as_slice(), b"inline" | b"anchor")
                    {
                        drawings.push(Drawing {
                            depth: stack.len(),
                            extent: None,
                            effect_extent: None,
                            shape_extent: None,
                            shows_image: false,
                        });
                    }
                    stack.push((namespace, local));
                }
            }
            Event::End(_) => {
                stack.pop();
                if drawings
                    .last()
                    .is_some_and(|drawing| drawing.depth == stack.len())
                    && let Some(drawing) = drawings.pop()
                    && drawing.shows_image
                    && let Some(extent) = drawing.extent
                {
                    let (cx_range, old_cx) = attribute_value(extent.clone(), b"cx")?;
                    let (cy_range, old_cy) = attribute_value(extent, b"cy")?;
                    edits.push((cx_range, cx));
                    edits.push((cy_range, cy));
                    if let Some(shape_extent) = drawing.shape_extent {
                        edits.push((attribute_value(shape_extent.clone(), b"cx")?.0, cx));
                        edits.push((attribute_value(shape_extent, b"cy")?.0, cy));
                    }
                    if let Some(effect_extent) = drawing.effect_extent {
                        for (name, new, old) in [
                            (&b"l"[..], cx, old_cx),
                            (b"t", cy, old_cy),
                            (b"r", cx, old_cx),
                            (b"b", cy, old_cy),
                        ] {
                            let (range, value) = attribute_value(effect_extent.clone(), name)?;
                            let scaled = if old == 0 {
                                0
                            } else {
                                i64::try_from(i128::from(value) * i128::from(new) / i128::from(old))
                                    .map_err(|_| {
                                        Error::Other(
                                            "scaled picture effect extent overflows".to_owned(),
                                        )
                                    })?
                            };
                            edits.push((range, scaled));
                        }
                    }
                    resized += 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut updated = xml.to_vec();
    for (range, value) in edits.into_iter().rev() {
        updated.splice(range, value.to_string().into_bytes());
    }
    Ok((updated, resized))
}

fn story_item_text(
    xml: &[u8],
    item: &StoryItemSpan,
    qualified_items: &[StoryItemSpan],
    source_scan: Range<usize>,
    added: usize,
) -> Result<Option<String>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut in_text = 0usize;
    let mut pieces: Vec<(StoryFieldContext, StoryTextPiece)> = Vec::new();
    let mut fields = StoryFieldTracker::default();
    let mut active = false;
    let mut element_stack: Vec<XmlElementFrame> = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut complex_results = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story text scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let depth = element_stack.len();
                let local_name = story_frame_local_name(element.local_name().as_ref());
                // Reuse only controls independently admitted in this operation's
                // owner inventory. A typed parent can still retain untyped raw
                // descendants, which must follow ordinary qualification.
                let qualified_control = namespace_kind == StoryNamespace::Word
                    && local_name.as_ref() == b"sdt"
                    && !element_stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_child(&element_stack, namespace_kind, &local_name)
                    && (if !item.complex_field && before == 0 {
                        Some(source_scan.start)
                    } else {
                        before
                            .checked_sub(added)
                            .and_then(|offset| source_scan.start.checked_add(offset))
                    })
                    .and_then(|start| {
                        qualified_items
                            .binary_search_by_key(&start, |item| item.full.start)
                            .ok()
                    })
                    .is_some_and(|index| {
                        let control = &qualified_items[index];
                        control.kind == StoryItemKind::ContentControl
                            && !control.complex_field
                            && control.full == control.scan
                            && source_scan.start <= control.full.start
                            && control.full.end <= source_scan.end
                            && control.sdt_context == story_sdt_context(&element_stack)
                    });
                let opaque =
                    if element_stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                        false
                    } else if nested_owner_depth > 0 {
                        // The qualified nested owner excludes this content from
                        // the selected item. Keep inherited opacity and balanced
                        // owner depth while traversing its source and scopes.
                        element_stack.last().is_some_and(|frame| frame.opaque)
                    } else if qualified_control {
                        false
                    } else {
                        opaque_story_event(
                            &mut element_stack,
                            namespace_kind,
                            &local_name,
                            &element,
                            false,
                            xml,
                            before,
                            after,
                        )?
                    };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if item.complex_field {
                        update_complex_field_results(&mut complex_results, marker);
                    } else {
                        fields.mark(marker);
                    }
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                {
                    if local_name.as_ref() == b"t" {
                        in_text += 1;
                        pieces.push((fields.context(), StoryTextPiece::Text(String::new())));
                    } else if let Some(piece) =
                        story_run_character(&reader, &element, &element_stack, item)?
                    {
                        pieces.push((fields.context(), piece));
                    }
                }
                let inherited = element_stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                element_stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &element_stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let local_name = element.local_name();
                let opaque = if nested_owner_depth > 0 {
                    element_stack.last().is_some_and(|frame| frame.opaque)
                } else {
                    opaque_story_event(
                        &mut element_stack,
                        namespace_kind,
                        local_name.as_ref(),
                        &element,
                        true,
                        xml,
                        before,
                        after,
                    )?
                };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if item.complex_field {
                        update_complex_field_results(&mut complex_results, marker);
                    } else {
                        fields.mark(marker);
                    }
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                {
                    if local_name.as_ref() == b"t" {
                        pieces.push((fields.context(), StoryTextPiece::Text(String::new())));
                    } else if let Some(piece) =
                        story_run_character(&reader, &element, &element_stack, item)?
                    {
                        pieces.push((fields.context(), piece));
                    }
                }
            }
            Event::End(element) => {
                let opaque = element_stack.pop().is_some_and(|frame| frame.opaque);
                let depth = element_stack.len();
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && element.local_name().as_ref() == b"t"
                {
                    in_text = in_text.saturating_sub(1);
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(element.local_name().as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
            }
            Event::Text(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &quick_xml::escape::unescape(&value.decode().map_err(|error| {
                        Error::Other(format!("story text decode failed: {error}"))
                    })?)
                    .map_err(|error| {
                        Error::Other(format!("story text unescape failed: {error}"))
                    })?,
                )
            }
            Event::CData(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &value.decode().map_err(|error| {
                        Error::Other(format!("story CDATA decode failed: {error}"))
                    })?,
                )
            }
            Event::GeneralRef(value)
                if complex_field_text_visible(item, &complex_results) && in_text > 0 =>
            {
                push_story_text(
                    &mut pieces,
                    &oxoffice_core::xml_text::resolve_entity(&value),
                );
            }
            Event::Eof => break,
            _ => {}
        }
        if (item.complex_field && after >= item.full.end)
            || (!item.complex_field && after == item.scan.end)
        {
            break;
        }
        buffer.clear();
    }
    let mut text = String::new();
    let mut found = false;
    for (context, piece) in pieces {
        if fields.hides(&context) {
            continue;
        }
        match piece {
            StoryTextPiece::Text(value) => {
                text.push_str(&value);
                found = true;
            }
            StoryTextPiece::Character(plain, in_result) => {
                let character = if fields.in_result(&context) {
                    in_result
                } else {
                    plain
                };
                if let Some(character) = character {
                    text.push(character);
                    found = true;
                }
            }
        }
    }
    Ok((found || item.complex_field).then_some(text))
}

/// Append decoded text to the `w:t` piece it belongs to.
fn push_story_text(pieces: &mut [(StoryFieldContext, StoryTextPiece)], value: &str) {
    if let Some((_, StoryTextPiece::Text(text))) = pieces.last_mut() {
        text.push_str(value);
    }
}

/// Return whether an element opened at this stack is run content, so a run's
/// `w:tab` counts as text and a tab stop under `w:tabs` does not.
fn story_run_child(stack: &[XmlElementFrame]) -> bool {
    stack
        .last()
        .is_some_and(|frame| !frame.opaque && frame.is_word && frame.local_name.as_ref() == b"r")
}

/// The text a visible Word element adds when it is run content other than
/// `w:t`: a tab for `w:tab`, and a break for `w:br` or `w:cr`, as read
/// outside and inside a field result. A field item reads inside its field.
fn story_run_character(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    stack: &[XmlElementFrame],
    item: &StoryItemSpan,
) -> Result<Option<StoryTextPiece>> {
    if !story_run_child(stack) {
        return Ok(None);
    }
    let (plain, in_result) = match element.local_name().as_ref() {
        b"tab" => (Some('\t'), Some('\t')),
        b"br" | b"cr" => (
            story_break_text(reader, element, false)?,
            story_break_text(reader, element, true)?,
        ),
        _ => return Ok(None),
    };
    let plain = if item.complex_field { in_result } else { plain };
    Ok(Some(StoryTextPiece::Character(plain, in_result)))
}

fn story_text_start(element: &BytesStart<'_>, preserve_space: bool) -> Result<Vec<u8>> {
    let content: &[u8] = element;
    let mut xml_space_key = None;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story text attribute scan failed: {error}")))?;
        if attribute.key.as_ref() == b"xml:space" {
            let key_address = attribute.key.as_ref().as_ptr() as usize;
            let content_address = content.as_ptr() as usize;
            let key_start = key_address.checked_sub(content_address).ok_or_else(|| {
                Error::Other("story text attribute lies outside its opening tag".to_owned())
            })?;
            if key_start + attribute.key.as_ref().len() > content.len() {
                return Err(Error::Other(
                    "story text attribute lies outside its opening tag".to_owned(),
                ));
            }
            xml_space_key = Some((key_start, attribute.key.as_ref().len()));
        }
    }

    let mut updated = Vec::with_capacity(content.len() + 23);
    updated.push(b'<');
    if let Some((key_start, key_len)) = xml_space_key {
        let mut attribute_start = key_start;
        while attribute_start > element.name().as_ref().len()
            && content[attribute_start - 1].is_ascii_whitespace()
        {
            attribute_start -= 1;
        }
        let mut attribute_end = key_start + key_len;
        while content
            .get(attribute_end)
            .is_some_and(u8::is_ascii_whitespace)
        {
            attribute_end += 1;
        }
        if content.get(attribute_end) != Some(&b'=') {
            return Err(Error::Other(
                "story text xml:space attribute has no equals sign".to_owned(),
            ));
        }
        attribute_end += 1;
        while content
            .get(attribute_end)
            .is_some_and(u8::is_ascii_whitespace)
        {
            attribute_end += 1;
        }
        let quote = *content.get(attribute_end).ok_or_else(|| {
            Error::Other("story text xml:space attribute has no quoted value".to_owned())
        })?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(Error::Other(
                "story text xml:space attribute has no quoted value".to_owned(),
            ));
        }
        attribute_end += 1;
        let value_end = content[attribute_end..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|offset| attribute_end + offset + 1)
            .ok_or_else(|| {
                Error::Other("story text xml:space attribute has no closing quote".to_owned())
            })?;
        updated.extend_from_slice(&content[..attribute_start]);
        updated.extend_from_slice(&content[value_end..]);
    } else {
        updated.extend_from_slice(content);
    }
    if preserve_space {
        updated.extend_from_slice(b" xml:space=\"preserve\"");
    }
    updated.push(b'>');
    Ok(updated)
}

fn replace_story_item_text(source: &[u8], item: &StoryItemSpan, value: &str) -> Result<Vec<u8>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut text_depth = 0usize;
    let mut root_prefix = None;
    let mut root_end = None;
    let mut root_is_empty = false;
    let mut texts: Vec<StoryTextElement> = Vec::new();
    // Visible run tabs and breaks, which the new text replaces as well.
    let mut removed: Vec<(StoryFieldContext, Range<usize>)> = Vec::new();
    let mut removed_start = None;
    let mut fields = StoryFieldTracker::default();
    let mut element_stack: Vec<XmlElementFrame> = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut complex_results = Vec::new();
    let mut complex_field_edit = ComplexFieldEdit::default();
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story text mutation scan failed: {error}")))?;
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let depth = element_stack.len();
                if depth == 0 {
                    let qualified_name = element.name();
                    let name = std::str::from_utf8(qualified_name.as_ref()).map_err(|error| {
                        Error::Other(format!("story item name is not UTF-8: {error}"))
                    })?;
                    root_prefix = Some(
                        name.split_once(':')
                            .map_or("", |(prefix, _)| prefix)
                            .to_owned(),
                    );
                }
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let opaque =
                    if element_stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                        false
                    } else {
                        opaque_story_event(
                            &mut element_stack,
                            namespace_kind,
                            &local_name,
                            &element,
                            false,
                            source,
                            before,
                            after,
                        )?
                    };
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    track_story_field_marker(
                        item,
                        marker,
                        &mut fields,
                        &mut complex_results,
                        &mut complex_field_edit,
                        before,
                        &element_stack,
                    );
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"t"
                {
                    texts.push(StoryTextElement {
                        context: fields.context(),
                        tag: before..after,
                        markup: story_text_markup_tags(
                            &element,
                            story_text_preserves_space(value),
                        )?,
                        empty: false,
                        content: Vec::new(),
                    });
                    text_depth += 1;
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && story_run_child(&element_stack)
                    && matches!(local_name.as_ref(), b"tab" | b"br" | b"cr")
                {
                    removed_start = Some((element_stack.len(), before, fields.context()));
                }
                let inherited = element_stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                element_stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &element_stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                if element_stack.is_empty() {
                    root_is_empty = true;
                    let qualified_name = element.name();
                    let name = std::str::from_utf8(qualified_name.as_ref()).map_err(|error| {
                        Error::Other(format!("story item name is not UTF-8: {error}"))
                    })?;
                    root_prefix = Some(
                        name.split_once(':')
                            .map_or("", |(prefix, _)| prefix)
                            .to_owned(),
                    );
                }
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut element_stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    source,
                    before,
                    after,
                )?;
                if !opaque
                    && story_field_marker_tracked(item, before, nested_owner_depth, &element_stack)
                    && is_word_namespace
                    && local_name.as_ref() == b"fldChar"
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    track_story_field_marker(
                        item,
                        marker,
                        &mut fields,
                        &mut complex_results,
                        &mut complex_field_edit,
                        before,
                        &element_stack,
                    );
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"t"
                {
                    texts.push(StoryTextElement {
                        context: fields.context(),
                        tag: before..after,
                        markup: story_text_markup_tags(
                            &element,
                            story_text_preserves_space(value),
                        )?,
                        empty: true,
                        content: Vec::new(),
                    });
                } else if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && accepted_story_text_visible(&element_stack)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && story_run_child(&element_stack)
                    && matches!(local_name.as_ref(), b"tab" | b"br" | b"cr")
                {
                    removed.push((fields.context(), before..after));
                }
            }
            Event::End(element) => {
                let opaque = element_stack.pop().is_some_and(|frame| frame.opaque);
                let depth = element_stack.len();
                if removed_start
                    .as_ref()
                    .is_some_and(|(start_depth, _, _)| *start_depth == depth)
                    && let Some((_, start, context)) = removed_start.take()
                {
                    removed.push((context, start..after));
                }
                if !opaque
                    && complex_field_text_visible(item, &complex_results)
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && element.local_name().as_ref() == b"t"
                {
                    text_depth = text_depth.saturating_sub(1);
                }
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(element.local_name().as_ref(), b"tc" | b"txbxContent")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
                if depth == 0 {
                    root_end = Some(before);
                }
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
                if complex_field_text_visible(item, &complex_results) && text_depth > 0 =>
            {
                if let Some(text) = texts.last_mut() {
                    text.content.push(before..after);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if (item.complex_field && after >= item.full.end)
            || (!item.complex_field && after == item.scan.end)
        {
            break;
        }
        buffer.clear();
    }

    // Only a field closed within the item hides its instruction part.
    let texts = texts
        .into_iter()
        .filter(|text| !fields.hides(&text.context))
        .collect::<Vec<_>>();
    let removed_ranges = removed
        .into_iter()
        .filter(|(context, _)| !fields.hides(context))
        .map(|(_, range)| range)
        .collect::<Vec<_>>();
    let ComplexFieldEdit {
        separator_found: outer_complex_separator_found,
        end_marker_start: complex_end_marker_start,
        word_prefixes: complex_insertion_word_prefixes,
    } = complex_field_edit;
    let mut updated = source.to_vec();
    let space = if story_text_preserves_space(value) {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    if texts.is_empty() {
        // Each insertion lies after every removed tab or break, so removing
        // them afterwards from the end keeps the insertion in place.
        if item.complex_field {
            let insert_at = complex_end_marker_start.ok_or_else(|| {
                Error::Other("complex story field has no closing marker".to_owned())
            })?;
            let nonempty_word_prefix = complex_insertion_word_prefixes
                .iter()
                .find(|prefix| !prefix.is_empty());
            let default_word_namespace =
                complex_insertion_word_prefixes.iter().any(String::is_empty);
            let (prefix, declaration, attribute_prefix, attribute_declaration) =
                if let Some(prefix) = nonempty_word_prefix {
                    (format!("{prefix}:"), String::new(), prefix.as_str(), "")
                } else if default_word_namespace {
                    (
                        String::new(),
                        String::new(),
                        "w",
                        concat!(
                            " xmlns:w=\"",
                            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
                            "\""
                        ),
                    )
                } else {
                    (
                        "w:".to_owned(),
                        format!(" xmlns:w=\"{WORD_NAMESPACE}\""),
                        "w",
                        "",
                    )
                };
            let separator = if outer_complex_separator_found {
                String::new()
            } else {
                format!(
                    "<{prefix}fldChar{declaration}{attribute_declaration} \
                     {attribute_prefix}:fldCharType=\"separate\"/>"
                )
            };
            let markup = StoryTextMarkup::new(&prefix, &declaration, space);
            let mut replacement = separator.into_bytes();
            replacement.extend_from_slice(&markup.open);
            replacement.extend_from_slice(&story_text_content(value, &markup));
            replacement.extend_from_slice(&markup.close);
            updated.splice(insert_at..insert_at, replacement);
        } else {
            if item.kind != StoryItemKind::Paragraph {
                return Err(StoryError::NotTextBearing { kind: item.kind }.into());
            }
            let prefix = root_prefix.unwrap_or_else(|| "w".to_owned());
            let prefix = if prefix.is_empty() {
                String::new()
            } else {
                format!("{prefix}:")
            };
            let markup = StoryTextMarkup::new(&prefix, "", space);
            let mut run = format!("<{prefix}r>").into_bytes();
            run.extend_from_slice(&markup.open);
            run.extend_from_slice(&story_text_content(value, &markup));
            run.extend_from_slice(&markup.close);
            run.extend_from_slice(format!("</{prefix}r>").as_bytes());
            if let Some(end) = root_end {
                updated.splice(end..end, run);
            } else if root_is_empty {
                let slash = item.full.end.checked_sub(2).ok_or_else(|| {
                    Error::Other("self-closing paragraph story item is truncated".to_owned())
                })?;
                if source.get(slash..item.full.end) != Some(b"/>".as_slice()) {
                    return Err(Error::Other(
                        "self-closing paragraph story item has no closing slash".to_owned(),
                    ));
                }
                let mut replacement = b">".to_vec();
                replacement.extend_from_slice(&run);
                replacement.extend_from_slice(format!("</{prefix}p>").as_bytes());
                updated.splice(slash..item.full.end, replacement);
            } else {
                return Err(Error::Other(
                    "paragraph story item has no closing element".to_owned(),
                ));
            }
        }
        for range in removed_ranges.into_iter().rev() {
            updated.splice(range, []);
        }
    } else {
        let mut edits: Vec<(Range<usize>, Vec<u8>)> = removed_ranges
            .into_iter()
            .map(|range| (range, Vec::new()))
            .collect();
        for (ordinal, text) in texts.into_iter().enumerate() {
            if ordinal == 0 {
                // The first text element takes the whole new text.
                let content = story_text_content(value, &text.markup);
                if text.empty {
                    let mut replacement = text.markup.open;
                    replacement.extend_from_slice(&content);
                    replacement.extend_from_slice(&text.markup.close);
                    edits.push((text.tag, replacement));
                } else {
                    edits.push((text.tag.end..text.tag.end, content));
                    edits.push((text.tag, text.markup.open));
                }
            }
            edits.extend(text.content.into_iter().map(|range| (range, Vec::new())));
        }
        // A start tag ends where the new text is inserted, so equal starts
        // sort the start tag first and apply it last.
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        for (range, replacement) in edits.into_iter().rev() {
            updated.splice(range, replacement);
        }
    }
    Ok(updated)
}

/// One accepted text element an item's new text may replace, with the
/// fields open where it was read.
struct StoryTextElement {
    context: StoryFieldContext,
    tag: Range<usize>,
    markup: StoryTextMarkup,
    empty: bool,
    content: Vec<Range<usize>>,
}

/// Where new text goes in a field item that has no text yet.
#[derive(Default)]
struct ComplexFieldEdit {
    separator_found: bool,
    end_marker_start: Option<usize>,
    word_prefixes: Vec<String>,
}

/// Track one field character for text replacement. A field item records
/// where its own field separates and ends. Any other item tracks the fields
/// of its story to learn which close within it.
fn track_story_field_marker(
    item: &StoryItemSpan,
    marker: Option<ComplexFieldMarker>,
    fields: &mut StoryFieldTracker,
    complex_results: &mut Vec<bool>,
    edit: &mut ComplexFieldEdit,
    position: usize,
    stack: &[XmlElementFrame],
) {
    if !item.complex_field {
        fields.mark(marker);
        return;
    }
    if complex_results.len() == 1 && marker == Some(ComplexFieldMarker::Separate) {
        edit.separator_found = true;
    } else if complex_results.len() == 1 && marker == Some(ComplexFieldMarker::End) {
        edit.end_marker_start = Some(position);
        edit.word_prefixes = stack
            .last()
            .map_or_else(Vec::new, |frame| frame.word_prefixes.as_ref().clone());
    }
    update_complex_field_results(complex_results, marker);
}

/// The tags that write new story text into a run: the text element's start
/// and end tags, and the tab and break elements a tab or line feed becomes.
struct StoryTextMarkup {
    open: Vec<u8>,
    close: Vec<u8>,
    tab: Vec<u8>,
    br: Vec<u8>,
}

impl StoryTextMarkup {
    fn new(prefix: &str, declaration: &str, space: &str) -> Self {
        Self {
            open: format!("<{prefix}t{declaration}{space}>").into_bytes(),
            close: format!("</{prefix}t>").into_bytes(),
            tab: format!("<{prefix}tab{declaration}/>").into_bytes(),
            br: format!("<{prefix}br{declaration}/>").into_bytes(),
        }
    }
}

/// Build the markup for new text written into an existing `w:t`, keeping its
/// attributes and namespace declarations for any further text element.
fn story_text_markup_tags(
    element: &BytesStart<'_>,
    preserve_space: bool,
) -> Result<StoryTextMarkup> {
    let name = element.name();
    let name = std::str::from_utf8(name.as_ref())
        .map_err(|error| Error::Other(format!("story text name is not UTF-8: {error}")))?;
    let prefix = name.strip_suffix('t').unwrap_or_default();
    let mut declarations = String::new();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story text attribute scan failed: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            declarations.push(' ');
            declarations.push_str(&String::from_utf8_lossy(key));
            declarations.push_str("=\"");
            declarations.push_str(&String::from_utf8_lossy(&attribute.value));
            declarations.push('"');
        }
    }
    let mut markup = StoryTextMarkup::new(prefix, &declarations, "");
    markup.open = story_text_start(element, preserve_space)?;
    markup.close = format!("</{name}>").into_bytes();
    Ok(markup)
}

/// Escape new story text, writing a tab as `w:tab` and a line feed, a
/// carriage return or a Windows line ending as `w:br` between text elements,
/// as `Paragraph::set_text` does.
fn story_text_content(value: &str, markup: &StoryTextMarkup) -> Vec<u8> {
    let value = if value.contains("\r\n") {
        std::borrow::Cow::Owned(value.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(value)
    };
    let mut content = Vec::with_capacity(value.len());
    let mut separators = value
        .chars()
        .filter(|character| matches!(character, '\t' | '\n' | '\r'));
    for (index, segment) in value.split(['\t', '\n', '\r']).enumerate() {
        if index > 0 {
            content.extend_from_slice(&markup.close);
            if separators.next() == Some('\t') {
                content.extend_from_slice(&markup.tab);
            } else {
                content.extend_from_slice(&markup.br);
            }
            content.extend_from_slice(&markup.open);
        }
        content.extend_from_slice(quick_xml::escape::escape(segment).as_bytes());
    }
    content
}

/// Return whether any text element of new story text must keep its spaces.
fn story_text_preserves_space(value: &str) -> bool {
    value
        .split(['\t', '\n', '\r'])
        .any(|segment| segment.starts_with(' ') || segment.ends_with(' '))
}

/// The heading level of a paragraph, from its style: the built-in name
/// `heading N` in any case, which Word keeps whatever language it saves in
/// (the id is localised, `Titre1` in French), else an id `HeadingN`.
pub(crate) fn paragraph_heading_level(para: &CT_P, styles: &CT_Styles) -> Option<u32> {
    let style_id = para.properties.as_ref()?.style_id.as_deref()?;
    let level = |rest: &str| {
        rest.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=9).contains(n))
    };
    let by_name = styles
        .get_by_id(style_id)
        .and_then(|style| style.name.as_deref())
        .and_then(|name| {
            name.to_ascii_lowercase()
                .strip_prefix("heading ")
                .and_then(level)
        });
    by_name.or_else(|| style_id.strip_prefix("Heading").and_then(level))
}

fn hyperlink_has_visible_content(source: &[u8], range: Range<usize>) -> Result<bool> {
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut word_text_depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("hyperlink content scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if after <= range.start {
            buffer.clear();
            continue;
        }
        if before >= range.end {
            return Ok(false);
        }
        match event {
            Event::Start(element) if is_word && element.local_name().as_ref() == b"t" => {
                word_text_depth += 1;
            }
            Event::End(element) if is_word && element.local_name().as_ref() == b"t" => {
                word_text_depth = word_text_depth.saturating_sub(1);
            }
            Event::Text(text) if word_text_depth > 0 => {
                let bytes: &[u8] = text.as_ref();
                if !bytes.is_empty() {
                    return Ok(true);
                }
            }
            Event::Start(element) | Event::Empty(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"tab"
                            | b"br"
                            | b"cr"
                            | b"sym"
                            | b"drawing"
                            | b"pict"
                            | b"object"
                            | b"footnoteReference"
                            | b"endnoteReference"
                    ) =>
            {
                return Ok(true);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn story_hyperlinks(source: &[u8], item_range: Range<usize>) -> Result<Vec<(Range<usize>, bool)>> {
    source
        .get(item_range.clone())
        .ok_or_else(|| Error::Other("story item range is outside updated XML".to_owned()))?;
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut ranges = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("empty hyperlink scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if after <= item_range.start {
            buffer.clear();
            continue;
        }
        if before >= item_range.end {
            break;
        }
        if let Event::Start(element) = event {
            if is_word && element.local_name().as_ref() == b"hyperlink" {
                let end = story_element_end(source, before)?;
                let visible = hyperlink_has_visible_content(source, before..end)?;
                ranges.push((before..end, visible));
            }
        } else if matches!(event, Event::Eof) {
            break;
        }
        buffer.clear();
    }
    Ok(ranges)
}

fn remove_newly_empty_hyperlinks(
    before: &[u8],
    before_item: Range<usize>,
    after: &[u8],
    after_item: Range<usize>,
) -> Result<Vec<u8>> {
    let old_links = story_hyperlinks(before, before_item)?;
    let new_links = story_hyperlinks(after, after_item)?;
    let ranges = new_links
        .into_iter()
        .enumerate()
        .filter_map(|(index, (range, visible))| {
            (!visible
                && old_links
                    .get(index)
                    .is_some_and(|(_, old_visible)| *old_visible))
            .then_some(range)
        })
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return Ok(after.to_vec());
    }
    let mut updated = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for range in ranges {
        updated.extend_from_slice(&after[cursor..range.start]);
        cursor = range.end;
    }
    updated.extend_from_slice(&after[cursor..]);
    Ok(updated)
}

fn validate_fresh_word_compatible_package(
    package: &OpcPackage,
    expected_class: WordPackageClass,
) -> Result<()> {
    let (_, actual_class) = validated_word_package_class(package)?;
    if actual_class != expected_class {
        return Err(Error::Other(
            "fresh Word package class does not match its profile".to_owned(),
        ));
    }
    let expected_parts = [
        ("/word/document.xml", expected_class.content_type()),
        (DEFAULT_STYLES_PART, STYLES_CONTENT_TYPE),
        (DEFAULT_SETTINGS_PART, SETTINGS_CONTENT_TYPE),
        (DEFAULT_THEME_PART, content_types::THEME),
        (DEFAULT_FONT_TABLE_PART, FONT_TABLE_CONTENT_TYPE),
        (DEFAULT_CORE_PROPERTIES_PART, content_types::CORE_PROPERTIES),
        (
            DEFAULT_APP_PROPERTIES_PART,
            content_types::EXTENDED_PROPERTIES,
        ),
    ];
    for (part_name, expected_content_type) in expected_parts {
        if !package.contains_part(part_name)
            || package
                .content_types
                .override_for(part_name)
                .is_none_or(|actual| actual != expected_content_type)
        {
            return Err(Error::Other(format!(
                "fresh Word package is missing required part {part_name}"
            )));
        }
    }
    if package.parts.len() != expected_parts.len()
        || package.content_types.overrides.len() != expected_parts.len()
    {
        return Err(Error::Other(
            "fresh Word package contains an unexpected owned part".to_owned(),
        ));
    }
    let expected_package_relationships = [
        (rel_types::DOCUMENT, "/word/document.xml"),
        (rel_types::CORE_PROPERTIES, DEFAULT_CORE_PROPERTIES_PART),
        (rel_types::EXTENDED_PROPERTIES, DEFAULT_APP_PROPERTIES_PART),
    ];
    let expected_document_relationships = [
        (rel_types::STYLES, DEFAULT_STYLES_PART),
        (rel_types::SETTINGS, DEFAULT_SETTINGS_PART),
        (rel_types::THEME, DEFAULT_THEME_PART),
        (rel_types::FONT_TABLE, DEFAULT_FONT_TABLE_PART),
    ];
    let document_relationships = package
        .get_part_rels("/word/document.xml")
        .ok_or_else(|| Error::Other("fresh Word package has no main relationships".to_owned()))?;
    if package.package_rels.items.len() != expected_package_relationships.len()
        || document_relationships.items.len() != expected_document_relationships.len()
        || package.part_rels.len() != 1
    {
        return Err(Error::Other(
            "fresh Word package relationship inventory is incomplete".to_owned(),
        ));
    }
    for (source, relationships, expected) in [
        (
            "/",
            &package.package_rels,
            expected_package_relationships.as_slice(),
        ),
        (
            "/word/document.xml",
            document_relationships,
            expected_document_relationships.as_slice(),
        ),
    ] {
        for (relationship_type, expected_target) in expected {
            let matching: Vec<_> = relationships
                .items
                .iter()
                .filter(|relationship| relationship.rel_type == *relationship_type)
                .collect();
            if matching.len() != 1
                || matching[0].target_mode.is_some()
                || OpcPackage::resolve_rel_target(source, &matching[0].target) != *expected_target
            {
                return Err(Error::Other(format!(
                    "fresh Word package relationship {relationship_type} is invalid"
                )));
            }
        }
    }
    for (source, relationships) in std::iter::once(("/", &package.package_rels)).chain(
        package
            .part_rels
            .iter()
            .map(|(source, rels)| (source.as_str(), rels)),
    ) {
        for relationship in &relationships.items {
            if !relationship_is_internal(relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(source, &relationship.target);
            if !package.contains_part(&target) {
                return Err(Error::Other(format!(
                    "fresh Word relationship targets missing part {target}"
                )));
            }
        }
    }
    Ok(())
}

fn validated_word_package_class(package: &OpcPackage) -> Result<(String, WordPackageClass)> {
    let relationships: Vec<_> = package
        .package_rels
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == rel_types::DOCUMENT)
        .collect();
    let relationship = match relationships.as_slice() {
        [relationship] => *relationship,
        [] => return Err(Error::NoDocumentPart),
        _ => {
            return Err(Error::Other(
                "Word package has multiple officeDocument relationships".to_owned(),
            ));
        }
    };
    if !matches!(relationship.target_mode.as_deref(), None | Some("Internal")) {
        return Err(Error::Other(
            "Word package officeDocument relationship must be internal".to_owned(),
        ));
    }
    let target = relationship.target.as_str();
    if !crate::embedded::relationship_target_is_normalized_pack_uri(target)
        || target
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err(Error::Other(
            "Word package officeDocument relationship has an unsafe target".to_owned(),
        ));
    }
    let doc_part_name = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("/{target}")
    };
    if !package.contains_part(&doc_part_name) {
        return Err(Error::NoDocumentPart);
    }
    let content_type = package
        .content_types
        .override_for(&doc_part_name)
        .ok_or_else(|| {
            Error::Other("Word main part requires an exact content-type override".to_owned())
        })?;
    let class = WordPackageClass::from_content_type(content_type).ok_or_else(|| {
        Error::Other(format!(
            "unsupported Word main-part content type: {content_type}"
        ))
    })?;
    Ok((doc_part_name, class))
}

fn take_paragraph<'a>(paragraph: &'a mut CT_P, index: &mut usize) -> Option<&'a mut CT_P> {
    if *index == 0 {
        Some(paragraph)
    } else {
        *index -= 1;
        None
    }
}

fn nth_paragraph_in_body<'a>(
    content: &'a mut [BodyContent],
    index: &mut usize,
) -> Option<&'a mut CT_P> {
    for child in content {
        let paragraph = match child {
            BodyContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            BodyContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            BodyContent::Table(_) | BodyContent::RawXml(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn nth_paragraph_in_control<'a>(
    control: &'a mut CT_Sdt,
    index: &mut usize,
) -> Option<&'a mut CT_P> {
    for child in &mut control.content {
        let paragraph = match child {
            SdtContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            SdtContent::Table(table) => nth_paragraph_in_table(table, index),
            SdtContent::Row(row) => nth_paragraph_in_row(row, index),
            SdtContent::Cell(cell) => nth_paragraph_in_cell(cell, index),
            SdtContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            SdtContent::Run(_) | SdtContent::RawXml(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn paragraph_count_in_control(control: &CT_Sdt) -> usize {
    control
        .content
        .iter()
        .map(|child| match child {
            SdtContent::Paragraph(_) => 1,
            SdtContent::Table(table) => paragraph_count_in_table(table),
            SdtContent::Row(row) => paragraph_count_in_row(row),
            SdtContent::Cell(cell) => paragraph_count_in_cell(cell),
            SdtContent::ContentControl(control) => paragraph_count_in_control(control),
            SdtContent::Run(_) | SdtContent::RawXml(_) => 0,
        })
        .sum()
}

fn paragraph_count_in_table(table: &CT_Tbl) -> usize {
    table
        .content_controls
        .iter()
        .map(|(_, _, control)| paragraph_count_in_control(control))
        .sum::<usize>()
        + table.rows.iter().map(paragraph_count_in_row).sum::<usize>()
}

fn paragraph_count_in_row(row: &CT_Row) -> usize {
    row.content_controls
        .iter()
        .map(|(_, _, control)| paragraph_count_in_control(control))
        .sum::<usize>()
        + row.cells.iter().map(paragraph_count_in_cell).sum::<usize>()
}

fn paragraph_count_in_cell(cell: &CT_Tc) -> usize {
    cell.content
        .iter()
        .map(|child| match child {
            CellContent::Paragraph(_) => 1,
            CellContent::ContentControl(control) => paragraph_count_in_control(control),
            CellContent::Table(_) => 0,
        })
        .sum()
}

fn nth_paragraph_in_table<'a>(table: &'a mut CT_Tbl, index: &mut usize) -> Option<&'a mut CT_P> {
    let CT_Tbl {
        rows,
        content_controls,
        ..
    } = table;
    let mut selected_control = None;
    let mut selected_row = None;
    for boundary in 0..=rows.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = paragraph_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(row) = rows.get(boundary) {
            let count = paragraph_count_in_row(row);
            if *index < count {
                selected_row = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_paragraph_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(row_index) = selected_row {
        nth_paragraph_in_row(&mut rows[row_index], index)
    } else {
        None
    }
}

fn nth_paragraph_in_row<'a>(row: &'a mut CT_Row, index: &mut usize) -> Option<&'a mut CT_P> {
    let CT_Row {
        cells,
        content_controls,
        ..
    } = row;
    let mut selected_control = None;
    let mut selected_cell = None;
    for boundary in 0..=cells.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = paragraph_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(cell) = cells.get(boundary) {
            let count = paragraph_count_in_cell(cell);
            if *index < count {
                selected_cell = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_paragraph_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(cell_index) = selected_cell {
        nth_paragraph_in_cell(&mut cells[cell_index], index)
    } else {
        None
    }
}

fn nth_paragraph_in_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_P> {
    for child in &mut cell.content {
        let paragraph = match child {
            CellContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            CellContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            CellContent::Table(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn take_table<'a>(table: &'a mut CT_Tbl, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    if *index == 0 {
        Some(table)
    } else {
        *index -= 1;
        None
    }
}

fn nth_table_in_body_ref<'a>(content: &'a [BodyContent], index: &mut usize) -> Option<&'a CT_Tbl> {
    fn take<'a>(table: &'a CT_Tbl, index: &mut usize) -> Option<&'a CT_Tbl> {
        if *index == 0 {
            Some(table)
        } else {
            *index -= 1;
            None
        }
    }
    fn in_cell<'a>(cell: &'a CT_Tc, index: &mut usize) -> Option<&'a CT_Tbl> {
        for child in &cell.content {
            let found = match child {
                CellContent::Table(table) => take(table, index),
                CellContent::ContentControl(control) => in_control(control, index),
                CellContent::Paragraph(_) => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
    fn in_control<'a>(control: &'a CT_Sdt, index: &mut usize) -> Option<&'a CT_Tbl> {
        for child in &control.content {
            let found = match child {
                SdtContent::Table(table) => take(table, index),
                SdtContent::Cell(cell) => in_cell(cell, index),
                SdtContent::ContentControl(control) => in_control(control, index),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
    for child in content {
        let found = match child {
            BodyContent::Table(table) => take(table, index),
            BodyContent::ContentControl(control) => in_control(control, index),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn nth_table_in_body<'a>(
    content: &'a mut [BodyContent],
    index: &mut usize,
) -> Option<&'a mut CT_Tbl> {
    for child in content {
        let table = match child {
            BodyContent::Table(table) => take_table(table, index),
            BodyContent::ContentControl(control) => nth_table_in_control(control, index),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

fn nth_table_in_control<'a>(control: &'a mut CT_Sdt, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    for child in &mut control.content {
        let table = match child {
            SdtContent::Table(table) => take_table(table, index),
            SdtContent::Cell(cell) => nth_table_in_cell(cell, index),
            SdtContent::ContentControl(control) => nth_table_in_control(control, index),
            SdtContent::Paragraph(_)
            | SdtContent::Row(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

fn nth_table_in_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    for child in &mut cell.content {
        let table = match child {
            CellContent::Table(table) => take_table(table, index),
            CellContent::ContentControl(control) => nth_table_in_control(control, index),
            CellContent::Paragraph(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

// Optional routes from one checked physical inventory. Both source consumers
// keep their original lookup if ordering or containment cannot be qualified.
fn modeled_main_cell_routes(
    owners: &[StoryOwnerSpan],
    direct_items: &[StoryItemSpan],
) -> Option<HashMap<usize, (usize, usize)>> {
    if direct_items
        .windows(2)
        .any(|pair| pair[0].full.end > pair[1].full.start)
    {
        return None;
    }
    let mut routes = HashMap::new();
    let mut ordinals = HashMap::<usize, usize>::new();
    let mut direct = 0;
    let mut previous = None;
    for owner in owners {
        if owner.kind != StoryKind::TableCell {
            continue;
        }
        if previous.is_some_and(|start| start > owner.full.start) {
            return None;
        }
        previous = Some(owner.full.start);
        loop {
            let item = direct_items.get(direct)?;
            if item.full.end <= owner.full.start {
                direct += 1;
                continue;
            }
            if item.full.start > owner.full.start
                || owner.full.end > item.full.end
                || !matches!(
                    item.kind,
                    StoryItemKind::Table | StoryItemKind::ContentControl
                )
            {
                return None;
            }
            let ordinal = ordinals.entry(direct).or_default();
            if routes
                .insert(owner.owner_index, (direct, *ordinal))
                .is_some()
            {
                return None;
            }
            *ordinal = ordinal.checked_add(1)?;
            break;
        }
    }
    Some(routes)
}

fn modeled_main_cell_route(xml: &[u8], target: &StoryOwnerSpan) -> Result<(usize, usize)> {
    let body = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    let direct_items = direct_story_content_items(xml, &body)?;
    let (content_index, direct_item) = direct_items
        .iter()
        .enumerate()
        .find(|(_, item)| item.full.start <= target.full.start && target.full.end <= item.full.end)
        .ok_or_else(|| Error::Other("table-cell owner has no direct body container".to_owned()))?;
    if !matches!(
        direct_item.kind,
        StoryItemKind::Table | StoryItemKind::ContentControl
    ) {
        return Err(Error::Other(
            "HTML fragments require a modeled main-document table-cell owner".to_owned(),
        ));
    }
    let cell_index = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .filter(|owner| {
            owner.kind == StoryKind::TableCell
                && direct_item.full.start <= owner.full.start
                && owner.full.end <= direct_item.full.end
        })
        .position(|owner| owner.full == target.full)
        .ok_or_else(|| Error::Other("modeled table-cell owner was not found".to_owned()))?;
    Ok((content_index, cell_index))
}

/// The body story path of each paragraph of `body`, in
/// [`CT_Body::paragraphs`] order, as [`Document::paragraph_story_location`]
/// resolves it from the body story items `items`, or `None` where it gives
/// no location.
fn body_paragraph_routes(body: &CT_Body, items: &[StoryItemSpan]) -> Vec<Option<Vec<usize>>> {
    let mut targets = HashMap::new();
    for (slot, child) in body.content.iter().enumerate() {
        match child {
            BodyContent::Paragraph(paragraph) => {
                targets
                    .entry(paragraph.as_ref() as *const CT_P)
                    .or_insert((slot, None));
            }
            BodyContent::ContentControl(control) => {
                for (ordinal, paragraph) in control
                    .content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Paragraph(paragraph) => Some(paragraph),
                        _ => None,
                    })
                    .enumerate()
                {
                    targets
                        .entry(paragraph as *const CT_P)
                        .or_insert((slot, Some(ordinal)));
                }
            }
            _ => {}
        }
    }
    let direct = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.direct_owner_child)
        .collect::<Vec<_>>();
    body.paragraphs()
        .map(|paragraph| {
            let (slot, ordinal) = *targets.get(&(paragraph as *const CT_P))?;
            let (item_index, item) = direct.get(slot)?;
            let expected_kind = if ordinal.is_some() {
                StoryItemKind::ContentControl
            } else {
                StoryItemKind::Paragraph
            };
            (item.kind == expected_kind)
                .then(|| std::iter::once(*item_index).chain(ordinal).collect())
        })
        .collect()
}

/// [`Document::cell_paragraph_story_locations`] of `body`, serialized as
/// `xml` with the story owners `owners` in the main part `part_name`, and
/// whether it routed every table cell owner. Like the single lookup, which
/// fails on the first owner it cannot route, the inventory stops there.
#[allow(clippy::type_complexity)] // The located paragraphs of each cell and one fact.
fn cell_paragraph_routes(
    body: &mut CT_Body,
    xml: &[u8],
    owners: &[StoryOwnerSpan],
    part_name: &str,
) -> Result<(HashMap<(usize, usize, usize), Vec<ContentLocation>>, bool)> {
    let mut addresses = HashMap::<*const CT_Tc, (usize, usize, usize)>::new();
    for (table_index, table) in body.tables().enumerate() {
        for (row_index, row) in table.rows.iter().enumerate() {
            for (cell_index, cell) in row.cells.iter().enumerate() {
                addresses.entry(cell as *const CT_Tc).or_insert((
                    table_index,
                    row_index,
                    cell_index,
                ));
            }
        }
    }
    let body_owner = owners
        .iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    let direct_items = direct_story_content_items(xml, body_owner)?;
    let routes = modeled_main_cell_routes(owners, &direct_items);
    let mut ordinals = HashMap::<usize, usize>::new();
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut complete = true;
    for owner in owners
        .iter()
        .filter(|owner| owner.kind == StoryKind::TableCell)
    {
        // The single lookup fails on the first owner it cannot route,
        // whatever its target, so the inventory stops there too.
        let Some((content, direct_item)) = routes
            .as_ref()
            .and_then(|routes| routes.get(&owner.owner_index))
            .and_then(|(content, _)| direct_items.get(*content).map(|item| (*content, item)))
            .or_else(|| {
                direct_items.iter().enumerate().find(|(_, item)| {
                    item.full.start <= owner.full.start && owner.full.end <= item.full.end
                })
            })
        else {
            complete = false;
            break;
        };
        if !matches!(
            direct_item.kind,
            StoryItemKind::Table | StoryItemKind::ContentControl
        ) {
            complete = false;
            break;
        }
        let counter = ordinals.entry(content).or_insert(0);
        let mut ordinal = *counter;
        *counter += 1;
        let modeled = match body.content.get_mut(content) {
            Some(BodyContent::Table(table)) => nth_cell_in_table(table.as_mut(), &mut ordinal),
            Some(BodyContent::ContentControl(control)) => {
                nth_cell_in_control(control.as_mut(), &mut ordinal)
            }
            _ => None,
        };
        let Some((pointer, count)) =
            modeled.map(|cell| (&*cell as *const CT_Tc, cell.paragraphs().len()))
        else {
            continue;
        };
        let Some(address) = addresses.get(&pointer) else {
            continue;
        };
        if !seen.insert(*address) {
            continue;
        }
        let story = StoryId {
            kind: owner.kind,
            part_name: part_name.to_owned(),
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        };
        selected.push((*address, count, (story, owner.clone())));
    }
    let pairs = selected
        .iter()
        .map(|(_, _, pair)| pair.clone())
        .collect::<Vec<_>>();
    let spans = story_range_paragraph_spans(xml, &pairs)?;
    let mut result = HashMap::with_capacity(selected.len());
    for ((address, count, _), mut paragraphs) in selected.into_iter().zip(spans) {
        paragraphs.sort_by_key(|(_, span)| span.start);
        if paragraphs.len() == count {
            result.insert(
                address,
                paragraphs
                    .into_iter()
                    .map(|(location, _)| location)
                    .collect(),
            );
        }
    }
    Ok((result, complete))
}

fn nth_cell_in_control<'a>(control: &'a mut CT_Sdt, index: &mut usize) -> Option<&'a mut CT_Tc> {
    for child in &mut control.content {
        let cell = match child {
            SdtContent::Table(table) => nth_cell_in_table(table, index),
            SdtContent::Row(row) => nth_cell_in_row(row, index),
            SdtContent::Cell(cell) => take_cell(cell, index),
            SdtContent::ContentControl(control) => nth_cell_in_control(control, index),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => None,
        };
        if cell.is_some() {
            return cell;
        }
    }
    None
}

fn nth_cell_in_table<'a>(table: &'a mut CT_Tbl, index: &mut usize) -> Option<&'a mut CT_Tc> {
    let CT_Tbl {
        rows,
        content_controls,
        ..
    } = table;
    let mut selected_control = None;
    let mut selected_row = None;
    for boundary in 0..=rows.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = cell_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(row) = rows.get(boundary) {
            let count = cell_count_in_row(row);
            if *index < count {
                selected_row = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_cell_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(row_index) = selected_row {
        nth_cell_in_row(&mut rows[row_index], index)
    } else {
        None
    }
}

fn nth_cell_in_row<'a>(row: &'a mut CT_Row, index: &mut usize) -> Option<&'a mut CT_Tc> {
    let CT_Row {
        cells,
        content_controls,
        ..
    } = row;
    let mut selected_control = None;
    let mut selected_cell = None;
    for boundary in 0..=cells.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = cell_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(cell) = cells.get(boundary) {
            let count = 1 + cell_count_in_cell(cell);
            if *index < count {
                selected_cell = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_cell_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(cell_index) = selected_cell {
        take_cell(&mut cells[cell_index], index)
    } else {
        None
    }
}

fn take_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_Tc> {
    if *index == 0 {
        Some(cell)
    } else {
        *index -= 1;
        for child in &mut cell.content {
            let nested = match child {
                CellContent::Table(table) => nth_cell_in_table(table, index),
                CellContent::ContentControl(control) => nth_cell_in_control(control, index),
                CellContent::Paragraph(_) => None,
            };
            if nested.is_some() {
                return nested;
            }
        }
        None
    }
}

// Traverse the parsed physical tree once. Coordinates name direct physical
// rows/cells only, while controls still participate in source correspondence.
enum PhysicalCellNode<'a> {
    Table(&'a CT_Tbl),
    Row(&'a CT_Row, usize, Option<usize>),
    Cell(&'a CT_Tc, usize, Option<(usize, usize)>),
    Control(&'a CT_Sdt, usize),
}

#[allow(clippy::type_complexity)]
fn physical_table_cells(table: &CT_Tbl) -> (usize, Vec<(usize, Option<(usize, usize)>)>) {
    let mut stack = vec![PhysicalCellNode::Table(table)];
    let mut tables = 0;
    let mut cells = Vec::new();
    while let Some(node) = stack.pop() {
        let mut children = Vec::new();
        match node {
            PhysicalCellNode::Table(table) => {
                let index = tables;
                tables += 1;
                let mut controls = table.content_controls.iter().peekable();
                for boundary in 0..=table.rows.len() {
                    while controls.peek().is_some_and(|(at, _, _)| *at == boundary) {
                        children.push(PhysicalCellNode::Control(
                            &controls.next().unwrap().2,
                            index,
                        ));
                    }
                    if let Some(row) = table.rows.get(boundary) {
                        children.push(PhysicalCellNode::Row(row, index, Some(boundary)));
                    }
                }
            }
            PhysicalCellNode::Row(row, table, ordinal) => {
                let mut controls = row.content_controls.iter().peekable();
                for boundary in 0..=row.cells.len() {
                    while controls.peek().is_some_and(|(at, _, _)| *at == boundary) {
                        children.push(PhysicalCellNode::Control(
                            &controls.next().unwrap().2,
                            table,
                        ));
                    }
                    if let Some(cell) = row.cells.get(boundary) {
                        children.push(PhysicalCellNode::Cell(
                            cell,
                            table,
                            ordinal.map(|row| (row, boundary)),
                        ));
                    }
                }
            }
            PhysicalCellNode::Cell(cell, table, coordinate) => {
                cells.push((table, coordinate));
                for child in &cell.content {
                    match child {
                        CellContent::Table(table) => children.push(PhysicalCellNode::Table(table)),
                        CellContent::ContentControl(control) => {
                            children.push(PhysicalCellNode::Control(control, table))
                        }
                        CellContent::Paragraph(_) => {}
                    }
                }
            }
            PhysicalCellNode::Control(control, table) => {
                for child in &control.content {
                    match child {
                        SdtContent::Table(table) => children.push(PhysicalCellNode::Table(table)),
                        SdtContent::Row(row) => {
                            children.push(PhysicalCellNode::Row(row, table, None))
                        }
                        SdtContent::Cell(cell) => {
                            children.push(PhysicalCellNode::Cell(cell, table, None))
                        }
                        SdtContent::ContentControl(control) => {
                            children.push(PhysicalCellNode::Control(control, table))
                        }
                        _ => {}
                    }
                }
            }
        }
        stack.extend(children.into_iter().rev());
    }
    (tables, cells)
}

fn cell_count_in_control(control: &CT_Sdt) -> usize {
    control
        .content
        .iter()
        .map(|child| match child {
            SdtContent::Table(table) => cell_count_in_table(table),
            SdtContent::Row(row) => cell_count_in_row(row),
            SdtContent::Cell(cell) => 1 + cell_count_in_cell(cell),
            SdtContent::ContentControl(control) => cell_count_in_control(control),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => 0,
        })
        .sum()
}

fn cell_count_in_table(table: &CT_Tbl) -> usize {
    table
        .content_controls
        .iter()
        .map(|(_, _, control)| cell_count_in_control(control))
        .sum::<usize>()
        + table.rows.iter().map(cell_count_in_row).sum::<usize>()
}

fn cell_count_in_row(row: &CT_Row) -> usize {
    row.content_controls
        .iter()
        .map(|(_, _, control)| cell_count_in_control(control))
        .sum::<usize>()
        + row
            .cells
            .iter()
            .map(|cell| 1 + cell_count_in_cell(cell))
            .sum::<usize>()
}

fn cell_count_in_cell(cell: &CT_Tc) -> usize {
    cell.content
        .iter()
        .map(|child| match child {
            CellContent::Table(table) => cell_count_in_table(table),
            CellContent::ContentControl(control) => cell_count_in_control(control),
            CellContent::Paragraph(_) => 0,
        })
        .sum()
}

fn insert_html_content_into_cell(
    cell: &mut CT_Tc,
    direct_index: usize,
    content: Vec<BodyContent>,
) -> Result<()> {
    let inserted = content
        .into_iter()
        .map(|item| match item {
            BodyContent::Paragraph(paragraph) => Ok(CellContent::Paragraph(*paragraph)),
            BodyContent::Table(table) => Ok(CellContent::Table(*table)),
            BodyContent::ContentControl(control) => Ok(CellContent::ContentControl(*control)),
            BodyContent::RawXml(_) => Err(Error::Other(
                "HTML projection produced unsupported raw cell content".to_owned(),
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    let inserted_len = inserted.len();
    let mut virtual_index = 0_usize;
    let mut insertion = None;
    for content_index in 0..=cell.content.len() {
        for (raw_index, (position, _)) in cell.extra_xml.iter().enumerate() {
            if *position != content_index {
                continue;
            }
            if virtual_index == direct_index {
                insertion = Some((content_index, raw_index));
                break;
            }
            virtual_index += 1;
        }
        if insertion.is_some() {
            break;
        }
        if content_index < cell.content.len() {
            if virtual_index == direct_index {
                let raw_index = cell
                    .extra_xml
                    .iter()
                    .position(|(position, _)| *position > content_index)
                    .unwrap_or(cell.extra_xml.len());
                insertion = Some((content_index, raw_index));
                break;
            }
            virtual_index += 1;
        }
    }
    if virtual_index == direct_index && insertion.is_none() {
        insertion = Some((cell.content.len(), cell.extra_xml.len()));
    }
    let Some((content_index, raw_shift_start)) = insertion else {
        return Err(StoryError::OutOfBounds {
            index: direct_index,
            len: virtual_index,
        }
        .into());
    };
    for (position, _) in cell.extra_xml.iter_mut().skip(raw_shift_start) {
        *position = position
            .checked_add(inserted_len)
            .ok_or_else(|| Error::Other("cell raw-content position overflowed".to_owned()))?;
    }
    cell.content.splice(content_index..content_index, inserted);
    Ok(())
}

pub(crate) fn visit_body_paragraphs<'a>(
    content: &'a [BodyContent],
    accepted: bool,
    visitor: &mut impl FnMut(&'a CT_P),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            BodyContent::Table(table) => visit_table(table, accepted, visitor),
            BodyContent::ContentControl(control) => visit_sdt(control, accepted, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

// With `accepted`, the visitors leave out the rows that accepting every
// tracked change removes, as `CT_Row::accepted_view_removes` reports them.
// `visit_row` itself visits the row it is given.
pub(crate) fn visit_paragraph<'a>(
    paragraph: &'a CT_P,
    accepted: bool,
    visitor: &mut impl FnMut(&'a CT_P),
) {
    visitor(paragraph);
    for (_, _, _, control) in &paragraph.content_controls {
        visit_sdt(control, accepted, visitor);
    }
}

fn visit_table<'a>(table: &'a CT_Tbl, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt(control, accepted, visitor);
        }
        if let Some(row) = table.rows.get(index)
            && !(accepted && row.accepted_view_removes())
        {
            visit_row(row, accepted, visitor);
        }
    }
}

fn visit_row<'a>(row: &'a CT_Row, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt(control, accepted, visitor);
        }
        if let Some(cell) = row.cells.get(index) {
            visit_cell(cell, accepted, visitor);
        }
    }
}

fn visit_cell<'a>(cell: &'a CT_Tc, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            CellContent::Table(table) => visit_table(table, accepted, visitor),
            CellContent::ContentControl(control) => visit_sdt(control, accepted, visitor),
        }
    }
}

fn visit_sdt<'a>(control: &'a CT_Sdt, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            SdtContent::Table(table) => visit_table(table, accepted, visitor),
            SdtContent::Row(row) if accepted && row.accepted_view_removes() => {}
            SdtContent::Row(row) => visit_row(row, accepted, visitor),
            SdtContent::Cell(cell) => visit_cell(cell, accepted, visitor),
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => visit_sdt(nested, accepted, visitor),
        }
    }
}

/// Collect the body paragraphs in document order, those that body-level
/// content controls wrap included. Tables are not entered, with or without a
/// control around them. This is what [`Document::headings`] and
/// [`Document::links`] read.
fn block_paragraphs(content: &[BodyContent]) -> Vec<&CT_P> {
    let mut paragraphs = Vec::new();
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => paragraphs.push(paragraph.as_ref()),
            BodyContent::ContentControl(control) => {
                collect_block_control_paragraphs(control, &mut paragraphs);
            }
            BodyContent::Table(_) | BodyContent::RawXml(_) => {}
        }
    }
    paragraphs
}

fn collect_block_control_paragraphs<'a>(control: &'a CT_Sdt, paragraphs: &mut Vec<&'a CT_P>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => paragraphs.push(paragraph),
            SdtContent::ContentControl(nested) => {
                collect_block_control_paragraphs(nested, paragraphs);
            }
            SdtContent::Table(_)
            | SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

/// Append a block-level paragraph to [`Document::text`] as one line.
fn push_paragraph_line(paragraph: &CT_P, text: &mut String) {
    text.push_str(&ParagraphRef { inner: paragraph }.text());
    text.push('\n');
}

/// Append a table to [`Document::text`] as one line per row, rows wrapped by
/// table-level content controls included.
fn push_table_text(table: &CT_Tbl, text: &mut String) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            push_control_text(control, text);
        }
        if let Some(row) = table.rows.get(index) {
            push_row_text(row, text);
        }
    }
}

/// Append a row to [`Document::text`] as one line in which every paragraph of
/// its cells ends with a tab, cell controls and nested tables included.
fn push_row_text(row: &CT_Row, text: &mut String) {
    if row.accepted_view_removes() {
        return;
    }
    visit_row(row, true, &mut |paragraph| {
        push_cell_paragraph(paragraph, text)
    });
    text.push('\n');
}

fn push_cell_paragraph(paragraph: &CT_P, text: &mut String) {
    text.push_str(&ParagraphRef { inner: paragraph }.text());
    text.push('\t');
}

/// Append what a block, table or row content control wraps to [`Document::text`].
fn push_control_text(control: &CT_Sdt, text: &mut String) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => push_paragraph_line(paragraph, text),
            SdtContent::Table(table) => push_table_text(table, text),
            SdtContent::Row(row) => push_row_text(row, text),
            SdtContent::Cell(cell) => {
                visit_cell(cell, true, &mut |paragraph| {
                    push_cell_paragraph(paragraph, text)
                });
            }
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => push_control_text(nested, text),
        }
    }
}

pub(crate) fn visit_body_paragraphs_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_P),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            BodyContent::Table(table) => visit_table_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_mut(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

/// Visit the paragraphs of the body's table cells in document order, nested
/// tables, tables inside block controls and controls around rows or cells
/// included, as [`visit_body_paragraphs_mut`] reaches them.
pub(crate) fn visit_body_table_paragraphs_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_P),
) {
    for item in content {
        match item {
            BodyContent::Table(table) => visit_table_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_tables_mut(control, visitor),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => {}
        }
    }
}

fn visit_sdt_tables_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut control.content {
        match item {
            SdtContent::Table(table) => visit_table_mut(table, visitor),
            SdtContent::Row(row) => visit_row_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_mut(cell, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_tables_mut(nested, visitor),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => {}
        }
    }
}

fn visit_paragraph_mut(paragraph: &mut CT_P, visitor: &mut impl FnMut(&mut CT_P)) {
    visitor(paragraph);
    for (_, _, _, control) in &mut paragraph.content_controls {
        visit_sdt_mut(control, visitor);
    }
}

fn visit_table_mut(table: &mut CT_Tbl, visitor: &mut impl FnMut(&mut CT_P)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_mut(control, visitor);
        }
        if let Some(row) = table.rows.get_mut(index) {
            visit_row_mut(row, visitor);
        }
    }
}

fn visit_row_mut(row: &mut CT_Row, visitor: &mut impl FnMut(&mut CT_P)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_mut(control, visitor);
        }
        if let Some(cell) = row.cells.get_mut(index) {
            visit_cell_mut(cell, visitor);
        }
    }
}

fn visit_cell_mut(cell: &mut CT_Tc, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            CellContent::Table(table) => visit_table_mut(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_mut(control, visitor),
        }
    }
}

fn visit_sdt_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            SdtContent::Table(table) => visit_table_mut(table, visitor),
            SdtContent::Row(row) => visit_row_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_mut(cell, visitor),
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => visit_sdt_mut(nested, visitor),
        }
    }
}

fn collect_relationship_ids(content: &[BodyContent], output: &mut Vec<String>) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => {
                collect_paragraph_relationship_ids(paragraph, output)
            }
            BodyContent::Table(table) => collect_table_relationship_ids(table, output),
            BodyContent::ContentControl(control) => collect_sdt_relationship_ids(control, output),
            BodyContent::RawXml(_) => {}
        }
    }
}

fn collect_paragraph_relationship_ids(paragraph: &CT_P, output: &mut Vec<String>) {
    if let Some(section) = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref())
    {
        for reference in section.header_refs.iter().chain(&section.footer_refs) {
            if !output.contains(&reference.rel_id) {
                output.push(reference.rel_id.clone());
            }
        }
    }
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == index)
        {
            collect_sdt_relationship_ids(control, output);
        }
        for hyperlink in paragraph
            .hyperlinks
            .iter()
            .filter(|hyperlink| hyperlink.run_start == index)
        {
            if let Some(id) = &hyperlink.rel_id
                && !output.contains(id)
            {
                output.push(id.clone());
            }
        }
        let Some(run) = paragraph.runs.get(index) else {
            continue;
        };
        for content in &run.content {
            let RunContent::Drawing(drawing) = content else {
                continue;
            };
            let id = drawing
                .inline
                .as_ref()
                .filter(|drawing| drawing.raw_xml.is_none())
                .map(|drawing| drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id))
                .or_else(|| {
                    drawing
                        .anchor
                        .as_ref()
                        .filter(|drawing| drawing.raw_xml.is_none())
                        .map(|drawing| drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id))
                });
            if let Some(id) = id
                && !output.contains(id)
            {
                output.push(id.clone());
            }
        }
    }
}

fn collect_table_relationship_ids(table: &CT_Tbl, output: &mut Vec<String>) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            collect_sdt_relationship_ids(control, output);
        }
        if let Some(row) = table.rows.get(index) {
            for cell_index in 0..=row.cells.len() {
                for (_, _, control) in row
                    .content_controls
                    .iter()
                    .filter(|(at, _, _)| *at == cell_index)
                {
                    collect_sdt_relationship_ids(control, output);
                }
                if let Some(cell) = row.cells.get(cell_index) {
                    for item in &cell.content {
                        match item {
                            CellContent::Paragraph(paragraph) => {
                                collect_paragraph_relationship_ids(paragraph, output)
                            }
                            CellContent::Table(table) => {
                                collect_table_relationship_ids(table, output)
                            }
                            CellContent::ContentControl(control) => {
                                collect_sdt_relationship_ids(control, output)
                            }
                        }
                    }
                }
            }
        }
    }
}

fn collect_sdt_relationship_ids(control: &CT_Sdt, output: &mut Vec<String>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => {
                collect_paragraph_relationship_ids(paragraph, output)
            }
            SdtContent::Table(table) => collect_table_relationship_ids(table, output),
            SdtContent::Row(row) => {
                let table = CT_Tbl {
                    properties: None,
                    grid: None,
                    rows: vec![row.clone()],
                    extra_xml: Vec::new(),
                    content_controls: Vec::new(),
                };
                collect_table_relationship_ids(&table, output);
            }
            SdtContent::Cell(cell) => {
                for item in &cell.content {
                    match item {
                        CellContent::Paragraph(paragraph) => {
                            collect_paragraph_relationship_ids(paragraph, output)
                        }
                        CellContent::Table(table) => collect_table_relationship_ids(table, output),
                        CellContent::ContentControl(control) => {
                            collect_sdt_relationship_ids(control, output)
                        }
                    }
                }
            }
            SdtContent::ContentControl(nested) => collect_sdt_relationship_ids(nested, output),
            SdtContent::Run(run) => {
                for content in &run.content {
                    if let RunContent::Drawing(drawing) = content
                        && let Some(id) = drawing
                            .inline
                            .as_ref()
                            .filter(|drawing| drawing.raw_xml.is_none())
                            .map(|drawing| {
                                drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id)
                            })
                            .or_else(|| {
                                drawing
                                    .anchor
                                    .as_ref()
                                    .filter(|drawing| drawing.raw_xml.is_none())
                                    .map(|drawing| {
                                        drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id)
                                    })
                            })
                        && !output.contains(id)
                    {
                        output.push(id.clone());
                    }
                }
            }
            SdtContent::RawXml(_) => {}
        }
    }
}

/// A body child as the ODT and RTF writers export it.
pub(crate) enum ExportItem<'a> {
    Paragraph(&'a CT_P),
    /// A table, with the rows, cells and cell content that content controls
    /// wrap in place, see [`unwrap_table_controls`].
    Table(Box<Cow<'a, CT_Tbl>>),
    /// A block content control. What it wraps follows it.
    ContentControl,
    RawXml,
}

/// The body children in document order with their source paths, `body[i]`.
/// A block content control is transparent: what it wraps follows it at
/// `{its path}/content[k]`, nested controls included.
pub(crate) fn body_export_items(content: &[BodyContent]) -> Vec<(ExportItem<'_>, String)> {
    let mut items = Vec::new();
    for (index, item) in content.iter().enumerate() {
        let path = format!("body[{index}]");
        match item {
            BodyContent::Paragraph(paragraph) => {
                items.push((ExportItem::Paragraph(paragraph), path))
            }
            BodyContent::Table(table) => items.push((
                ExportItem::Table(Box::new(unwrap_table_controls(table))),
                path,
            )),
            BodyContent::ContentControl(control) => {
                push_control_export_items(control, path, &mut items)
            }
            BodyContent::RawXml(_) => items.push((ExportItem::RawXml, path)),
        }
    }
    items
}

fn push_control_export_items<'a>(
    control: &'a CT_Sdt,
    path: String,
    items: &mut Vec<(ExportItem<'a>, String)>,
) {
    items.push((ExportItem::ContentControl, path.clone()));
    for (index, item) in control.content.iter().enumerate() {
        let path = format!("{path}/content[{index}]");
        match item {
            SdtContent::Paragraph(paragraph) => {
                items.push((ExportItem::Paragraph(paragraph), path))
            }
            SdtContent::Table(table) => items.push((
                ExportItem::Table(Box::new(unwrap_table_controls(table))),
                path,
            )),
            SdtContent::ContentControl(nested) => push_control_export_items(nested, path, items),
            // A control around a paragraph or a table holds no rows, cells or
            // runs of its own.
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::Run(_) => {}
            SdtContent::RawXml(_) => items.push((ExportItem::RawXml, path)),
        }
    }
}

/// Whether a table, not counting the tables nested in its cells, holds a
/// content control around a row, a cell or cell content.
fn table_has_content_controls(table: &CT_Tbl) -> bool {
    !table.content_controls.is_empty()
        || table.rows.iter().any(|row| {
            !row.content_controls.is_empty()
                || row.cells.iter().any(|cell| {
                    cell.content
                        .iter()
                        .any(|content| matches!(content, CellContent::ContentControl(_)))
                })
        })
}

/// The table with the rows, cells and cell content that its content controls
/// wrap in place of the controls, nested ones included. Borrowed when the
/// table has no such control.
pub(crate) fn unwrap_table_controls(table: &CT_Tbl) -> Cow<'_, CT_Tbl> {
    if !table_has_content_controls(table) {
        return Cow::Borrowed(table);
    }
    let rows = table
        .rows()
        .into_iter()
        .map(|row| CT_Row {
            table_property_exception: row.table_property_exception.clone(),
            properties: row.properties.clone(),
            cells: row
                .cells()
                .into_iter()
                .map(|cell| {
                    let mut content = Vec::with_capacity(cell.content.len());
                    for item in &cell.content {
                        match item {
                            CellContent::ContentControl(control) => {
                                push_control_cell_content(control, &mut content)
                            }
                            item => content.push(item.clone()),
                        }
                    }
                    CT_Tc {
                        properties: cell.properties.clone(),
                        content,
                        extra_xml: cell.extra_xml.clone(),
                    }
                })
                .collect(),
            extra_xml: row.extra_xml.clone(),
            content_controls: Vec::new(),
        })
        .collect();
    Cow::Owned(CT_Tbl {
        properties: table.properties.clone(),
        grid: table.grid.clone(),
        rows,
        extra_xml: table.extra_xml.clone(),
        content_controls: Vec::new(),
    })
}

fn push_control_cell_content(control: &CT_Sdt, content: &mut Vec<CellContent>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => {
                content.push(CellContent::Paragraph(paragraph.clone()))
            }
            SdtContent::Table(table) => content.push(CellContent::Table(table.clone())),
            SdtContent::ContentControl(nested) => push_control_cell_content(nested, content),
            SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

/// Visit the drawings the exporters write, in the order they write them: those
/// of the accepted view of every paragraph (see `CT_P::accepted_view`), in
/// the paragraphs [`visit_body_paragraphs`] reaches.
pub(crate) fn visit_accepted_drawings(
    content: &[BodyContent],
    visitor: &mut impl FnMut(&CT_Drawing),
) {
    visit_body_paragraphs(content, true, &mut |paragraph| {
        for run in &paragraph.accepted_view().runs {
            visit_run_drawings(run, visitor);
        }
    });
}

pub(crate) fn visit_all_drawings(content: &[BodyContent], visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            BodyContent::Table(table) => visit_table_drawings(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_drawings(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

pub(crate) fn visit_all_drawings_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_Drawing),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            BodyContent::Table(table) => visit_table_drawings_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_drawings_mut(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

fn visit_authored_drawings(content: &[BodyContent], visitor: &mut impl FnMut(&CT_Drawing)) {
    visit_all_drawings(content, &mut |drawing| {
        if authored_drawing(drawing) {
            visitor(drawing);
        }
    });
}

fn visit_authored_drawings_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_Drawing),
) {
    visit_all_drawings_mut(content, &mut |drawing| {
        if authored_drawing(drawing) {
            visitor(drawing);
        }
    });
}

fn close_typed_story_drawing_namespaces(paragraph: &mut CT_P) -> Result<()> {
    let scope = BTreeMap::from([
        ("r".to_owned(), drawing_ns::R.to_owned()),
        ("wp".to_owned(), drawing_ns::WP.to_owned()),
    ]);
    let mut error = None;
    visit_paragraph_drawings_mut(paragraph, &mut |drawing| {
        if error.is_some() || !authored_drawing(drawing) {
            return;
        }
        if let Some(inline) = &mut drawing.inline {
            let mut writer = Writer::new(Vec::new());
            if let Err(current) = inline.to_xml(&mut writer).map_err(Error::from) {
                error = Some(current);
                return;
            }
            match close_content_fragment_namespaces(&writer.into_inner(), &scope) {
                Ok(xml) => inline.raw_xml = Some(xml),
                Err(current) => error = Some(current),
            }
        } else if let Some(anchor) = &mut drawing.anchor {
            let mut writer = Writer::new(Vec::new());
            if let Err(current) = anchor.to_xml(&mut writer).map_err(Error::from) {
                error = Some(current);
                return;
            }
            match close_content_fragment_namespaces(&writer.into_inner(), &scope) {
                Ok(xml) => anchor.raw_xml = Some(xml),
                Err(current) => error = Some(current),
            }
        }
    });
    error.map_or(Ok(()), Err)
}

fn authored_drawing(drawing: &CT_Drawing) -> bool {
    drawing
        .inline
        .as_ref()
        .is_some_and(|value| value.raw_xml.is_none())
        || drawing
            .anchor
            .as_ref()
            .is_some_and(|value| value.raw_xml.is_none())
}

fn visit_run_drawings(run: &CT_R, visitor: &mut impl FnMut(&CT_Drawing)) {
    for content in &run.content {
        if let RunContent::Drawing(drawing) = content {
            visitor(drawing);
        }
    }
}

fn visit_run_drawings_mut(run: &mut CT_R, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for content in &mut run.content {
        if let RunContent::Drawing(drawing) = content {
            visitor(drawing);
        }
    }
}

fn visit_paragraph_drawings(paragraph: &CT_P, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(run) = paragraph.runs.get(index) {
            visit_run_drawings(run, visitor);
        }
    }
}

fn visit_paragraph_drawings_mut(paragraph: &mut CT_P, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter_mut()
            .filter(|(at, _, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(run) = paragraph.runs.get_mut(index) {
            visit_run_drawings_mut(run, visitor);
        }
    }
}

fn visit_table_drawings(table: &CT_Tbl, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(row) = table.rows.get(index) {
            visit_row_drawings(row, visitor);
        }
    }
}

fn visit_table_drawings_mut(table: &mut CT_Tbl, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(row) = table.rows.get_mut(index) {
            visit_row_drawings_mut(row, visitor);
        }
    }
}

fn visit_row_drawings(row: &CT_Row, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(cell) = row.cells.get(index) {
            visit_cell_drawings(cell, visitor);
        }
    }
}

fn visit_row_drawings_mut(row: &mut CT_Row, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(cell) = row.cells.get_mut(index) {
            visit_cell_drawings_mut(cell, visitor);
        }
    }
}

fn visit_cell_drawings(cell: &CT_Tc, visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            CellContent::Table(table) => visit_table_drawings(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_drawings(control, visitor),
        }
    }
}

fn visit_cell_drawings_mut(cell: &mut CT_Tc, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for item in &mut cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            CellContent::Table(table) => visit_table_drawings_mut(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_drawings_mut(control, visitor),
        }
    }
}

fn visit_sdt_drawings(control: &CT_Sdt, visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            SdtContent::Table(table) => visit_table_drawings(table, visitor),
            SdtContent::Row(row) => visit_row_drawings(row, visitor),
            SdtContent::Cell(cell) => visit_cell_drawings(cell, visitor),
            SdtContent::Run(run) => visit_run_drawings(run, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_drawings(nested, visitor),
            SdtContent::RawXml(_) => {}
        }
    }
}

fn visit_sdt_drawings_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for item in &mut control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            SdtContent::Table(table) => visit_table_drawings_mut(table, visitor),
            SdtContent::Row(row) => visit_row_drawings_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_drawings_mut(cell, visitor),
            SdtContent::Run(run) => visit_run_drawings_mut(run, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_drawings_mut(nested, visitor),
            SdtContent::RawXml(_) => {}
        }
    }
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
