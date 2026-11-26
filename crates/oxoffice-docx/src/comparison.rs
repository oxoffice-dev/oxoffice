//! Deterministic native document comparison and tracked-revision generation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use base64::Engine;
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{BodyContent, CT_Document};
use oxoffice_docx_oxml::namespace::{R_NS, W_NS};
use oxoffice_docx_oxml::properties::CT_PPr;
use oxoffice_docx_oxml::shared::ST_PageOrientation;
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_TblPr, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::{
    CT_P, CT_R, CT_Text, CommentRangeMarker, RunContent, declare_w14_on_part_root,
};
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Reader, Writer, XmlVersion};
use sha2::{Digest, Sha256};

use crate::revision::{FieldEvent, paragraph_field_events, validate_revision_timestamp};
use crate::{Document, Error, Result, StoryKind};

type NamespaceBindings = Vec<(String, String)>;
/// The target of each hyperlink relationship, by story part and id, with
/// whether it is external.
type HyperlinkTargets = HashMap<String, HashMap<String, (String, bool)>>;
/// The attributes of a link's start tag, named as written (another prefix
/// than `w` included), in order, with the value of those no `HYPERLINK`
/// switch carries.
type HyperlinkOpening = Vec<(String, Option<String>)>;
type SourceCoordinates = (usize, Vec<(Range<usize>, Range<usize>)>);
type RawSourceSelection = (usize, usize, usize);
type RunSourceSelection<'a> = (&'a [RawSourceSelection], &'a [Option<TypedSourceSelection>]);
type PublishedRunRole = (Range<usize>, Option<usize>, Option<TypedSourceSelection>);
type SourceBudget = Vec<(Vec<(u8, usize)>, Vec<String>)>;
type SourceBudgetHole = (Vec<(u8, usize)>, u8, Range<usize>, NamespaceBindings);
type OriginalBodySource<'a> = (&'a str, &'a [Range<usize>], &'a [(String, String)]);
type PropertyChildren = Vec<((String, usize), Range<usize>)>;

use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::{Relationship, rel_types};

mod comments;
mod footnotes;

pub(crate) use comments::{
    COMMENT_COMPARISON_CLOSE, COMMENT_COMPARISON_OPEN, COMMENT_COMPARISON_PART,
    COMMENT_COMPARISON_REL, apply_comment_snapshot,
};
use comments::{
    comment_package_links_match, comment_snapshot, comment_snapshots_match,
    extended_comment_snapshots_match, remap_comment_targets,
};
pub(crate) use footnotes::{
    NoteKind, RawNoteType, note_part_name, raw_note_attribute_in_scope, raw_note_type_in_scope,
    remove_selected_note_definitions, resolved_note_kinds, settle_note_custody,
    validate_note_custody,
};
use footnotes::{
    RenumberedNotes, compare_note_parts, normal_note_owner, note_custody_reserved_ids,
    note_markers, preflight_note_inputs, reconcile_note_custody_inputs, reconcile_note_ids,
    refuse_shared_note_references, renumbered_notes, retire_completed_note_inputs,
    retire_note_markers, verify_note_destination,
};

mod bookmarks;
mod field_conversions;
mod guard;
mod move_pairs;
mod namespaces;
mod postconditions;
mod relationships;
mod source_metadata;
mod source_scopes;
mod stories;
mod text_boxes;

use bookmarks::{
    bookmark_coverage, holds_bookmarks_once, reconcile_body_bookmarks, rewritten_opening,
    shared_bookmark_ids, without_bookmarks, word_attributes, zero_width_bookmark_ids,
};
use field_conversions::{restore_whole_hyperlinks, validate_field_conversion_role};
use guard::{
    guard_mask_coordinates, guard_source_body_at, guard_story_owners, guard_story_part,
    normalized_story_part,
};
use move_pairs::{crossing_reply_additions, pair_moves, share_trailing_move};
use namespaces::{
    actual_root_bindings, declared_prefixes, drawing_signature, element_inner_range_any_prefix,
    foreign_word_binding, required_external_bindings, root_prefix,
};
pub(crate) use postconditions::reopen_staged;
use postconditions::{
    NormalizedPackage, comparison_postcondition_error, normalized_package,
    refuse_changed_existing_revisions, resolved_candidate, text_in_deletions,
};
use relationships::{
    chart_relationship_signature, hyperlink_targets, relationship_classes,
    remap_equivalent_story_relationships,
};
use source_metadata::{
    ContentMetadata, field_source_metadata, source_root_metadata,
    source_without_run_root_attributes, source_without_text_properties,
};
use source_scopes::{
    ParentScopes, canonical_owned_story, child_scopes, publication_parent_at,
    publication_parents_at, root_inner_range, root_namespace_bindings, same_held_owner,
    same_owner_tree, same_written_owner, source_child, source_children, source_fragment,
    story_skeleton,
};
pub(crate) use stories::revision_story_parts;
use stories::{
    compare_story_inner, compare_story_inner_impl, compare_story_part, comparison_input,
    contains_modeled_revisions, reject_cross_story_moves, story_content_spans, story_document,
    story_ignored, story_parts, story_parts_with_options, story_wrapper, story_xml,
    validate_comparison_options,
};
use text_boxes::{
    RunHostRole, comparison_text_box_markers, mask_text_box_subtrees,
    modeled_revision_count_with_options, paired_run_replacement, policy_textbox_intervals,
    private_text_box_host_spans, replace_text_boxes_in_hosts, restore_text_box_subtrees,
    text_box_coordinates, text_box_host_skeletons, text_box_host_spans, text_box_marker_local,
    text_box_spans, text_boxes_changed_inside, without_text_box_subtrees, word_ids_with_options,
};

mod alignment;
mod leaf_publications;
mod property_sources;
mod run_properties;
mod run_source;
mod signatures;
mod source_snapshots;
mod xml;

use run_source::{
    raw_occurrence_representation, resolved_source_bindings, run_content_source_width,
    run_source_events, same_raw_occurrence,
};

use alignment::{
    Alignment, align, pair_gaps_by_kind, replace_changed_paragraph_runs, uncarried_field_owners,
    validate_field_alignment,
};
use leaf_publications::{
    complete_run_selection, emitted_fragment_leaves, preserve_run_source,
    restore_resolved_leaf_occurrences, shift_leaf_publications, source_content_metadata,
    source_content_model, splice_leaf_publications,
};
use property_sources::{
    direct_property, modeled_control_source_spans, patch_property_source,
    preserve_prior_property_sources, replace_property_slot, without_property_children,
};
use run_properties::{
    modeled_run_properties, preserve_unmodeled_run_properties, run_property_xml,
    unmodeled_run_properties_differ,
};
use signatures::{
    RelationshipClassScope, body_signature, body_signature_with_options, cell_formatting,
    coalesced_postcondition_body, control_content_signature_with_options,
    control_metadata_diagnostics, control_property_signature, control_signature_with_options,
    control_whitespace_slots, has_edge_whitespace, modeled_control_content,
    normalized_body_with_options, paragraph_formatting, paragraph_signature_with_options,
    postcondition_body, row_formatting, row_signature_with_options, run_content_signature,
    run_signature, run_signature_with_options, semantic_run_raw, table_signature_with_options,
};
use xml::{
    append_word_child, body_content_xml, cell_child_spans, cell_xml, control_xml, deleted_run,
    deleted_run_xml, detach_field_spans, direct_element_spans, direct_word_element_spans,
    direct_word_element_spans_in_scope, element_local_name, element_opening, extract_body_inner,
    insert_first_child, main_body_source, modeled_paragraph_run_spans, paragraph_close_start,
    paragraph_owned_run_xml, paragraph_run_spans, paragraph_xml, property_xml, replace_body_inner,
    replace_direct_word_elements, replace_element_inner, replace_owner_inner,
    replace_paragraph_run_elements, replace_ranges, row_xml, run_xml, section_property_xml,
    table_xml, utf8_error, with_mark_revision, word_element_starts, word_ids,
};

mod attributed_runs;
mod body;
mod controls;
mod fields;
mod granular;
mod granular_units;
mod numbering;
mod paragraph_marks;
mod paragraph_properties;
mod paragraphs;
mod projection;
mod tables;

use attributed_runs::{
    attributed_run_signature, attributed_run_units, attributed_unit_signature, granular_text,
    ignored_run_content, run_is_ignored, validate_raw_attribution,
};
use body::{
    body_location, compare_body, moved_body_content, moved_paragraph, moved_table,
    restore_current_property_roots, restore_extracted_root, retain_source_root_namespaces,
};
use controls::{compare_control, compare_control_from_xml, marked_control};
use fields::{
    actual_field_is_complex, actual_field_result_range, actual_field_source_fragment,
    compared_run_xml, comparison_instruction_eq, containing_run, conversion_namespace_facts,
    conversion_source_inventory, field_carrier_source_budget, field_result_source_paragraph,
    field_source_sequence_budget, prepared_field_result_owner, publish_run_source, run_field,
    run_is_field, whole_paragraph_hyperlinks, whole_simple_field,
};
use granular::{
    GranularAction, align_between_shells, compare_granular_paragraph, granular_action,
    hyperlink_raw_boundaries, hyperlink_shells, policy_run_boundary, run_formatting,
    shell_run_boundaries, shell_unit_cuts, unit_is_empty, unit_is_ignorable,
    validate_fragment_partition,
};
use granular_units::{
    compare_granular_controls, grouped_text_alignment, interleave_granular_paragraph,
    merge_unit_runs, replace_paragraph_properties_and_runs, unit_plain_text, unit_text_element,
};
use paragraph_marks::{
    EmittedOwner, after_kept_table, final_paragraph, mark_before_final_paragraph,
    mark_previous_paragraph, marked_paragraph_properties_xml, marked_paragraph_xml,
    next_kept_owner, previous_mark_position, trailing_paragraph, trailing_paragraphs,
    unmatched_body_owner,
};
use paragraph_properties::{
    clear_default_orientation, compared_paragraph_properties, deleted_paragraph,
    final_paragraph_properties_xml, inserted_paragraph, modeled_paragraph_properties,
    nonempty_paragraph_properties, paragraph_children_xml, paragraph_properties_differ,
    paragraph_properties_xml, place_shared_bookmarks, same_xml_fragment, section_properties_xml,
    shared_bookmark_markers,
};
use paragraphs::{
    coalesced_held_owner_equal, coalesced_paragraphs_equal, compare_body_content,
    compare_paragraph, deleted_body_content, inserted_body_content, paragraph_boundaries_differ,
    paragraph_shell, uses_attributed_run_path,
};
use projection::{
    attach_projection_actions, coalesced_granular_alignment, projection_action_ledgers,
    projection_leaf_pairs, projection_unit_alignment, record_projection_actions,
    retain_projection_separators, validate_projection_actions,
};
use tables::{
    compare_cell_from_xml, compare_row, compare_table, marked_cell_xml, marked_row, marked_table,
    row_raw_children,
};

/// A comparison difference that cannot be represented as a content revision.
///
/// The redline keeps the original for every diagnostic. The message starts
/// with a stable prefix naming the difference: `formatting differs` for
/// formatting that cannot be revised, and `content-control <name> differs`
/// for a control's `tag`, `alias`, `lock`, `placeholder` or
/// `docPartGallery`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonDiagnostic {
    pub location: String,
    pub message: String,
}

fn formatting_diagnostic(diagnostics: &mut Vec<ComparisonDiagnostic>, location: String) {
    diagnostics.push(ComparisonDiagnostic {
        location,
        message: "formatting differs and the original formatting was retained".to_owned(),
    });
}

/// A Word story category that can be excluded from document comparison.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComparisonStoryKind {
    Main,
    Header,
    Footer,
    Comment,
    TextBox,
    Footnote,
    Endnote,
}

/// The text boundary used when generating content revisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ComparisonGranularity {
    /// Preserve the legacy whole-run comparison behavior.
    #[default]
    Run,
    /// Compare maximal word, whitespace, and punctuation or symbol units.
    Word,
    /// Compare individual Unicode scalar values.
    Character,
}

/// Policy controls for native document comparison.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComparisonOptions {
    pub granularity: ComparisonGranularity,
    pub ignore_formatting: bool,
    pub ignore_whitespace: bool,
    pub ignore_fields: bool,
    pub ignore_comments: bool,
    pub ignored_stories: Vec<ComparisonStoryKind>,
}

#[derive(Clone, Debug)]
struct FieldConversionWitness {
    sources: [FieldSourceOccurrence; 2],
    instruction_changed: bool,
    form_changed: bool,
    input_complex: [bool; 2],
    revision_ids: [i32; 2],
    generated_spans: [String; 2],
}

/// A simple field written as one complex field with the paragraph that
/// holds it, inserted or deleted whole. Only the input that keeps the
/// paragraph holds the field: the witness names that occurrence and the one
/// revision id of the wrapper that publishes the whole field.
#[derive(Clone, Debug)]
struct WholeFieldWitness {
    source: FieldSourceOccurrence,
    revision_id: i32,
    generated_span: String,
}

/// A hyperlink written as one `HYPERLINK` complex field with the paragraph
/// that holds it, inserted or deleted whole (see `whole_paragraph_hyperlinks`).
/// The resolution that keeps the paragraph reads the link back from the field
/// alone: `opening` names the attributes the input link has, in its order,
/// with the values no field switch carries: its namespace declarations and
/// `w:history`, which Word drops and writes back too.
#[derive(Clone, Debug)]
struct WholeHyperlinkWitness {
    owner: String,
    input_role: usize,
    revision_id: i32,
    generated_span: String,
    opening: HyperlinkOpening,
}

/// What the whole-paragraph hyperlink publisher needs: the target of each
/// hyperlink relationship by story part and id, the original story part being
/// compared, and the witnesses it wrote.
#[derive(Clone, Debug, Default)]
struct WholeHyperlinks {
    targets: std::rc::Rc<HyperlinkTargets>,
    owner: String,
    witnesses: Vec<WholeHyperlinkWitness>,
}

impl WholeHyperlinks {
    /// The same targets and part, without witnesses.
    fn scope(&self) -> Self {
        Self {
            targets: self.targets.clone(),
            owner: self.owner.clone(),
            witnesses: Vec::new(),
        }
    }
}

struct Metadata<'a> {
    author: &'a str,
    timestamp: &'a str,
    options: &'a ComparisonOptions,
    ids: IdAllocator,
    /// Whether an input holds a modeled revision, which the comparison
    /// keeps only in owners that both inputs hold unchanged.
    existing_revisions: bool,
    source_frames: Vec<SourceSnapshots>,
    projected_textbox_noop: bool,
    leaf_capture: bool,
    /// Set only inside the checked paired-host source/interval publisher.
    paired_field_story: Option<String>,
    leaf_publications: Vec<LeafPublication>,
    field_conversions: Vec<FieldConversionWitness>,
    whole_field_conversions: Vec<WholeFieldWitness>,
    /// The ids of the bookmarks both inputs hold (see
    /// [`shared_bookmark_ids`]), which a whole deleted or inserted paragraph
    /// writes once, untracked, where the edited input has them.
    shared_bookmarks: HashSet<i32>,
    /// Those of them the edited input holds as a point (see
    /// [`zero_width_bookmark_ids`]), which a whole inserted paragraph writes
    /// whole.
    point_bookmarks: HashSet<i32>,
    /// Those of them a whole paragraph wrote that way, whose start or end
    /// the acceptance or the rejection leaves at a paragraph edge.
    relocated_bookmarks: HashSet<i32>,
    whole_hyperlinks: WholeHyperlinks,
}

#[derive(Clone)]
struct IdAllocator {
    used: HashSet<i32>,
    next: i32,
    /// Producer w bindings stay untouched. Newly written Word roots carry a
    /// local binding when that prefix has a different source meaning.
    local_word_binding: bool,
}

impl ComparisonStoryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Footer => "footer",
            Self::Comment => "comments",
            Self::Footnote => "footnotes",
            Self::Endnote => "endnotes",
            Self::Main => "body",
            Self::TextBox => "text-box",
        }
    }

    fn root_local(self) -> &'static str {
        match self {
            Self::Header => "hdr",
            Self::Footer => "ftr",
            Self::Comment => "comments",
            Self::Footnote => "footnotes",
            Self::Endnote => "endnotes",
            Self::Main | Self::TextBox => "document",
        }
    }

    fn owner_local(self) -> Option<&'static str> {
        match self {
            Self::Comment => Some("comment"),
            Self::Footnote => Some("footnote"),
            Self::Endnote => Some("endnote"),
            Self::Main | Self::Header | Self::Footer | Self::TextBox => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoryPart {
    kind: ComparisonStoryKind,
    part_name: String,
}

#[derive(Default)]
struct TextBoxMarkers {
    main: String,
    related: HashMap<String, String>,
}

impl IdAllocator {
    fn new(used: HashSet<i32>) -> Self {
        Self {
            used,
            next: 0,
            local_word_binding: false,
        }
    }

    fn generated_bindings(&self) -> BTreeMap<String, String> {
        if self.local_word_binding {
            BTreeMap::from([("w".to_owned(), W_NS.to_owned())])
        } else {
            BTreeMap::new()
        }
    }

    fn revision_parent_scope(&self, parent: &NamespaceBindings) -> NamespaceBindings {
        let mut scope = parent.clone();
        for (prefix, uri) in self.generated_bindings() {
            let key = format!("xmlns:{prefix}");
            scope.retain(|(name, _)| name != &key);
            scope.push((key, uri));
        }
        scope
    }

    fn generated(&self, xml: String) -> Result<String> {
        let scope = self.generated_bindings();
        if scope.is_empty() || xml.is_empty() {
            return Ok(xml);
        }
        String::from_utf8(crate::document::close_content_fragment_namespaces(
            xml.as_bytes(),
            &scope,
        )?)
        .map_err(utf8_error)
    }

    fn allocate(&mut self) -> Result<i32> {
        while self.used.contains(&self.next) {
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| Error::Other("comparison revision ids are exhausted".to_owned()))?;
        }
        let id = self.next;
        self.used.insert(id);
        self.next = self.next.saturating_add(1);
        Ok(id)
    }

    fn allocate_note_token(&mut self) -> Result<i32> {
        let id = self.allocate()?;
        if id > 999_999_999 {
            return Err(Error::Other(
                "comparison note custody token exceeds its nine-digit representation".into(),
            ));
        }
        Ok(id)
    }

    fn revision(
        &mut self,
        kind: &str,
        author: &str,
        timestamp: &str,
        inner: &str,
    ) -> Result<String> {
        let id = self.allocate()?;
        self.generated(Self::revision_with_id(kind, author, timestamp, inner, id))
    }

    fn revision_with_id(kind: &str, author: &str, timestamp: &str, inner: &str, id: i32) -> String {
        let author = oxoffice_core::xml::escape_attribute_value(author);
        let timestamp = oxoffice_core::xml::escape_attribute_value(timestamp);
        format!(
            r#"<w:{kind} w:id="{id}" w:author="{author}" w:date="{timestamp}">{inner}</w:{kind}>"#
        )
    }

    fn marker(&mut self, kind: &str, author: &str, timestamp: &str) -> Result<String> {
        let id = self.allocate()?;
        self.generated(Self::marker_with_id(kind, author, timestamp, id))
    }

    fn marker_with_id(kind: &str, author: &str, timestamp: &str, id: i32) -> String {
        let author = oxoffice_core::xml::escape_attribute_value(author);
        let timestamp = oxoffice_core::xml::escape_attribute_value(timestamp);
        format!(r#"<w:{kind} w:id="{id}" w:author="{author}" w:date="{timestamp}"/>"#)
    }
}

struct MovePairs {
    original: Vec<Option<i32>>,
    edited: Vec<Option<i32>>,
}

impl Document {
    /// Compare every supported Word story with `edited` and record tracked changes.
    ///
    /// Bookmark ids, which Word renumbers on every save, and the `_Toc` names
    /// a rebuilt table of contents gives its heading bookmarks are no change:
    /// a body bookmark both documents hold keeps this document's id, a heading
    /// paired with one of this document's takes its `_Toc` name, a new heading
    /// takes a fresh one where this document uses its own, and the links and
    /// page references of `edited` follow them. Footnote and endnote ids,
    /// which Word also renumbers, take this document's, the notes of each kind
    /// paired in reference order. A footnote or an endnote added or removed is
    /// inserted or deleted whole with its reference, and one whose reference
    /// moved is deleted with its old reference and inserted again, under a new
    /// id, with the new one, as Word shows it.
    ///
    /// No tracked change records a bookmark: a paragraph whose bookmarks
    /// differ is deleted and inserted whole, the insertion first, and a
    /// bookmark both documents hold is written once, outside the revision
    /// marks, its start in the insertion and its end in the deletion, as Word
    /// resolves them. Accepting leaves such an end just after the inserted
    /// paragraph, and rejecting such a start at the restored paragraph's
    /// start, over the same text. Such a paragraph is still refused when a
    /// bookmark both documents hold starts or ends inside its text, or sits
    /// under another id or inside a hyperlink or control, or when it holds a
    /// complex field or a comment range both documents hold.
    pub fn compare(
        &mut self,
        edited: &Document,
        author: &str,
        timestamp: &str,
    ) -> Result<Vec<ComparisonDiagnostic>> {
        self.compare_with_options(edited, author, timestamp, &ComparisonOptions::default())
    }

    /// Compare supported Word stories using an explicit granularity and ignore policy.
    ///
    /// Bookmark ids, generated `_Toc` names and footnote and endnote ids are
    /// reconciled as for [`Document::compare`]. An ignored footnote or endnote
    /// story keeps this document's notes of that kind whole.
    pub fn compare_with_options(
        &mut self,
        edited: &Document,
        author: &str,
        timestamp: &str,
        options: &ComparisonOptions,
    ) -> Result<Vec<ComparisonDiagnostic>> {
        // A pass that finds moved footnotes or endnotes hands back the edited
        // input with them under new ids, kept per kind, and the next one starts
        // from it. Each pass gives at least one more note a new id, so the
        // passes end.
        let mut renumbered = Vec::new();
        let mut next: Option<Document> = None;
        loop {
            let mut again = None;
            let input = next.as_ref().unwrap_or(edited);
            let diagnostics =
                self.compare_pass(input, author, timestamp, options, &renumbered, &mut again)?;
            let Some((document, ids)) = again else {
                return Ok(diagnostics);
            };
            renumbered.extend(ids);
            next = Some(document);
        }
    }

    /// One comparison pass. `renumbered` holds the ids an earlier pass gave
    /// the edited notes whose reference moved: they stay new notes. A pass
    /// that finds more sets `again` and writes nothing.
    fn compare_pass(
        &mut self,
        edited: &Document,
        author: &str,
        timestamp: &str,
        options: &ComparisonOptions,
        renumbered: &[(NoteKind, i32)],
        again: &mut Option<(Document, RenumberedNotes)>,
    ) -> Result<Vec<ComparisonDiagnostic>> {
        validate_revision_timestamp(timestamp)?;
        validate_comparison_options(options)?;
        let edited_input = edited;
        let mut original = comparison_input(self)?;
        let mut edited = comparison_input(edited)?;
        let mut note_ids = HashMap::new();
        for kind in NoteKind::ALL {
            if !story_ignored(options, kind.story_kind()) {
                retire_completed_note_inputs(&mut original, &mut edited, kind)?;
                reconcile_note_custody_inputs(&original, &mut edited, kind)?;
                note_ids.insert(
                    kind,
                    reconcile_note_ids(&original, &mut edited, kind, renumbered)?,
                );
            }
        }
        if !story_ignored(options, ComparisonStoryKind::Main) {
            reconcile_body_bookmarks(&original, &mut edited, options)?;
        }
        for kind in NoteKind::ALL {
            preflight_note_inputs(
                &original,
                &edited,
                kind,
                story_ignored(options, kind.story_kind()),
            )?;
        }
        if original.package.contains_part(COMMENT_COMPARISON_PART)
            || edited.package.contains_part(COMMENT_COMPARISON_PART)
        {
            return Err(Error::Other(
                "comparison requires resolved comment revisions".to_owned(),
            ));
        }
        let comments_ignored = story_ignored(options, ComparisonStoryKind::Comment);
        let original_comments = if comments_ignored {
            serde_json::Value::Null
        } else {
            comment_snapshot(&original)?
        };
        let edited_comments = if comments_ignored {
            serde_json::Value::Null
        } else {
            comment_snapshot(&edited)?
        };
        let comments_changed =
            !comments_ignored && !comment_snapshots_match(&original_comments, &edited_comments)?;
        let mut original_stories = story_parts_with_options(&original, options)?;
        let mut edited_stories = story_parts_with_options(&edited, options)?;
        // Existing revisions are kept where both inputs hold them unchanged,
        // and every other change near them is refused at its owner.
        let existing_revisions = contains_modeled_revisions(&original, &original_stories, options)?
            || contains_modeled_revisions(&edited, &edited_stories, options)?;
        if comments_changed
            && let Some((left, right)) = original_stories
                .iter()
                .find(|story| story.kind == ComparisonStoryKind::Comment)
                .zip(
                    edited_stories
                        .iter()
                        .find(|story| story.kind == ComparisonStoryKind::Comment),
                )
            && left == right
        {
            let original_source =
                std::str::from_utf8(story_xml(&original, left)?).map_err(utf8_error)?;
            let edited_source =
                std::str::from_utf8(story_xml(&edited, right)?).map_err(utf8_error)?;
            let root_shell = |source| -> Result<Vec<String>> {
                Ok(canonical_owned_story(source, "comment")?
                    .0
                    .into_iter()
                    .filter(|token| token != "owner")
                    .collect())
            };
            if root_shell(original_source)? != root_shell(edited_source)? {
                return Err(Error::Other(format!(
                    "comments story root shell changed in {}",
                    left.part_name
                )));
            }
        }
        let comment_stories = original_stories
            .iter()
            .find(|story| story.kind == ComparisonStoryKind::Comment)
            .zip(
                edited_stories
                    .iter()
                    .find(|story| story.kind == ComparisonStoryKind::Comment),
            );
        // The inputs' list levels, for every signature and check of this pass;
        // entered again below once the edited input may have been replayed.
        let _input_numbering = numbering::NumberingLevelsScope::enter(
            numbering::NumberingLevels::new(&original.styles, &edited.styles),
        );
        let compare_comment_story = |left: &StoryPart, right: &StoryPart| {
            compare_story_part(
                story_xml(&original, left).unwrap_or_default(),
                story_xml(&edited, right).unwrap_or_default(),
                left,
                &mut Metadata {
                    author,
                    timestamp,
                    options,
                    ids: IdAllocator::new(HashSet::new()),
                    existing_revisions,
                    source_frames: Vec::new(),
                    projected_textbox_noop: false,
                    leaf_capture: false,
                    paired_field_story: None,
                    leaf_publications: Vec::new(),
                    field_conversions: Vec::new(),
                    whole_field_conversions: Vec::new(),
                    shared_bookmarks: HashSet::new(),
                    point_bookmarks: HashSet::new(),
                    relocated_bookmarks: HashSet::new(),
                    // Before the relationships are remapped: the comment
                    // story compares in place only when both inputs hold the
                    // same comment relationships.
                    whole_hyperlinks: WholeHyperlinks {
                        targets: std::rc::Rc::new(hyperlink_targets(
                            &original,
                            [left.part_name.clone()],
                            &[],
                        )),
                        owner: left.part_name.clone(),
                        witnesses: Vec::new(),
                    },
                },
                &mut Vec::new(),
            )
        };
        let compatible_comment_story = comment_stories.is_some_and(|(left, right)| {
            left == right && compare_comment_story(left, right).is_ok()
        });
        let snapshot_comments = comments_changed
            && (!compatible_comment_story
                || !extended_comment_snapshots_match(&original_comments, &edited_comments)
                || !comment_package_links_match(&original_comments, &edited_comments));
        if snapshot_comments && existing_revisions {
            // The comment package revision replaces every comment part whole,
            // so it would rewrite the revisions a comment holds.
            for (document, stories) in [(&original, &original_stories), (&edited, &edited_stories)]
            {
                for story in stories
                    .iter()
                    .filter(|story| story.kind == ComparisonStoryKind::Comment)
                {
                    if modeled_revision_count_with_options(story_xml(document, story)?, options)?
                        > 0
                    {
                        let reason = match comment_stories {
                            Some((left, right)) if left == right => compare_comment_story(
                                left, right,
                            )
                            .err()
                            .map_or_else(
                                || "comments were added, removed, answered or resolved".to_owned(),
                                |error| error.to_string(),
                            ),
                            _ => "the comment parts differ".to_owned(),
                        };
                        return Err(Error::Other(format!(
                            "comparison cannot replace the comments of {} whole, because a comment holds an existing tracked revision: {reason}",
                            story.part_name
                        )));
                    }
                }
            }
        }
        let added_reply_ids = if snapshot_comments
            && !options.ignore_comments
            && !story_ignored(options, ComparisonStoryKind::Main)
        {
            if let Some((ids, replay)) = crossing_reply_additions(&original, &edited, options)? {
                // This is the actual original-plus-reply producer. Using its
                // models and Sources together preserves every ignored value,
                // while keeping the newly authored crossing graph.
                edited = replay;
                ids
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        if snapshot_comments {
            original_stories.retain(|story| story.kind != ComparisonStoryKind::Comment);
            edited_stories.retain(|story| story.kind != ComparisonStoryKind::Comment);
        }
        original_stories.retain(|story| NoteKind::of_story(story.kind).is_none());
        edited_stories.retain(|story| NoteKind::of_story(story.kind).is_none());
        if original_stories != edited_stories {
            return Err(Error::Other(
                "document comparison requires identical related-story shells".to_owned(),
            ));
        }
        let (carried_links, mut charts) = remap_equivalent_story_relationships(
            &original,
            &mut edited,
            &original_stories,
            &edited_stories,
        )?;
        reject_cross_story_moves(&original, &edited, &original_stories, options)?;

        let original_xml = original
            .package
            .get_part(&original.doc_part_name)
            .ok_or_else(|| {
                Error::Other(format!("missing main story {}", original.doc_part_name))
            })?;
        let edited_xml = edited
            .package
            .get_part(&edited.doc_part_name)
            .ok_or_else(|| Error::Other(format!("missing main story {}", edited.doc_part_name)))?;
        if original_xml == edited_xml {
            let mut stories_unchanged = true;
            for story in &original_stories {
                if story_xml(&original, story)? != story_xml(&edited, story)? {
                    stories_unchanged = false;
                    break;
                }
            }
            for kind in NoteKind::ALL {
                stories_unchanged &= NotePart::of(&original, kind)?.map(|part| part.xml)
                    == NotePart::of(&edited, kind)?.map(|part| part.xml);
            }
            if stories_unchanged && !snapshot_comments {
                return Ok(Vec::new());
            }
        }
        let text_box_markers =
            comparison_text_box_markers(&original, &edited, &original_stories, options)?;
        let mut used_ids = if story_ignored(options, ComparisonStoryKind::Main) {
            HashSet::new()
        } else {
            word_ids_with_options(original_xml, options)?
        };
        if !story_ignored(options, ComparisonStoryKind::Main) {
            used_ids.extend(word_ids_with_options(edited_xml, options)?);
        }
        for story in &original_stories {
            used_ids.extend(word_ids_with_options(
                story_xml(&original, story)?,
                options,
            )?);
            used_ids.extend(word_ids_with_options(story_xml(&edited, story)?, options)?);
        }
        for document in [&original, &edited] {
            for kind in NoteKind::ALL {
                if let Some(part) = NotePart::of(document, kind)? {
                    used_ids.extend(word_ids_with_options(part.xml.as_bytes(), options)?);
                }
                used_ids.extend(note_custody_reserved_ids(document, kind)?);
            }
        }
        let input_ids = used_ids.clone();
        let _numbering = numbering::NumberingLevelsScope::enter(numbering::NumberingLevels::new(
            &original.styles,
            &edited.styles,
        ));
        let mut link_owners = vec![original.doc_part_name.clone()];
        link_owners.extend(original_stories.iter().map(|story| story.part_name.clone()));
        for document in [&original, &edited] {
            for kind in NoteKind::ALL {
                link_owners.extend(NotePart::of(document, kind)?.map(|part| part.name));
            }
        }
        let link_targets = hyperlink_targets(&original, link_owners, &carried_links);
        let shared_bookmarks = shared_bookmark_ids(&original, &edited)?;
        let point_bookmarks = zero_width_bookmark_ids(&edited, &shared_bookmarks)?;
        let mut metadata = Metadata {
            author,
            timestamp,
            options,
            ids: IdAllocator::new(used_ids),
            existing_revisions,
            source_frames: Vec::new(),
            projected_textbox_noop: false,
            leaf_capture: false,
            paired_field_story: None,
            leaf_publications: Vec::new(),
            field_conversions: Vec::new(),
            whole_field_conversions: Vec::new(),
            shared_bookmarks,
            point_bookmarks,
            relocated_bookmarks: HashSet::new(),
            whole_hyperlinks: WholeHyperlinks {
                targets: std::rc::Rc::new(link_targets),
                owner: original.doc_part_name.clone(),
                witnesses: Vec::new(),
            },
        };
        metadata.ids.local_word_binding =
            foreign_word_binding(original_xml)? || foreign_word_binding(edited_xml)?;
        let mut diagnostics = Vec::new();
        let main_classes = RelationshipClassScope::enter(relationship_classes(
            &original,
            &original.doc_part_name,
        )?);
        let tracked_body = if !added_reply_ids.is_empty() {
            extract_body_inner(edited_xml)?.to_owned()
        } else if story_ignored(options, ComparisonStoryKind::Main) {
            extract_body_inner(original_xml)?.to_owned()
        } else if story_ignored(options, ComparisonStoryKind::TextBox)
            || text_boxes_changed_inside(
                extract_body_inner(original_xml)?,
                extract_body_inner(edited_xml)?,
                "w",
            )?
        {
            let original_source = std::str::from_utf8(original_xml).map_err(utf8_error)?;
            let edited_source = std::str::from_utf8(edited_xml).map_err(utf8_error)?;
            let (_, original_bindings) = main_body_source(original_source.as_bytes())?;
            let (_, edited_bindings) = main_body_source(edited_source.as_bytes())?;
            // Box by box, whatever the formatting policy: replaced whole, the
            // deleted copy of a box would keep its text as text (#666).
            compare_story_inner_impl(
                extract_body_inner(original_xml)?,
                extract_body_inner(edited_xml)?,
                "body",
                "w",
                &original_bindings,
                &edited_bindings,
                &mut metadata,
                &mut diagnostics,
                true,
            )?
        } else {
            let original_body = extract_body_inner(original_xml)?;
            let edited_body = extract_body_inner(edited_xml)?;
            let original_source = std::str::from_utf8(original_xml).map_err(utf8_error)?;
            let edited_source = std::str::from_utf8(edited_xml).map_err(utf8_error)?;
            let (_, original_bindings) = main_body_source(original_source.as_bytes())?;
            let (_, edited_bindings) = main_body_source(edited_source.as_bytes())?;
            let original_spans = story_content_spans(original_body, &original_bindings)?;
            let edited_spans = story_content_spans(edited_body, &edited_bindings)?;
            let original_document = story_document(original_body, &original_bindings)?;
            let edited_document = story_document(edited_body, &edited_bindings)?;
            if original_spans.len() != original_document.body.content.len()
                || edited_spans.len() != edited_document.body.content.len()
            {
                return Err(Error::Other(
                    "comparison could not correlate main-story owners".to_owned(),
                ));
            }
            let held_original = HeldRevisions::of(
                original_body,
                &original_spans,
                &original_bindings,
                metadata.existing_revisions,
            )?;
            let held_edited = HeldRevisions::of(
                edited_body,
                &edited_spans,
                &edited_bindings,
                metadata.existing_revisions,
            )?;
            compare_body(
                [&original_document, &edited_document],
                "body",
                Some((original_body, &original_spans, &original_bindings)),
                (edited_body, &edited_spans, &edited_bindings),
                [&held_original, &held_edited],
                &mut metadata,
                &mut diagnostics,
            )?
        };
        drop(main_classes);
        let mut tracked_xml = replace_body_inner(original_xml, &tracked_body)?.into_bytes();
        // Generated fragments use the canonical Word prefix. A valid source
        // may declare only an alias. Add the missing binding without changing
        // any existing producer binding or retained revision bytes.
        let generated_bindings = required_external_bindings(
            &tracked_xml,
            &BTreeMap::from([("w".to_owned(), W_NS.to_owned())]),
        )?;
        tracked_xml =
            crate::document::close_content_fragment_namespaces(&tracked_xml, &generated_bindings)?;
        // Content from the edited document can use the `w14` its own root
        // declares, which the original root may not.
        declare_w14_on_part_root(&mut tracked_xml)?;
        let tracked_xml = crate::document::uniquify_drawing_ids_in_xml(&tracked_xml)?;
        // Physical package source is not a parse-closure scratch buffer. The
        // imported model retains its actual ancestor namespace context.
        let tracked = CT_Document::from_xml(&tracked_xml)?;
        tracked.to_xml()?;
        let mut candidate = original.clone_for_staging();
        candidate.document = crate::document::DocumentModel::unproved(tracked);
        candidate
            .package
            .set_part(&candidate.doc_part_name, tracked_xml);
        for story in &original_stories {
            let tracked_story = if !added_reply_ids.is_empty() {
                story_xml(&edited, story)?.to_vec()
            } else {
                let _classes = RelationshipClassScope::enter(relationship_classes(
                    &original,
                    &story.part_name,
                )?);
                metadata.whole_hyperlinks.owner = story.part_name.clone();
                compare_story_part(
                    story_xml(&original, story)?,
                    story_xml(&edited, story)?,
                    story,
                    &mut metadata,
                    &mut diagnostics,
                )?
            };
            // A picture deleted and inserted again keeps its drawing id on
            // both sides, as in the body.
            let tracked_story = crate::document::uniquify_drawing_ids_in_xml(&tracked_story)?;
            candidate.package.set_part(&story.part_name, tracked_story);
        }
        for kind in NoteKind::ALL {
            let moved = compare_note_parts(
                &original,
                &edited,
                kind,
                &mut candidate,
                &mut metadata,
                &mut diagnostics,
            )?;
            if moved.is_empty() {
                continue;
            }
            // A note whose reference moved to another site would be
            // referenced twice, from the deleted original site and from the
            // inserted edited one. As Word does, the edited note becomes a
            // new note: the comparison runs again with it under a new id, so
            // the original note is deleted with its reference and the edited
            // one inserted with its own. Nothing was written to this document.
            // See `compare_with_options`.
            // The ids found are the reconciled ones: back to the input's.
            let moved = moved
                .iter()
                .map(|&id| {
                    note_ids
                        .get(&kind)
                        .and_then(|ids| ids.iter().find(|(_, to)| **to == id))
                        .map_or(id, |(&from, _)| from)
                })
                .collect::<Vec<_>>();
            let Some((next, ids)) = renumbered_notes(&original, edited_input, kind, &moved)? else {
                return Err(Error::Other(format!(
                    "comparison would give {} {} more physical references than either input: its reference moved to another site",
                    kind.word(),
                    moved[0]
                )));
            };
            *again = Some((next, ids));
            return Ok(Vec::new());
        }
        refuse_shared_note_references(
            &original,
            &edited,
            &candidate,
            story_ignored(options, ComparisonStoryKind::Endnote),
        )?;
        if snapshot_comments {
            let revision_id = metadata.ids.allocate()?;
            let mut carried_comments = edited_comments.clone();
            remap_comment_targets(&candidate, &mut carried_comments)?;
            apply_comment_snapshot(&mut candidate, &carried_comments)?;
            // The markers of a removed comment stay in the deleted content
            // until the deletion is accepted, and their comment with them.
            candidate.keep_comment_definitions_from(&original)?;
            let record = serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "author": author,
                "timestamp": timestamp,
                "id": revision_id,
                "original": original_comments,
                "edited": carried_comments,
                "added_reply_ids": added_reply_ids,
            }))
            .map_err(|error| Error::Other(error.to_string()))?;
            candidate.package.set_part(
                COMMENT_COMPARISON_PART,
                format!(
                    "{COMMENT_COMPARISON_OPEN}{}{COMMENT_COMPARISON_CLOSE}",
                    base64::engine::general_purpose::STANDARD.encode(record)
                )
                .into_bytes(),
            );
            candidate
                .package
                .content_types
                .add_override(COMMENT_COMPARISON_PART, "application/xml");
            candidate
                .package
                .get_or_create_part_rels(&candidate.doc_part_name)
                .add(
                    COMMENT_COMPARISON_REL,
                    "../customXml/rdocxComparisonComments.xml",
                );
        }
        // A hyperlink that only the edited side targets arrives with the
        // edited content that holds it.
        let mut referenced_by_owner = HashMap::<String, HashSet<String>>::new();
        let mut carried_image = false;
        for carried in carried_links {
            let CarriedRelationship {
                owner,
                relationship,
                media,
                chart,
            } = carried;
            if !referenced_by_owner.contains_key(&owner) {
                let ids = candidate
                    .package
                    .get_part(&owner)
                    .map(crate::document::xml_relationship_ids_in_order)
                    .transpose()?
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                referenced_by_owner.insert(owner.clone(), ids);
            }
            if referenced_by_owner
                .get(&owner)
                .is_some_and(|ids| ids.contains(&relationship.id))
            {
                if let Some(CarriedMedia {
                    part_name,
                    bytes,
                    content_type,
                }) = media
                {
                    carried_image = true;
                    candidate.package.set_part(&part_name, bytes);
                    candidate
                        .package
                        .content_types
                        .add_override(&part_name, &content_type);
                }
                if let Some(source_part) = chart {
                    crate::field::copy_fragment_part(
                        &edited.package,
                        &mut candidate,
                        &source_part,
                        &charts.part_map,
                        &mut charts.copied,
                    )?;
                }
                candidate
                    .package
                    .get_or_create_part_rels(&owner)
                    .items
                    .push(relationship);
            }
        }
        if carried_image {
            let mut bytes = std::io::Cursor::new(Vec::new());
            candidate.package.write_to(&mut bytes)?;
            candidate = Document::from_bytes(bytes.get_ref())?;
        }
        // Content taken from the edited version names its styles, and the
        // first note or comment it brings the styles of notes and comments.
        candidate.adopt_missing_styles(&edited)?;
        candidate = reopen_staged(candidate)?;
        // The postconditions resolve only the revisions this comparison
        // wrote, so a kept revision stays in both resolved copies and must
        // match the input that holds it.
        let new_ids = metadata
            .ids
            .used
            .difference(&input_ids)
            .copied()
            .collect::<HashSet<_>>();
        let mut field_stories = original_stories.clone();
        for kind in NoteKind::ALL {
            if !story_ignored(options, kind.story_kind())
                && let Some(part) = NotePart::of(&original, kind)?.or(NotePart::of(&edited, kind)?)
            {
                field_stories.push(StoryPart {
                    kind: kind.story_kind(),
                    part_name: part.name,
                });
            }
        }

        if existing_revisions {
            let mut prior_stories = original_stories.clone();
            for kind in NoteKind::ALL {
                if !story_ignored(options, kind.story_kind())
                    && let Some(part) = NotePart::of(&original, kind)?
                {
                    prior_stories.push(StoryPart {
                        kind: kind.story_kind(),
                        part_name: part.name,
                    });
                }
            }
            refuse_changed_existing_revisions(
                &original,
                &candidate,
                &prior_stories,
                options,
                &new_ids,
            )?;
        }
        let parts = std::iter::once(&candidate.doc_part_name)
            .chain(original_stories.iter().map(|story| &story.part_name));
        for part in parts {
            if candidate
                .package
                .get_part(part)
                .is_some_and(text_in_deletions)
            {
                return Err(Error::Other(format!(
                    "comparison would write text inside a deletion of {part}, as a run holding a text box deleted or replaced whole does (#666)"
                )));
            }
        }
        // A link written as a field is read back before the resolution
        // retires its note markers, from the bytes the field was written in.
        let mut accepted_links = Default::default();
        let accepted = resolved_candidate(
            &candidate,
            |document| {
                let count = document.accept_revision_ids(&new_ids)?;
                accepted_links = restore_whole_hyperlinks(
                    &candidate,
                    document,
                    &metadata.whole_hyperlinks.witnesses,
                    1,
                    &new_ids,
                )?;
                Ok(count)
            },
            Some(&new_ids),
        )?;
        // A bookmark both inputs hold that a whole paragraph wrote once (see
        // `place_shared_bookmarks`) is held once under its name and covers
        // the same text after either resolution; its start or end may sit at
        // the paragraph edge the resolution leaves it on, so the rest of the
        // stories is compared without it.
        let relocated = &metadata.relocated_bookmarks;
        let relocated_views = |resolved: &Document, input: &Document, outcome: &str| {
            if relocated.is_empty() {
                return Ok((None, None));
            }
            if !holds_bookmarks_once(resolved, input, relocated)?
                || bookmark_coverage(resolved, relocated)? != bookmark_coverage(input, relocated)?
            {
                return Err(Error::Other(format!(
                    "comparison {outcome} does not keep each bookmark both inputs hold once over the same text: \
                     the paragraphs it replaces whole cannot carry one of them where it sits"
                )));
            }
            Ok((
                Some(without_bookmarks(resolved, relocated)?),
                Some(without_bookmarks(input, relocated)?),
            ))
        };
        let (accepted_view, edited_view) = relocated_views(&accepted, &edited, "acceptance")?;
        let mut accepted_body = normalized_package(
            accepted_view.as_ref().unwrap_or(&accepted),
            &original_stories,
            options,
            &text_box_markers,
        )?;
        let mut edited_package = normalized_package(
            edited_view.as_ref().unwrap_or(&edited),
            &edited_stories,
            options,
            &text_box_markers,
        )?;
        let main_ignored = story_ignored(options, ComparisonStoryKind::Main);
        if main_ignored {
            edited_package.0 = accepted_body.0.clone();
        }
        validate_field_conversion_role(
            &candidate,
            &mut accepted_body,
            &edited_package,
            &field_stories,
            &metadata.field_conversions,
            &metadata.whole_field_conversions,
            1,
            &new_ids,
            &accepted_links,
        )?;
        for kind in NoteKind::ALL {
            verify_note_destination(&accepted, &edited, kind, true, options)?;
        }
        if accepted_body != edited_package {
            return Err(comparison_postcondition_error(
                "acceptance",
                "edited",
                &accepted_body,
                &edited_package,
            ));
        }
        if snapshot_comments {
            let mut accepted = candidate.clone_for_staging();
            accepted.accept_revision_ids(&new_ids)?;
            if !comment_snapshots_match(&comment_snapshot(&accepted)?, &edited_comments)? {
                return Err(Error::Other(
                    "comparison acceptance does not reproduce edited comments".to_owned(),
                ));
            }
        }
        // A link written as a field is read back before the resolution
        // retires its note markers, from the bytes the field was written in.
        let mut rejected_links = Default::default();
        let rejected = resolved_candidate(
            &candidate,
            |document| {
                let count = document.reject_revision_ids(&new_ids)?;
                rejected_links = restore_whole_hyperlinks(
                    &candidate,
                    document,
                    &metadata.whole_hyperlinks.witnesses,
                    0,
                    &new_ids,
                )?;
                Ok(count)
            },
            Some(&new_ids),
        )?;
        let (rejected_view, original_view) = relocated_views(&rejected, &original, "rejection")?;
        let mut rejected_package = normalized_package(
            rejected_view.as_ref().unwrap_or(&rejected),
            &original_stories,
            options,
            &text_box_markers,
        )?;
        let mut original_package = normalized_package(
            original_view.as_ref().unwrap_or(&original),
            &original_stories,
            options,
            &text_box_markers,
        )?;
        if main_ignored {
            original_package.0 = rejected_package.0.clone();
        }
        validate_field_conversion_role(
            &candidate,
            &mut rejected_package,
            &original_package,
            &field_stories,
            &metadata.field_conversions,
            &metadata.whole_field_conversions,
            0,
            &new_ids,
            &rejected_links,
        )?;
        for kind in NoteKind::ALL {
            verify_note_destination(&rejected, &original, kind, false, options)?;
        }
        if rejected_package != original_package {
            return Err(comparison_postcondition_error(
                "rejection",
                "original",
                &rejected_package,
                &original_package,
            ));
        }
        if snapshot_comments {
            let mut rejected = candidate.clone_for_staging();
            rejected.reject_revision_ids(&new_ids)?;
            if !comment_snapshots_match(&comment_snapshot(&rejected)?, &original_comments)? {
                return Err(Error::Other(
                    "comparison rejection does not reproduce original comments".to_owned(),
                ));
            }
        }

        if metadata.projected_textbox_noop
            && new_ids.is_empty()
            && candidate.package.parts == original.package.parts
            && candidate.package.content_types == original.package.content_types
            && candidate.package.package_rels.to_xml()? == original.package.package_rels.to_xml()?
            && candidate
                .package
                .part_rels
                .iter()
                .map(|(name, rels)| Ok((name, rels.to_xml()?)))
                .collect::<Result<BTreeMap<_, _>>>()?
                == original
                    .package
                    .part_rels
                    .iter()
                    .map(|(name, rels)| Ok((name, rels.to_xml()?)))
                    .collect::<Result<BTreeMap<_, _>>>()?
        {
            return Ok(diagnostics);
        }
        self.commit_staged_mutation(candidate);
        Ok(diagnostics)
    }
}

#[derive(Clone)]
struct CarriedMedia {
    part_name: String,
    bytes: Vec<u8>,
    content_type: String,
}

struct CarriedRelationship {
    owner: String,
    relationship: Relationship,
    media: Option<CarriedMedia>,
    chart: Option<String>,
}

#[derive(Default)]
struct CarriedCharts {
    part_map: BTreeMap<String, String>,
    copied: HashSet<String>,
    bytes: usize,
}

/// The owners of one side of a story that hold an existing modeled revision.
#[derive(Default)]
struct HeldRevisions {
    /// One flag per owner, in the order of the story's content spans.
    owners: Vec<bool>,
    /// Source metadata absent from typed owners, indexed by physical owner.
    source_metadata: Vec<Option<Vec<String>>>,
    /// Whether the story's own section properties hold one.
    section: bool,
    section_source: Option<Vec<String>>,
}

#[derive(Clone)]
struct NoteOwner {
    id: Option<i32>,
    kind: RawNoteType,
    span: Range<usize>,
    inner: Range<usize>,
    bindings: NamespaceBindings,
}

#[derive(Clone)]
struct NotePart {
    name: String,
    xml: String,
    owners: Vec<NoteOwner>,
    kind: NoteKind,
}

#[derive(Clone)]
struct NoteMarker {
    token: i32,
    original: bool,
    edited: bool,
    family: char,
    part: String,
    span: Range<usize>,
    paragraph: Range<usize>,
    pair_id: i32,
    owner: Option<i32>,
    reference: Option<i32>,
    /// A body pair that Word moved into its own reference's revision wrapper.
    wrapped: bool,
}

#[derive(Clone)]
pub(crate) struct FootnoteCustody {
    /// The persistent token of a marked unit. A tracked-note unit has none:
    /// it is proved from its own owner and reference, never from a marker.
    token: Option<i32>,
    original: bool,
    edited: bool,
    owner: Option<i32>,
    addition: Option<bool>,
    selected: bool,
    revisions: Vec<crate::revision::RevisionMetadata>,
    /// The main-story span of a body pair that sits inside its reference's
    /// insertion. Resolving that insertion keeps the pair, so the unit's
    /// lifetime and bits do not depend on Word's wrapper merging.
    pub(crate) wrapped_body: Option<Range<usize>>,
}

#[derive(Default)]
struct SourceMetadata {
    cursor: usize,
    has_fields: bool,
    facts: ContentMetadata,
    field_records: HashMap<usize, (usize, usize, Option<usize>)>,
}

#[derive(Clone, Debug, PartialEq)]
struct GuardField {
    instruction: oxoffice_docx_oxml::text::FieldInstruction,
    complex: bool,
    cached: String,
    dirty: Option<bool>,
    locked: Option<bool>,
    legacy: Option<oxoffice_docx_oxml::text::LegacyFormFieldData>,
    segments: Vec<(String, Option<oxoffice_docx_oxml::properties::CT_RPr>)>,
    instruction_fields: Vec<GuardField>,
    result_fields: Vec<GuardField>,
    graphs: Vec<(String, String)>,
}

/// Guard-only values. Empty payload slots in the private model clone retain
/// canonical content boundaries. Their identities live here, never in XML.
#[derive(Clone, Debug)]
struct GuardFieldObservation {
    field_index: usize,
    result_path: Vec<usize>,
    occurrence: FieldSourceOccurrence,
    metadata_record: (usize, usize, Option<usize>),
}

#[derive(Clone, Debug)]
struct GuardItem {
    content: String,
    hosts: Vec<GuardHost>,
    fields: Vec<GuardField>,
    content_metadata: ContentMetadata,
    field_observations: Vec<GuardFieldObservation>,
}

#[derive(Clone, Debug, PartialEq)]
struct GuardHost {
    outside: Vec<String>,
    bodies: Vec<Vec<GuardItem>>,
    drawing: Option<oxoffice_docx_oxml::drawing::CT_Drawing>,
}

#[derive(Clone)]
struct RunSource {
    owner: usize,
    scoped: String,
    raw: String,
    parent: NamespaceBindings,
    // Indexed by the actual parsed raw ordinal. Root records have no child.
    raw_slots: Vec<Option<(Range<usize>, usize, bool)>>,
    typed_slots: Vec<Option<Range<usize>>>,
    // Only qualified policy projections may change these original physical slots.
    projected_raw_slots: HashSet<usize>,
    projected_typed_slots: HashSet<usize>,
}

#[derive(Clone, Debug)]
struct FieldSourceOccurrence {
    input_role: usize,
    story: String,
    owner_path: Vec<(u8, usize)>,
    field_slot: usize,
    result_ancestors: Vec<Range<usize>>,
    result_budget: Option<SourceBudget>,
    carrier_budget: Option<Vec<String>>,
    part_span: Option<Range<usize>>,
    part_hash: Option<[u8; 32]>,
    bounds: Range<usize>,
    source: String,
    scope: NamespaceBindings,
}

#[derive(Default)]
struct SourceSnapshots {
    properties_only: bool,
    guard_graphs: bool,
    field_graphs: HashMap<usize, Vec<(String, String)>>,
    input_role: usize,
    story: String,
    owner_paths: HashMap<usize, Vec<(u8, usize)>>,
    result_ancestors: HashMap<usize, Vec<Range<usize>>>,
    result_origins: HashMap<usize, (Range<usize>, usize)>,
    unproved_result_owners: HashSet<usize>,
    budget_sources: HashMap<usize, (String, NamespaceBindings)>,
    budget_holes: HashMap<usize, Vec<SourceBudgetHole>>,
    budget_offsets: HashMap<usize, usize>,
    part_base: Option<usize>,
    part_len: Option<usize>,
    part_hash: Option<[u8; 32]>,
    part_projection: Vec<(Range<usize>, Range<usize>)>,
    part_origins: HashMap<usize, (usize, usize)>,
    field_occurrences: HashMap<usize, FieldSourceOccurrence>,
    paragraphs: HashMap<usize, String>,
    runs: HashMap<usize, RunSource>,
    tables: HashMap<usize, String>,
    rows: HashMap<usize, String>,
    cells: HashMap<usize, String>,
    controls: HashMap<usize, String>,
    fields: HashMap<usize, Vec<String>>,
    field_scopes: HashMap<usize, NamespaceBindings>,
    roots: HashMap<usize, (String, NamespaceBindings)>,
    shared_root_runs: HashSet<usize>,
    shared_root_facts: HashMap<usize, Vec<String>>,
}

/// One actual property side, captured before the new paragraph markers.
#[derive(Clone, Default)]
struct PropertySide<'a> {
    typed: Option<&'a CT_PPr>,
    xml: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TypedSourceSelection {
    ordinal: usize,
    scalars: Option<(usize, usize)>,
}

/// A leaf emitted from a proved typed input slot, before temporary resolution.
/// The key is local input owner plus original content ordinal, never XML value.
#[derive(Clone)]
struct LeafPublication {
    range: Range<usize>,
    owner: usize,
    ordinal: usize,
    scalars: Range<usize>,
    source: String,
    context: NamespaceBindings,
    payload: String,
    physical: (usize, usize, Range<usize>),
    domains: [Option<(usize, usize, usize)>; 2],
    actions: Vec<([Option<Range<usize>>; 2], GranularAction)>,
}

/// What [`trailing_paragraph`] carries from one paragraph of the run that
/// ends the body to the next.
#[derive(Default)]
struct TrailingState<'a> {
    /// The properties [`final_paragraph`] merges.
    merged: PropertySide<'a>,
    /// Whether a kept table precedes the run.
    after_table: bool,
    /// Whether the run is one change that marks the paragraph before it.
    single: bool,
    /// The moved paragraphs the emitted run paragraphs end with: their move
    /// id and the range of the output they fill.
    block: Option<(i32, Range<usize>)>,
}

/// A paragraph of the run of owners one side alone holds that ends the
/// body.
struct TrailingRun<'a, 'l> {
    content: &'a BodyContent,
    paragraph: &'a CT_P,
    kind: &'static str,
    move_id: Option<i32>,
    /// Whether it is replaced whole inside a complex field (see
    /// [`deleted_paragraph`]).
    carried: bool,
    /// Whether it is the first paragraph of the run, and the only one.
    first: bool,
    single: bool,
    next_takes_mark: bool,
    location: &'l str,
}

#[derive(Clone)]
struct AttributedRunUnit {
    run: CT_R,
    ignored: bool,
    owner: usize,
    // Local raw-array index, original physical ordinal and original boundary.
    raw_sources: Vec<(usize, usize, usize)>,
    // A whole unchanged typed atom, or generated authority for a split atom.
    typed_sources: Vec<Option<TypedSourceSelection>>,
}

/// The paragraphs, by index on each side, that a replaced run deletes or
/// inserts whole inside the complex fields that enclose their hyperlinks
/// and simple fields.
#[derive(Default)]
struct CarriedParagraphs {
    original: HashSet<usize>,
    edited: HashSet<usize>,
}
