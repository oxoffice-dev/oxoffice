//! Deterministic native document comparison and tracked-revision generation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use base64::Engine;
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{BodyContent, CT_Document};
use oxoffice_docx_oxml::namespace::W_NS;
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
    RawNoteType, raw_note_attribute_in_scope, raw_note_type_in_scope,
    remove_selected_footnote_definitions, settle_footnote_custody, validate_footnote_custody,
};
use footnotes::{
    compare_footnote_parts, footnote_custody_reserved_ids, normal_note_owner, note_markers,
    preflight_footnote_inputs, reconcile_footnote_custody_inputs, reconcile_footnote_ids,
    refuse_shared_note_references, renumbered_footnotes, retire_completed_footnote_inputs,
    retire_note_markers, verify_footnote_destination,
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

use bookmarks::{reconcile_body_bookmarks, rewritten_opening, word_attributes};
use field_conversions::validate_field_conversion_role;
use guard::{
    guard_mask_coordinates, guard_source_body_at, guard_story_owners, guard_story_part,
    normalized_story_part,
};
use move_pairs::{crossing_reply_additions, pair_moves};
use namespaces::{
    actual_root_bindings, declared_prefixes, drawing_signature, element_inner_range_any_prefix,
    foreign_word_binding, required_external_bindings, root_prefix,
};
pub(crate) use postconditions::reopen_staged;
use postconditions::{
    NormalizedPackage, comparison_postcondition_error, normalized_package,
    refuse_changed_existing_revisions, resolved_candidate,
};
use relationships::{chart_relationship_signature, remap_equivalent_story_relationships};
use source_metadata::{
    ContentMetadata, field_source_metadata, source_root_metadata,
    source_without_run_root_attributes, source_without_text_properties,
};
use source_scopes::{
    ParentScopes, canonical_owned_story, child_scopes, publication_parent_at,
    publication_parents_at, root_inner_range, root_namespace_bindings, same_owner_tree,
    same_written_owner, source_child, source_children, source_fragment, story_skeleton,
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
    text_box_spans, without_text_box_subtrees, word_ids_with_options,
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
    run_source_events,
};

use alignment::{
    Alignment, align, pair_gaps_by_kind, refuse_uncarried_field_owners,
    replace_changed_paragraph_runs, validate_field_alignment,
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
    body_signature, body_signature_with_options, cell_formatting, coalesced_postcondition_body,
    control_content_signature_with_options, control_metadata_diagnostics,
    control_property_signature, control_signature_with_options, control_whitespace_slots,
    has_edge_whitespace, modeled_control_content, normalized_body_with_options,
    paragraph_formatting, paragraph_signature_with_options, postcondition_body, row_formatting,
    row_signature_with_options, run_content_signature, run_signature, run_signature_with_options,
    semantic_run_raw, table_signature_with_options,
};
use xml::{
    append_word_child, body_content_xml, cell_child_spans, cell_xml, control_xml, deleted_run,
    deleted_run_xml, detach_field_spans, direct_element_spans, direct_word_element_spans,
    direct_word_element_spans_in_scope, element_local_name, extract_body_inner,
    inject_before_close, main_body_source, modeled_paragraph_run_spans, paragraph_close_start,
    paragraph_owned_run_xml, paragraph_run_spans, paragraph_xml, property_xml, replace_body_inner,
    replace_direct_word_elements, replace_element_inner, replace_paragraph_run_elements,
    replace_ranges, row_xml, run_xml, section_property_xml, table_xml, utf8_error,
    with_mark_revision, word_ids,
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
        let author = quick_xml::escape::escape(author);
        let timestamp = quick_xml::escape::escape(timestamp);
        format!(
            r#"<w:{kind} w:id="{id}" w:author="{author}" w:date="{timestamp}">{inner}</w:{kind}>"#
        )
    }

    fn marker(&mut self, kind: &str, author: &str, timestamp: &str) -> Result<String> {
        let id = self.allocate()?;
        self.generated(Self::marker_with_id(kind, author, timestamp, id))
    }

    fn marker_with_id(kind: &str, author: &str, timestamp: &str, id: i32) -> String {
        let author = quick_xml::escape::escape(author);
        let timestamp = quick_xml::escape::escape(timestamp);
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
    /// page references of `edited` follow them. Footnote ids, which Word also
    /// renumbers, take this document's, the notes paired in reference order. A
    /// footnote whose reference moved is deleted with its old reference and
    /// inserted again, under a new id, with the new one, as Word shows it.
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
    /// Bookmark ids and generated `_Toc` names are reconciled as for
    /// [`Document::compare`].
    pub fn compare_with_options(
        &mut self,
        edited: &Document,
        author: &str,
        timestamp: &str,
        options: &ComparisonOptions,
    ) -> Result<Vec<ComparisonDiagnostic>> {
        // A pass that finds moved footnotes hands back the edited input with
        // them under new ids, and the next one starts from it. Each pass gives
        // at least one more note a new id, so the passes end.
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
        renumbered: &[i32],
        again: &mut Option<(Document, Vec<i32>)>,
    ) -> Result<Vec<ComparisonDiagnostic>> {
        validate_revision_timestamp(timestamp)?;
        validate_comparison_options(options)?;
        let edited_input = edited;
        let mut original = comparison_input(self)?;
        let mut edited = comparison_input(edited)?;
        let mut footnote_ids = HashMap::new();
        if !story_ignored(options, ComparisonStoryKind::Footnote) {
            retire_completed_footnote_inputs(&mut original, &mut edited)?;
            reconcile_footnote_custody_inputs(&original, &mut edited)?;
            footnote_ids = reconcile_footnote_ids(&original, &mut edited, renumbered)?;
        }
        if !story_ignored(options, ComparisonStoryKind::Main) {
            reconcile_body_bookmarks(&original, &mut edited, options)?;
        }
        preflight_footnote_inputs(
            &original,
            &edited,
            story_ignored(options, ComparisonStoryKind::Footnote),
        )?;
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
        original_stories.retain(|story| story.kind != ComparisonStoryKind::Footnote);
        edited_stories.retain(|story| story.kind != ComparisonStoryKind::Footnote);
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
            if stories_unchanged
                && !snapshot_comments
                && FootnotePart::of(&original)?.map(|part| part.xml)
                    == FootnotePart::of(&edited)?.map(|part| part.xml)
            {
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
            if let Some(part) = FootnotePart::of(document)? {
                used_ids.extend(word_ids_with_options(part.xml.as_bytes(), options)?);
            }
            used_ids.extend(footnote_custody_reserved_ids(document)?);
        }
        let input_ids = used_ids.clone();
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
        };
        metadata.ids.local_word_binding =
            foreign_word_binding(original_xml)? || foreign_word_binding(edited_xml)?;
        let mut diagnostics = Vec::new();
        let tracked_body = if !added_reply_ids.is_empty() {
            extract_body_inner(edited_xml)?.to_owned()
        } else if story_ignored(options, ComparisonStoryKind::Main) {
            extract_body_inner(original_xml)?.to_owned()
        } else if story_ignored(options, ComparisonStoryKind::TextBox) {
            let original_source = std::str::from_utf8(original_xml).map_err(utf8_error)?;
            let edited_source = std::str::from_utf8(edited_xml).map_err(utf8_error)?;
            let (_, original_bindings) = main_body_source(original_source.as_bytes())?;
            let (_, edited_bindings) = main_body_source(edited_source.as_bytes())?;
            compare_story_inner(
                extract_body_inner(original_xml)?,
                extract_body_inner(edited_xml)?,
                "body",
                "w",
                &original_bindings,
                &edited_bindings,
                &mut metadata,
                &mut diagnostics,
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
                compare_story_part(
                    story_xml(&original, story)?,
                    story_xml(&edited, story)?,
                    story,
                    &mut metadata,
                    &mut diagnostics,
                )?
            };
            candidate.package.set_part(&story.part_name, tracked_story);
        }
        let moved = compare_footnote_parts(
            &original,
            &edited,
            &mut candidate,
            &mut metadata,
            &mut diagnostics,
        )?;
        if !moved.is_empty() {
            // A footnote whose reference moved to another site would be
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
                    footnote_ids
                        .iter()
                        .find(|(_, to)| **to == id)
                        .map_or(id, |(&from, _)| from)
                })
                .collect::<Vec<_>>();
            let Some((next, ids)) = renumbered_footnotes(&original, edited_input, &moved)? else {
                return Err(Error::Other(format!(
                    "comparison would give footnote {} more physical references than either input: its reference moved to another site",
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
            let record = serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "author": author,
                "timestamp": timestamp,
                "id": revision_id,
                "original": original_comments,
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
        if !story_ignored(options, ComparisonStoryKind::Footnote)
            && let Some(part) = FootnotePart::of(&original)?.or(FootnotePart::of(&edited)?)
        {
            field_stories.push(StoryPart {
                kind: ComparisonStoryKind::Footnote,
                part_name: part.name,
            });
        }

        if existing_revisions {
            let mut prior_stories = original_stories.clone();
            if !story_ignored(options, ComparisonStoryKind::Footnote)
                && let Some(part) = FootnotePart::of(&original)?
            {
                prior_stories.push(StoryPart {
                    kind: ComparisonStoryKind::Footnote,
                    part_name: part.name,
                });
            }
            refuse_changed_existing_revisions(
                &original,
                &candidate,
                &prior_stories,
                options,
                &new_ids,
            )?;
        }
        let accepted = resolved_candidate(
            &candidate,
            |document| document.accept_revision_ids(&new_ids),
            Some(&new_ids),
        )?;
        let mut accepted_body =
            normalized_package(&accepted, &original_stories, options, &text_box_markers)?;
        let mut edited_package =
            normalized_package(&edited, &edited_stories, options, &text_box_markers)?;
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
            1,
            &new_ids,
        )?;
        verify_footnote_destination(&accepted, &edited, true, options)?;
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
        let rejected = resolved_candidate(
            &candidate,
            |document| document.reject_revision_ids(&new_ids),
            Some(&new_ids),
        )?;
        let mut rejected_package =
            normalized_package(&rejected, &original_stories, options, &text_box_markers)?;
        let mut original_package =
            normalized_package(&original, &original_stories, options, &text_box_markers)?;
        if main_ignored {
            original_package.0 = rejected_package.0.clone();
        }
        validate_field_conversion_role(
            &candidate,
            &mut rejected_package,
            &original_package,
            &field_stories,
            &metadata.field_conversions,
            0,
            &new_ids,
        )?;
        verify_footnote_destination(&rejected, &original, false, options)?;
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
struct FootnoteOwner {
    id: Option<i32>,
    kind: RawNoteType,
    span: Range<usize>,
    inner: Range<usize>,
    bindings: NamespaceBindings,
}

#[derive(Clone)]
struct FootnotePart {
    name: String,
    xml: String,
    owners: Vec<FootnoteOwner>,
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

/// Compare the owners of one story and return the tracked story content.
///
/// `held` names the owners of each side that hold an existing modeled
/// revision. Such an owner is kept only when both sides hold it unchanged,
/// and then its original bytes are emitted. Any other change that would
/// revise, delete, insert or move it, or mark its paragraph mark, is refused.
// Frames contain only live immutable parse owners. Nested body calls push a
// separate frame, and pop on every Result exit before their owners can drop.
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

fn compare_body(
    documents: [&CT_Document; 2],
    location: &str,
    original_source: Option<OriginalBodySource<'_>>,
    edited_source: OriginalBodySource<'_>,
    held: [&HeldRevisions; 2],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let [original, edited] = documents;
    let mut frame = SourceSnapshots {
        story: location.to_owned(),
        ..SourceSnapshots::default()
    };
    if let Some(source) = original_source {
        frame.input_role = 0;
        frame.body(original, source, held[0])?;
    }
    frame.input_role = 1;
    frame.body(edited, edited_source, held[1])?;
    metadata.source_frames.push(frame);
    let result = compare_body_impl(
        original,
        edited,
        location,
        original_source,
        edited_source,
        held,
        metadata,
        diagnostics,
    );
    metadata.source_frames.pop();
    result
}

fn compare_body_impl(
    original: &CT_Document,
    edited: &CT_Document,
    location: &str,
    original_source: Option<OriginalBodySource<'_>>,
    edited_source: OriginalBodySource<'_>,
    held: [&HeldRevisions; 2],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let [held_original, held_edited] = held;
    let source_bindings = original_source.map_or(&[][..], |(_, _, bindings)| bindings);
    let original_signatures = original
        .body
        .content
        .iter()
        .map(|content| body_signature_with_options(content, metadata.options))
        .collect::<Vec<_>>();
    let edited_signatures = edited
        .body
        .content
        .iter()
        .map(|content| body_signature_with_options(content, metadata.options))
        .collect::<Vec<_>>();
    fn paragraphs(content: &[BodyContent]) -> Vec<Option<&CT_P>> {
        content
            .iter()
            .map(|content| match content {
                BodyContent::Paragraph(paragraph) => Some(paragraph.as_ref()),
                _ => None,
            })
            .collect()
    }
    let original_paragraphs = paragraphs(&original.body.content);
    let edited_paragraphs = paragraphs(&edited.body.content);
    let (aligned, carried) = replace_changed_paragraph_runs(
        pair_gaps_by_kind(
            align(&original_signatures, &edited_signatures),
            &original_signatures,
            &edited_signatures,
        ),
        &original_paragraphs,
        &edited_paragraphs,
        &original_signatures,
        &edited_signatures,
        metadata.options,
    )?;
    refuse_uncarried_field_owners(
        &aligned,
        &original_paragraphs,
        &edited_paragraphs,
        &carried,
        location,
    )?;
    let moves = pair_moves(
        &aligned,
        &original_signatures,
        &edited_signatures,
        &mut metadata.ids,
    )?;
    let trailing_inserted = trailing_paragraphs(&aligned, |(left, right)| {
        right
            .filter(|_| left.is_none())
            .and_then(|index| edited.body.content.get(index))
    });
    let trailing_deleted = trailing_paragraphs(&aligned, |(left, right)| {
        left.filter(|_| right.is_none())
            .and_then(|index| original.body.content.get(index))
    });
    // A final paragraph change marks the paragraph mark of an owner already
    // emitted, which must not be one that holds an existing revision.
    let refuse_held_mark = |position: Option<usize>| -> Result<()> {
        let Some((left, right)) = position.map(|position| aligned[position]) else {
            return Ok(());
        };
        if held_original.holds(left) || held_edited.holds(right) {
            return Err(Error::Other(format!(
                "comparison cannot mark the paragraph mark of {location}/paragraph[{}] for the final paragraph change after it, because that paragraph holds an existing tracked revision",
                left.or(right).unwrap_or_default()
            )));
        }
        Ok(())
    };
    // The protected-owner exception admits only actual equal content and
    // formatting. Caller ignore policies still govern ordinary comparison.
    let protected_equality_options = ComparisonOptions::default();
    let mut output: Vec<(EmittedOwner, String)> = Vec::new();
    let mut trailing = (PropertySide::default(), false, false);
    let mut owner_leaves = Vec::new();
    for (position, (original_index, edited_index)) in aligned.iter().copied().enumerate() {
        let item_leaf_mark = metadata.leaf_publications.len();
        let item = |(left, right): (Option<usize>, Option<usize>)| {
            right
                .and_then(|index| edited.body.content.get(index))
                .or_else(|| left.and_then(|index| original.body.content.get(index)))
        };
        let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
            matches!(item(pair), Some(BodyContent::Table(_)))
        })
        .is_some_and(|pair| {
            matches!(
                item(pair),
                Some(BodyContent::Paragraph(_) | BodyContent::Table(_))
            )
        });
        match (original_index, edited_index) {
            (Some(left), Some(right)) => {
                let original_content = &original.body.content[left];
                let edited_content = &edited.body.content[right];
                let protected = held_original.holds(Some(left)) || held_edited.holds(Some(right));
                let same_source = held_original.same_owner_source(left, held_edited, right);
                let equal = original_content == edited_content
                    || !metadata.leaf_capture
                        && if protected {
                            coalesced_held_owner_equal(
                                original_content,
                                edited_content,
                                &protected_equality_options,
                                metadata,
                            )
                        } else {
                            match (original_content, edited_content) {
                                (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => {
                                    coalesced_paragraphs_equal(
                                        left,
                                        right,
                                        metadata.options,
                                        metadata,
                                    )
                                }
                                _ => false,
                            }
                        };
                // A protected owner both versions hold as the same tree,
                // written another way (a character as an entity, attributes
                // or namespace declarations in another order, as a save
                // writes them), is unchanged: the redline keeps the
                // original's bytes.
                let rewritten = protected
                    && !(equal && same_source)
                    && !metadata.leaf_capture
                    && (same_owner_tree(original_source, left, edited_source, right)?
                        || same_written_owner(
                            original_content,
                            edited_content,
                            original_source.map_or(&[][..], |(_, _, bindings)| bindings),
                            edited_source.2,
                        )?);
                let equal = equal || rewritten;
                let same_source = same_source || rewritten;
                let needs_field_dispatch = equal
                    && !(protected && same_source)
                    && SourceMetadata::body(
                        original_content,
                        &SourceSnapshots::default(),
                        metadata.options,
                    )?
                    .has_fields;
                let same_physical_source = if needs_field_dispatch
                    && let Some((source, spans, bindings)) = original_source
                {
                    if let Some((left, right)) = source
                        .get(spans[left].clone())
                        .zip(edited_source.0.get(edited_source.1[right].clone()))
                        && left == right
                    {
                        let external = |bindings: &[(String, String)]| {
                            bindings
                                .iter()
                                .map(|(name, uri)| {
                                    (
                                        name.strip_prefix("xmlns:").unwrap_or("").to_owned(),
                                        uri.clone(),
                                    )
                                })
                                .collect::<BTreeMap<_, _>>()
                        };
                        required_external_bindings(left.as_bytes(), &external(bindings))?
                            == required_external_bindings(
                                right.as_bytes(),
                                &external(edited_source.2),
                            )?
                    } else {
                        false
                    }
                } else {
                    false
                };
                // Global Field equality omits representation. Exact source and
                // required bindings additionally prove a field owner's no-op.
                let field_dispatch = needs_field_dispatch && !same_physical_source;
                let compared = if equal && !field_dispatch && (!protected || same_source) {
                    if let Some((source, spans, _)) = original_source {
                        source
                            .get(spans[left].clone())
                            .ok_or_else(|| {
                                Error::Other(format!(
                                    "comparison source span is invalid at {location}[{left}]"
                                ))
                            })?
                            .to_owned()
                    } else {
                        body_content_xml(original_content)?
                    }
                } else {
                    let held_reason = match (
                        held_original.holds(Some(left)),
                        held_edited.holds(Some(right)),
                    ) {
                        (true, true) => Some("the edited document changes it"),
                        (true, false) => Some(
                            "the edited document does not hold the revision the original holds there",
                        ),
                        (false, true) => Some(
                            "the edited document holds a tracked revision there that the original does not",
                        ),
                        (false, false) => None,
                    };
                    if let Some(reason) = held_reason {
                        return Err(Error::Other(format!(
                            "comparison cannot change {}, which holds an existing tracked revision: {reason}",
                            body_location(location, original_content, left)
                        )));
                    }
                    let content_source = original_source
                        .map(|(source, spans, bindings)| {
                            source_fragment(&source[spans[left].clone()], bindings)
                        })
                        .transpose()?;
                    compare_body_content(
                        original_content,
                        edited_content,
                        content_source.as_deref(),
                        &body_location(location, edited_content, right),
                        metadata,
                        diagnostics,
                    )?
                };
                let owner = match (original_content, edited_content) {
                    (BodyContent::Table(_), BodyContent::Table(_)) => EmittedOwner::KeptTable,
                    _ => EmittedOwner::of(original_content, edited_content, None, metadata)?,
                };
                output.push((owner, compared));
            }
            (Some(left), None) => {
                let content = &original.body.content[left];
                if held_original.holds(Some(left)) {
                    let place = body_location(location, content, left);
                    return Err(Error::Other(if moves.original[left].is_some() {
                        format!(
                            "comparison cannot move {place}, which holds an existing tracked revision"
                        )
                    } else if carried.original.contains(&left) {
                        format!(
                            "comparison cannot replace {place} whole, because it holds an existing tracked revision"
                        )
                    } else {
                        format!(
                            "comparison cannot delete {place}, which holds an existing tracked revision"
                        )
                    }));
                }
                let move_id = moves.original[left];
                let kind = if move_id.is_some() { "moveFrom" } else { "del" };
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some((first, last)) =
                        trailing_deleted.filter(|&(first, _)| position >= first)
                {
                    trailing_paragraph(
                        &mut output,
                        TrailingRun {
                            content,
                            paragraph,
                            kind,
                            move_id,
                            first: position == first,
                            single: first == last,
                            next_takes_mark,
                            location,
                        },
                        &mut trailing,
                        &refuse_held_mark,
                        source_bindings,
                        metadata,
                    )?;
                } else if let BodyContent::Paragraph(paragraph) = content
                    && !next_takes_mark
                    && !after_kept_table(&output, kind, location)
                {
                    refuse_held_mark(previous_mark_position(&output, kind))?;
                    let merged = mark_before_final_paragraph(
                        &mut output,
                        kind,
                        move_id,
                        source_bindings,
                        metadata,
                    )?;
                    output.push(final_paragraph(paragraph, kind, move_id, merged, metadata)?);
                } else {
                    output.push(unmatched_body_owner(content, kind, move_id, metadata)?);
                }
            }
            (None, Some(right)) => {
                let content = &edited.body.content[right];
                if held_edited.holds(Some(right)) {
                    let action = if moves.edited[right].is_some() {
                        "move"
                    } else {
                        "insert"
                    };
                    return Err(Error::Other(format!(
                        "comparison cannot {action} edited {}, which holds a tracked revision that the original does not hold there",
                        body_location(location, content, right)
                    )));
                }
                let move_id = moves.edited[right];
                let kind = if move_id.is_some() { "moveTo" } else { "ins" };
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some((first, last)) =
                        trailing_inserted.filter(|&(first, _)| position >= first)
                {
                    trailing_paragraph(
                        &mut output,
                        TrailingRun {
                            content,
                            paragraph,
                            kind,
                            move_id,
                            first: position == first,
                            single: first == last,
                            next_takes_mark,
                            location,
                        },
                        &mut trailing,
                        &refuse_held_mark,
                        source_bindings,
                        metadata,
                    )?;
                } else if let BodyContent::Paragraph(paragraph) = content
                    && !next_takes_mark
                    && !after_kept_table(&output, kind, location)
                {
                    refuse_held_mark(previous_mark_position(&output, kind))?;
                    let merged = mark_before_final_paragraph(
                        &mut output,
                        kind,
                        move_id,
                        source_bindings,
                        metadata,
                    )?;
                    output.push(final_paragraph(paragraph, kind, move_id, merged, metadata)?);
                } else {
                    output.push(unmatched_body_owner(content, kind, move_id, metadata)?);
                }
            }
            (None, None) => unreachable!(),
        }
        owner_leaves.push(metadata.leaf_publications.split_off(item_leaf_mark));
    }
    let mut output = if let Some((source, spans, bindings)) = original_source {
        let section_start = if original.body.sect_pr.is_some() {
            direct_word_child_start(source, bindings, b"sectPr")?
        } else {
            None
        };
        interleave_story_source(
            source,
            spans,
            &aligned,
            output,
            section_start,
            bindings,
            edited_source,
            &mut owner_leaves,
            &mut metadata.leaf_publications,
        )?
    } else {
        let mut result = String::new();
        for ((_, xml), mut leaves) in output.into_iter().zip(owner_leaves) {
            shift_leaf_publications(&mut leaves, result.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
            result.push_str(&xml);
        }
        result
    };
    let same_section_source = held_original
        .section_source
        .as_ref()
        .zip(held_edited.section_source.as_ref())
        .is_some_and(|(original, edited)| original == edited);
    if (held_original.section || held_edited.section)
        && (original.body.sect_pr != edited.body.sect_pr || !same_section_source)
    {
        return Err(Error::Other(format!(
            "comparison cannot change the section properties of {location}, which hold an existing tracked revision"
        )));
    }
    let section_xml = section_properties_xml(
        original.body.sect_pr.as_ref(),
        edited.body.sect_pr.as_ref(),
        "section",
        metadata,
        diagnostics,
    )?;
    let section_xml = if let Some((source, _, bindings)) = original_source {
        let wrapper = source_fragment(&format!("<source>{source}</source>"), bindings)?;
        if let Some(span) = direct_word_element_spans(&wrapper, "sectPr")?.first() {
            if held_original.section {
                wrapper[span.clone()].to_owned()
            } else {
                let raw = source_child(&wrapper, span.clone())?;
                let before = original
                    .body
                    .sect_pr
                    .as_ref()
                    .map(section_property_xml)
                    .transpose()?
                    .unwrap_or_default();
                let current =
                    patch_property_source(&before, &section_xml, Some(&raw), &metadata.ids)?;
                preserve_prior_property_sources(&current, &raw)?
            }
        } else {
            section_xml
        }
    } else {
        section_xml
    };
    output.push_str(&section_xml);
    Ok(output)
}

/// Remove only closure added to a modeled source owner's root when it is
/// published back under identical inherited bindings. Authored declarations,
/// non-declaration attributes and every descendant byte remain untouched.
fn retain_source_root_namespaces(
    published: &str,
    source: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let scoped = source_fragment(source, destination)?;
    let mut source_reader = NsReader::from_str(&scoped);
    let (namespace, event) = source_reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Ok(published.to_owned());
    };
    if !matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == W_NS.as_bytes())
        || !matches!(
            root.local_name().as_ref(),
            b"p" | b"tbl"
                | b"sdt"
                | b"hyperlink"
                | b"r"
                | b"t"
                | b"delText"
                | b"instrText"
                | b"delInstrText"
                | b"fldChar"
                | b"drawing"
                | b"tab"
                | b"br"
                | b"sym"
                | b"footnoteReference"
                | b"endnoteReference"
                | b"commentReference"
                | b"cr"
                | b"noBreakHyphen"
                | b"softHyphen"
                | b"ptab"
        )
    {
        return Ok(published.to_owned());
    }
    restore_extracted_root(published, source, destination)
}

/// Source-only namespace restoration after an actual retained owner has been
/// established. It does not participate in comparison identity or selection.
fn restore_extracted_root(
    published: &str,
    source: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let mut original = Reader::from_str(source);
    let (Event::Start(original) | Event::Empty(original)) = original
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    else {
        return Ok(published.to_owned());
    };
    let authored = declared_prefixes(&original)?;
    let original_scope = source_fragment(source, destination)?;
    let mut qualified_original = NsReader::from_str(&original_scope);
    let (source_namespace, _) = qualified_original
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let source_namespace = match source_namespace {
        ResolveResult::Bound(uri) => Some(uri.as_ref().to_vec()),
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(_) => {
            return Err(Error::Other(
                "retained source root namespace is unresolved".into(),
            ));
        }
    };
    let published_scope = source_fragment(published, destination)?;
    let mut qualified = NsReader::from_str(&published_scope);
    let (namespace, qualified_event) = qualified
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let namespace = match namespace {
        ResolveResult::Bound(uri) => Some(uri.as_ref().to_vec()),
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(_) => {
            return Err(Error::Other(
                "published retained root namespace is unresolved".into(),
            ));
        }
    };
    if namespace != source_namespace
        || !matches!(qualified_event, Event::Start(_) | Event::Empty(_))
    {
        return Ok(published.to_owned());
    }
    let mut reader = Reader::from_str(published);
    let event = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(element) | Event::Empty(element)) = event else {
        return Ok(published.to_owned());
    };
    if element.local_name().as_ref() != element_local_name(source).as_bytes() {
        return Ok(published.to_owned());
    }
    let mut remove = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let name = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        let prefix = if name == "xmlns" {
            ""
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            prefix
        } else {
            continue;
        };
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| Error::Other(error.to_string()))?;
        if authored.contains(prefix)
            || !destination
                .iter()
                .any(|(key, uri)| key == name && uri == value.as_ref())
        {
            continue;
        }
        // The borrowed event retains the exact source slices, including quotes.
        let key = attribute.key.as_ref().as_ptr() as usize - published.as_ptr() as usize;
        let end = attribute.value.as_ptr() as usize - published.as_ptr() as usize
            + attribute.value.len()
            + 1;
        if key == 0
            || end > published.len()
            || !matches!(published.as_bytes()[end - 1], b'\'' | b'"')
        {
            return Err(Error::Other(
                "namespace declaration has no exact source range".into(),
            ));
        }
        let start = if published.as_bytes()[key - 1].is_ascii_whitespace() {
            key - 1
        } else {
            key
        };
        remove.push(start..end);
    }
    let mut output = published.to_owned();
    for range in remove.into_iter().rev() {
        output.replace_range(range, "");
    }
    Ok(output)
}

/// Current pPr and its mark rPr retain their actual owner context. Prior
/// change payloads have different roles and deliberately keep their closure.
fn restore_current_property_roots(
    published: &str,
    actual: &str,
    destination: &[(String, String)],
) -> Result<String> {
    let mut output = restore_extracted_root(published, actual, destination)?;
    if element_local_name(actual) == "pPr" {
        let scoped = source_fragment(actual, destination)?;
        if let (Some(source_span), Some(output_span)) = (
            direct_word_element_spans(&scoped, "rPr")?.first(),
            direct_word_element_spans(&output, "rPr")?.first(),
        ) {
            let delta = element_opening(&scoped)?
                .0
                .checked_sub(element_opening(actual)?.0)
                .ok_or_else(|| {
                    Error::Other("property extraction shortened its source root".into())
                })?;
            if scoped[element_opening(&scoped)?.0..] != actual[element_opening(actual)?.0..] {
                return Err(Error::Other(
                    "property extraction changed descendant bytes".into(),
                ));
            }
            let context = root_namespace_bindings(&output, "pPr", destination)?;
            let restored = restore_extracted_root(
                &output[output_span.clone()],
                &actual[source_span.start - delta..source_span.end - delta],
                &context,
            )?;
            output.replace_range(output_span.clone(), &restored);
        }
    }
    Ok(output)
}

fn interleave_story_source(
    source: &str,
    spans: &[Range<usize>],
    aligned: &[(Option<usize>, Option<usize>)],
    output: Vec<(EmittedOwner<'_>, String)>,
    section_start: Option<usize>,
    bindings: &[(String, String)],
    edited_source: OriginalBodySource<'_>,
    owner_leaves: &mut [Vec<LeafPublication>],
    publications: &mut Vec<LeafPublication>,
) -> Result<String> {
    if spans.len() != aligned.iter().filter(|(left, _)| left.is_some()).count()
        || aligned.len() != output.len()
    {
        return Err(Error::Other(
            "comparison source spans do not match related-story owners".to_owned(),
        ));
    }
    let content_end = section_start.unwrap_or(source.len());
    let mut tracked = String::new();
    if let Some(first) = spans.first() {
        tracked.push_str(&source[..first.start]);
    } else {
        tracked.push_str(&source[..content_end]);
    }
    let mut next_original = 0usize;
    for (owner_index, ((left, right), (_, xml))) in aligned.iter().zip(output).enumerate() {
        if let Some(index) = left {
            if *index != next_original {
                return Err(Error::Other(
                    "comparison related-story owner order is not monotonic".to_owned(),
                ));
            }
            if *index > 0 {
                tracked.push_str(&source[spans[index - 1].end..spans[*index].start]);
            }
            next_original += 1;
        }
        let template = left
            .map(|index| &source[spans[index].clone()])
            .or_else(|| right.map(|index| &edited_source.0[edited_source.1[index].clone()]));
        let published = if let Some(template) = template {
            retain_source_root_namespaces(&xml, template, bindings)?
        } else {
            xml.clone()
        };
        let delta = element_opening(&published)?.0 as isize - element_opening(&xml)?.0 as isize;
        let mut leaves = std::mem::take(&mut owner_leaves[owner_index]);
        shift_leaf_publications(&mut leaves, tracked.len() as isize + delta)?;
        publications.extend(leaves);
        tracked.push_str(&published);
    }
    if let Some(last) = spans.last() {
        if last.end > content_end {
            return Err(Error::Other(
                "comparison section properties overlap body content".to_owned(),
            ));
        }
        tracked.push_str(&source[last.end..content_end]);
    }
    Ok(tracked)
}

fn direct_word_child_start(
    xml: &str,
    bindings: &[(String, String)],
    local_name: &[u8],
) -> Result<Option<usize>> {
    let (wrapped, offset) = story_wrapper(xml, bindings)?;
    let mut reader = NsReader::from_reader(wrapped.as_slice());
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_word = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()
        );
        match event {
            Event::Start(element) => {
                if depth == 2 && is_word && element.local_name().as_ref() == local_name {
                    return Ok(Some(before.saturating_sub(offset)));
                }
                depth += 1;
            }
            Event::Empty(element)
                if depth == 2 && is_word && element.local_name().as_ref() == local_name =>
            {
                return Ok(Some(before.saturating_sub(offset)));
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn moved_body_content(
    content: &BodyContent,
    kind: &str,
    id: i32,
    metadata: &Metadata<'_>,
) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => moved_paragraph(paragraph, kind, id, metadata),
        BodyContent::Table(table) => moved_table(table, kind, id, metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot move a content-control or opaque body node".to_owned(),
        )),
    }
}

fn moved_paragraph(
    paragraph: &CT_P,
    kind: &str,
    id: i32,
    metadata: &Metadata<'_>,
) -> Result<String> {
    refuse_moved_markers(paragraph)?;
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let source = PropertySide::of(paragraph, metadata)?;
    let properties = marked_paragraph_properties_xml(
        &source
            .xml
            .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?),
        &marker,
        &metadata.ids,
    )?;
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&properties);
    let children = paragraph_children_xml(paragraph, false, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        false,
        false,
        &mut |content| {
            metadata.ids.generated(IdAllocator::revision_with_id(
                kind,
                metadata.author,
                metadata.timestamp,
                content,
                id,
            ))
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

/// Both ends of a move would hold the paragraph's bookmarks and comment
/// ranges, and each may appear once.
fn refuse_moved_markers(paragraph: &CT_P) -> Result<()> {
    if paragraph.bookmark_markers.is_empty() && paragraph.comment_ranges.is_empty() {
        return Ok(());
    }
    Err(Error::Other(
        "comparison cannot move a paragraph that holds a bookmark or comment range".to_owned(),
    ))
}

fn moved_table(table: &CT_Tbl, kind: &str, id: i32, metadata: &Metadata<'_>) -> Result<String> {
    let source = metadata
        .table_source(table)
        .map(str::to_owned)
        .map_or_else(|| table_xml(table), Ok)?;
    let replacements = table
        .rows
        .iter()
        .map(|row| moved_row(row, kind, id, metadata))
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&source, "tr", &replacements)?)
}

fn moved_row(row: &CT_Row, kind: &str, id: i32, metadata: &Metadata<'_>) -> Result<String> {
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let mut xml = metadata
        .row_source(row)
        .map(str::to_owned)
        .map_or_else(|| row_xml(row), Ok)?;
    let properties = direct_word_element_spans(&xml, "trPr")?;
    if let Some(span) = properties.first() {
        let updated = append_word_child(&xml[span.clone()], "trPr", &marker)?;
        xml.replace_range(span.clone(), &updated);
        metadata.ids.generated(xml)
    } else {
        metadata.ids.generated(insert_first_child(
            &xml,
            &metadata
                .ids
                .generated(format!("<w:trPr>{marker}</w:trPr>"))?,
        )?)
    }
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

/// What a later final paragraph change can do with an owner already emitted.
#[derive(Clone)]
enum EmittedOwner<'a> {
    /// A paragraph whose mark can carry the change, with its properties after
    /// acceptance and after rejection.
    Paragraph {
        accepted: PropertySide<'a>,
        rejected: PropertySide<'a>,
    },
    /// A table whose every row carries a marker of this kind (`ins`, `del`,
    /// `moveFrom` or `moveTo`). A change that the same resolution removes
    /// crosses it, because that resolution removes the table and leaves the
    /// paragraphs on either side adjacent; the other resolution keeps it.
    MarkedTable(&'static str),
    /// Source whitespace between owners, which every change crosses.
    Whitespace,
    /// A table both versions hold, which resolving the comparison's
    /// revisions keeps. A final paragraph after it carries its own change:
    /// the resolution removing its mark removes it, and the body ends with
    /// the table, as it did on that side. When existing revisions in the
    /// table remove it too, resolving them with the change, the paragraph
    /// goes with the table, and the body ends with what came before it.
    KeptTable,
    Other,
}

impl<'a> EmittedOwner<'a> {
    /// The owner emitted for an original and an edited item, the same item
    /// when only one side has it.
    fn of(
        original: &'a BodyContent,
        edited: &'a BodyContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (BodyContent::Paragraph(original), BodyContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, BodyContent::Table(_), Some(kind)) => Ok(Self::MarkedTable(kind)),
            _ => Ok(Self::Other),
        }
    }

    fn of_control(
        original: &'a SdtContent,
        edited: &'a SdtContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (SdtContent::Paragraph(original), SdtContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, SdtContent::Table(_), Some(kind)) => Ok(Self::MarkedTable(kind)),
            _ => Ok(Self::Other),
        }
    }

    fn of_cell(
        original: &'a CellContent,
        edited: &'a CellContent,
        marked_kind: Option<&'static str>,
        metadata: &Metadata<'_>,
    ) -> Result<Self> {
        match (original, edited, marked_kind) {
            (CellContent::Paragraph(original), CellContent::Paragraph(edited), _) => {
                Self::paragraph(original, edited, metadata)
            }
            (_, CellContent::Table(_), Some(kind)) => Ok(Self::MarkedTable(kind)),
            _ => Ok(Self::Other),
        }
    }

    fn paragraph(original: &'a CT_P, edited: &'a CT_P, metadata: &Metadata<'_>) -> Result<Self> {
        let (accepted, rejected) = PropertySide::matched(original, edited, metadata)?;
        Ok(Self::Paragraph { accepted, rejected })
    }
}

/// The aligned owner after an inserted, deleted or moved one at `position`
/// that the resolution removing its paragraph mark keeps. A table on the same
/// side is crossed, because that resolution removes every row of it.
///
/// When that owner is a paragraph or a table, the paragraph at `position`
/// carries its own change: removing its mark merges it into that paragraph,
/// or, before a table, removes the paragraph, which has no content left.
/// Otherwise its mark ends the story, and [`final_paragraph`] applies.
fn next_kept_owner(
    aligned: &[(Option<usize>, Option<usize>)],
    position: usize,
    is_table: impl Fn((Option<usize>, Option<usize>)) -> bool,
) -> Option<(Option<usize>, Option<usize>)> {
    let (left, right) = aligned[position];
    aligned[position + 1..]
        .iter()
        .copied()
        .find(|&(next_left, next_right)| {
            next_left.is_some() != left.is_some()
                || next_right.is_some() != right.is_some()
                || !is_table((next_left, next_right))
        })
}

/// Whether accepting, rather than rejecting, removes content marked `kind`.
fn accept_removes(kind: &str) -> bool {
    matches!(kind, "del" | "moveFrom")
}

/// The first and last positions of the paragraphs in the run of owners
/// that one side alone holds (`one_sided` gives the owner) and that ends
/// the body. The tables between them are crossed: the mark before the
/// first paragraph carries the change of the last one, whose own mark ends
/// the story.
fn trailing_paragraphs<'d>(
    aligned: &[(Option<usize>, Option<usize>)],
    one_sided: impl Fn((Option<usize>, Option<usize>)) -> Option<&'d BodyContent>,
) -> Option<(usize, usize)> {
    let paragraphs = aligned
        .iter()
        .enumerate()
        .rev()
        .map_while(|(position, &pair)| {
            one_sided(pair)
                .filter(|content| {
                    matches!(content, BodyContent::Paragraph(_) | BodyContent::Table(_))
                })
                .map(|content| (position, content))
        })
        .filter(|(_, content)| matches!(content, BodyContent::Paragraph(_)))
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    paragraphs.last().copied().zip(paragraphs.first().copied())
}

/// The position in `output` of the owner whose paragraph mark
/// [`mark_previous_paragraph`] marks for a final `kind` change.
fn previous_mark_position(output: &[(EmittedOwner<'_>, String)], kind: &str) -> Option<usize> {
    output.iter().rposition(|(owner, _)| match owner {
        EmittedOwner::MarkedTable(marked) => accept_removes(marked) != accept_removes(kind),
        EmittedOwner::Whitespace => false,
        EmittedOwner::Paragraph { .. } | EmittedOwner::KeptTable | EmittedOwner::Other => true,
    })
}

/// Whether the owner a final `kind` change would mark is a table this
/// change's resolution keeps: one both versions hold, or one the other
/// resolution removes (a table this resolution removes is crossed before
/// it). The final paragraph then carries its own change.
fn after_kept_table(output: &[(EmittedOwner<'_>, String)], kind: &str, location: &str) -> bool {
    // Only the body may end with a table; every other story ends with a
    // paragraph.
    location == "body"
        && matches!(
            previous_mark_position(output, kind).map(|position| &output[position].0),
            Some(EmittedOwner::KeptTable | EmittedOwner::MarkedTable(_))
        )
}

/// Mark the paragraph before a final inserted or deleted paragraph, and
/// return its properties on the side that removes that mark, which
/// [`final_paragraph`] gives the paragraph the two merge into.
fn mark_previous_paragraph<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<PropertySide<'a>> {
    let Some((EmittedOwner::Paragraph { accepted, rejected }, paragraph)) =
        previous_mark_position(output, kind).map(|position| &mut output[position])
    else {
        return Err(Error::Other(
            "comparison needs an adjacent paragraph for a final paragraph change".to_owned(),
        ));
    };
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let scoped = source_fragment(paragraph, bindings)?;
    *paragraph = marked_paragraph_xml(&scoped, &marker, &metadata.ids)?;
    Ok(if kind == "del" {
        accepted.clone()
    } else {
        rejected.clone()
    })
}

fn marked_paragraph_xml(paragraph: &str, marker: &str, ids: &IdAllocator) -> Result<String> {
    let paragraph_properties = direct_word_element_spans(paragraph, "pPr")?;
    if let Some(properties_span) = paragraph_properties.first() {
        let properties = source_child(paragraph, properties_span.clone())?;
        let updated = marked_paragraph_properties_xml(&properties, marker, ids)?;
        let mut marked = paragraph.to_owned();
        marked.replace_range(properties_span.clone(), &updated);
        return Ok(marked);
    }
    let properties = ids.generated(format!("<w:pPr><w:rPr>{marker}</w:rPr></w:pPr>"))?;
    insert_first_child(paragraph, &properties)
}

/// Put a paragraph-mark revision marker into serialized `w:pPr` in schema
/// order: first in `w:rPr`, and a new `w:rPr` before `w:sectPr` and
/// `w:pPrChange`.
fn marked_paragraph_properties_xml(
    properties: &str,
    marker: &str,
    ids: &IdAllocator,
) -> Result<String> {
    if let Some(run_span) = direct_word_element_spans(properties, "rPr")?.first() {
        let run_properties = source_child(properties, run_span.clone())?;
        let updated_run = with_mark_revision(&run_properties, marker)?;
        let mut updated = properties.to_owned();
        updated.replace_range(run_span.clone(), &updated_run);
        return Ok(updated);
    }
    let run_properties = ids.generated(format!("<w:rPr>{marker}</w:rPr>"))?;
    let later = direct_word_element_spans(properties, "sectPr")?
        .into_iter()
        .chain(direct_word_element_spans(properties, "pPrChange")?)
        .map(|span| span.start)
        .min();
    if let Some(start) = later {
        let mut updated = properties.to_owned();
        updated.insert_str(start, &run_properties);
        return Ok(updated);
    }
    append_word_child(properties, "pPr", &run_properties)
}

/// A paragraph of the run of owners one side alone holds that ends the
/// body.
struct TrailingRun<'a, 'l> {
    content: &'a BodyContent,
    paragraph: &'a CT_P,
    kind: &'static str,
    move_id: Option<i32>,
    /// Whether it is the first paragraph of the run, and the only one.
    first: bool,
    single: bool,
    next_takes_mark: bool,
    location: &'l str,
}

/// Emit a paragraph of the run that ends the body, with `trailing` the
/// properties [`final_paragraph`] merges, whether a kept table precedes the
/// run, and whether the run paragraph emitted last is moved.
///
/// The first paragraph marks the paragraph before the run, unless a kept
/// table precedes it: with its own change when it is the only one, else
/// with a mark of the run's resolution under a fresh id, so that resolving
/// one move of the run alone merges nothing else. Each paragraph but the
/// last carries its own change. The last keeps its mark as the story
/// terminator; when it is moved, its move also goes on the mark of the
/// paragraph before it in the run, which resolving that move alone merges
/// into it, unless that paragraph is moved too: a mark holds one move
/// marker of a kind, and resolving the last move alone then leaves its
/// paragraph empty.
fn trailing_paragraph<'a>(
    output: &mut Vec<(EmittedOwner<'a>, String)>,
    run: TrailingRun<'a, '_>,
    trailing: &mut (PropertySide<'a>, bool, bool),
    refuse_held_mark: &dyn Fn(Option<usize>) -> Result<()>,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<()> {
    let (merged, after_table, previous_moved) = (&mut trailing.0, &mut trailing.1, &mut trailing.2);
    if run.first {
        let (kind, move_id) = if run.single {
            (run.kind, run.move_id)
        } else if accept_removes(run.kind) {
            ("del", None)
        } else {
            ("ins", None)
        };
        *after_table = after_kept_table(output, kind, run.location);
        if !*after_table {
            refuse_held_mark(previous_mark_position(output, kind))?;
            *merged = mark_before_final_paragraph(output, kind, move_id, bindings, metadata)?;
        }
    }
    if run.next_takes_mark || *after_table {
        output.push(unmatched_body_owner(
            run.content,
            run.kind,
            run.move_id,
            metadata,
        )?);
        *previous_moved = run.move_id.is_some();
        return Ok(());
    }
    if let Some(id) = run.move_id
        && !run.single
        && !*previous_moved
    {
        mark_previous_paragraph_with_id(output, run.kind, id, bindings, metadata)?;
    }
    output.push(final_paragraph(
        run.paragraph,
        run.kind,
        run.move_id,
        merged.clone(),
        metadata,
    )?);
    Ok(())
}

/// A body owner one side alone holds, written with its own `kind` marks:
/// moved under `move_id`, deleted or inserted.
fn unmatched_body_owner<'a>(
    content: &'a BodyContent,
    kind: &'static str,
    move_id: Option<i32>,
    metadata: &mut Metadata<'_>,
) -> Result<(EmittedOwner<'a>, String)> {
    let xml = match (move_id, kind) {
        (Some(id), _) => moved_body_content(content, kind, id, metadata)?,
        (None, "del") => deleted_body_content(content, metadata)?,
        (None, _) => inserted_body_content(content, metadata)?,
    };
    Ok((
        EmittedOwner::of(content, content, Some(kind), metadata)?,
        xml,
    ))
}

/// Mark the paragraph before a final `kind` paragraph, with the id of its
/// move pair when it is moved, and return what [`final_paragraph`] merges.
fn mark_before_final_paragraph<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    move_id: Option<i32>,
    bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
) -> Result<PropertySide<'a>> {
    match move_id {
        Some(id) => mark_previous_paragraph_with_id(output, kind, id, bindings, metadata),
        None => mark_previous_paragraph(output, kind, bindings, metadata),
    }
}

/// [`mark_previous_paragraph`] for a final moved paragraph, whose move
/// pair shares `id`.
fn mark_previous_paragraph_with_id<'a>(
    output: &mut [(EmittedOwner<'a>, String)],
    kind: &str,
    id: i32,
    bindings: &[(String, String)],
    metadata: &Metadata<'_>,
) -> Result<PropertySide<'a>> {
    let Some((EmittedOwner::Paragraph { accepted, rejected }, paragraph)) =
        previous_mark_position(output, kind).map(|position| &mut output[position])
    else {
        return Err(Error::Other(
            "comparison needs an adjacent paragraph for a final paragraph move".to_owned(),
        ));
    };
    let marker = metadata.ids.generated(IdAllocator::marker_with_id(
        kind,
        metadata.author,
        metadata.timestamp,
        id,
    ))?;
    let scoped = source_fragment(paragraph, bindings)?;
    *paragraph = marked_paragraph_xml(&scoped, &marker, &metadata.ids)?;
    Ok(if kind == "moveFrom" {
        accepted.clone()
    } else {
        rejected.clone()
    })
}

/// A final inserted, deleted or moved paragraph. Its own mark stays as the
/// story terminator, and the mark of the paragraph before it carries the
/// change. A move wraps the runs with the `move_id` of its pair.
///
/// Removing that mark merges the paragraph before into this one, and the
/// merged paragraph keeps these properties. `merged` holds the properties the
/// paragraph before has on that side, so this paragraph carries the accepted
/// properties with a `w:pPrChange` and a mark `w:rPrChange` that hold the
/// rejected ones, and both resolutions keep each paragraph's properties.
fn final_paragraph<'a>(
    paragraph: &'a CT_P,
    kind: &str,
    move_id: Option<i32>,
    merged: PropertySide<'a>,
    metadata: &mut Metadata<'_>,
) -> Result<(EmittedOwner<'a>, String)> {
    let own = PropertySide::of(paragraph, metadata)?;
    let (accepted, rejected) = if matches!(kind, "del" | "moveFrom") {
        (merged, own.clone())
    } else {
        (own.clone(), merged)
    };
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&final_paragraph_properties_xml(
        &own, &accepted, &rejected, metadata,
    )?);
    let children = paragraph_children_xml(paragraph, kind == "del", metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        kind == "del",
        false,
        &mut |content| match move_id {
            Some(id) => metadata.ids.generated(IdAllocator::revision_with_id(
                kind,
                metadata.author,
                metadata.timestamp,
                content,
                id,
            )),
            None => metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, content),
        },
    )?);
    output.push_str(&closing);
    Ok((
        EmittedOwner::Paragraph { accepted, rejected },
        metadata.ids.generated(output)?,
    ))
}

fn compare_body_content(
    original: &BodyContent,
    edited: &BodyContent,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => compare_paragraph(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (BodyContent::Table(left), BodyContent::Table(right)) => compare_table(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (BodyContent::ContentControl(left), BodyContent::ContentControl(right)) => {
            if let Some(source) = original_source {
                compare_control_from_xml(left, right, location, metadata, diagnostics, source)
            } else {
                compare_control(left, right, location, metadata, diagnostics)
            }
        }
        _ if body_signature(original) == body_signature(edited) => body_content_xml(original),
        _ => Err(Error::Other(format!(
            "comparison cannot replace unlike body structures at {location}"
        ))),
    }
}

fn deleted_body_content(content: &BodyContent, metadata: &mut Metadata<'_>) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => deleted_paragraph(paragraph, metadata),
        BodyContent::Table(table) => marked_table(table, "del", metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot delete an unmatched content-control or opaque body node".to_owned(),
        )),
    }
}

fn inserted_body_content(content: &BodyContent, metadata: &mut Metadata<'_>) -> Result<String> {
    match content {
        BodyContent::Paragraph(paragraph) => inserted_paragraph(paragraph, metadata),
        BodyContent::Table(table) => marked_table(table, "ins", metadata),
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => Err(Error::Other(
            "comparison cannot insert an unmatched content-control or opaque body node".to_owned(),
        )),
    }
}

fn coalescible_plain_run(run: &CT_R) -> bool {
    !run.content.is_empty()
        && run.extra_xml.is_empty()
        && run.alt_drawings.is_empty()
        && run.content.iter().all(|content| {
            matches!(
                content,
                RunContent::Text(_) | RunContent::Tab | RunContent::Break(_)
            )
        })
}

fn coalesced_paragraph_signature(paragraph: &CT_P, options: &ComparisonOptions) -> String {
    let scalar = ComparisonOptions {
        granularity: ComparisonGranularity::Character,
        ..options.clone()
    };
    let mut properties = Vec::<(Option<&oxoffice_docx_oxml::properties::CT_RPr>, usize)>::new();
    if !options.ignore_formatting {
        for run in paragraph
            .runs
            .iter()
            .filter(|run| coalescible_plain_run(run))
        {
            let width = run
                .content
                .iter()
                .map(|content| match content {
                    RunContent::Text(text) => text
                        .text
                        .chars()
                        .filter(|character| {
                            !options.ignore_whitespace || !character.is_whitespace()
                        })
                        .count(),
                    _ => 1,
                })
                .sum::<usize>();
            // Empty text and ignored whitespace still carry a real format.
            let current = run_formatting(run);
            if let Some((previous, count)) = properties.last_mut()
                && *previous == current
            {
                *count += width;
            } else {
                properties.push((current, width));
            }
        }
    }
    // Character publication protects every new whitespace boundary. On the
    // whole-run path, retain the actual unprotected edge characters as
    // additional content facts before using that publication view as equality.
    // Internal whitespace and explicitly protected split boundaries stay equal.
    let mut unprotected_edges = Vec::new();
    if !uses_attributed_run_path(options) {
        let xml_space = |character: char| matches!(character, ' ' | '\t' | '\n' | '\r');
        let mut offset = 0usize;
        for run in &paragraph.runs {
            for content in &run.content {
                if let RunContent::Text(text) = content {
                    let leading = text.text.len() - text.text.trim_start_matches(xml_space).len();
                    let trailing = text.text.trim_end_matches(xml_space).len();
                    for (byte, character) in text.text.char_indices() {
                        if !text.preserve_space
                            && xml_space(character)
                            && (byte < leading || byte >= trailing)
                        {
                            unprotected_edges.push(offset);
                        }
                        offset += 1;
                    }
                } else {
                    offset += 1;
                }
            }
        }
    }
    format!(
        "{}:{properties:?}:{unprotected_edges:?}",
        paragraph_signature_with_options(paragraph, &scalar)
    )
}

/// Compare complete protected owners, coalescing only their actual paragraph
/// leaves. Shell clones are equality-only and never become source lookups or
/// publications, so private control state remains part of the proof.
fn coalesced_held_owner_equal(
    left: &BodyContent,
    right: &BodyContent,
    options: &ComparisonOptions,
    metadata: &Metadata<'_>,
) -> bool {
    fn controls(
        left: &[(usize, usize, CT_Sdt)],
        right: &[(usize, usize, CT_Sdt)],
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|((a, before_a, left), (b, before_b, right))| {
                    a == b && before_a == before_b && control(left, right, options, metadata)
                })
    }
    fn table(
        left: &CT_Tbl,
        right: &CT_Tbl,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.rows.len() != right.rows.len()
            || left.content_controls.len() != right.content_controls.len()
        {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.rows.clear();
        b.rows.clear();
        a.content_controls.clear();
        b.content_controls.clear();
        a == b
            && left
                .rows
                .iter()
                .zip(&right.rows)
                .all(|(left, right)| row(left, right, options, metadata))
            && controls(
                &left.content_controls,
                &right.content_controls,
                options,
                metadata,
            )
    }
    fn row(
        left: &CT_Row,
        right: &CT_Row,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.cells.len() != right.cells.len()
            || left.content_controls.len() != right.content_controls.len()
        {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.cells.clear();
        b.cells.clear();
        a.content_controls.clear();
        b.content_controls.clear();
        a == b
            && left
                .cells
                .iter()
                .zip(&right.cells)
                .all(|(left, right)| cell(left, right, options, metadata))
            && controls(
                &left.content_controls,
                &right.content_controls,
                options,
                metadata,
            )
    }
    fn cell(
        left: &CT_Tc,
        right: &CT_Tc,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.content.len() != right.content.len() {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.content.clear();
        b.content.clear();
        a == b
            && left
                .content
                .iter()
                .zip(&right.content)
                .all(|(left, right)| match (left, right) {
                    (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
                        coalesced_paragraphs_equal(left, right, options, metadata)
                    }
                    (CellContent::Table(left), CellContent::Table(right)) => {
                        table(left, right, options, metadata)
                    }
                    (CellContent::ContentControl(left), CellContent::ContentControl(right)) => {
                        control(left, right, options, metadata)
                    }
                    _ => false,
                })
    }
    fn control(
        left: &CT_Sdt,
        right: &CT_Sdt,
        options: &ComparisonOptions,
        metadata: &Metadata<'_>,
    ) -> bool {
        if left.content.len() != right.content.len() {
            return false;
        }
        let mut a = left.clone();
        let mut b = right.clone();
        a.content.clear();
        b.content.clear();
        a == b
            && left
                .content
                .iter()
                .zip(&right.content)
                .all(|(left, right)| match (left, right) {
                    (SdtContent::Paragraph(left), SdtContent::Paragraph(right)) => {
                        coalesced_paragraphs_equal(left, right, options, metadata)
                    }
                    (SdtContent::Table(left), SdtContent::Table(right)) => {
                        table(left, right, options, metadata)
                    }
                    (SdtContent::Row(left), SdtContent::Row(right)) => {
                        row(left, right, options, metadata)
                    }
                    (SdtContent::Cell(left), SdtContent::Cell(right)) => {
                        cell(left, right, options, metadata)
                    }
                    (SdtContent::ContentControl(left), SdtContent::ContentControl(right)) => {
                        control(left, right, options, metadata)
                    }
                    _ => left == right,
                })
    }
    match (left, right) {
        (BodyContent::Paragraph(left), BodyContent::Paragraph(right)) => {
            coalesced_paragraphs_equal(left, right, options, metadata)
        }
        (BodyContent::Table(left), BodyContent::Table(right)) => {
            table(left, right, options, metadata)
        }
        (BodyContent::ContentControl(left), BodyContent::ContentControl(right)) => {
            control(left, right, options, metadata)
        }
        _ => left == right,
    }
}

fn coalesced_paragraphs_equal(
    original: &CT_P,
    edited: &CT_P,
    options: &ComparisonOptions,
    metadata: &Metadata<'_>,
) -> bool {
    // A drawing signature deliberately omits its nested Word body. Atomic
    // barriers must remain equal in full, so their own publishers still run.
    if !original
        .runs
        .iter()
        .filter(|run| !coalescible_plain_run(run))
        .eq(edited.runs.iter().filter(|run| !coalescible_plain_run(run)))
    {
        return false;
    }
    let original_fields = original
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let edited_fields = edited
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if original_fields.len() != edited_fields.len()
        || original_fields
            .iter()
            .zip(&edited_fields)
            .any(|(left, right)| {
                match (
                    metadata.field_source_facts(left),
                    metadata.field_source_facts(right),
                ) {
                    (Some(left), Some(right)) => left != right,
                    _ => true,
                }
            })
    {
        return false;
    }
    if original.content_controls.len() != edited.content_controls.len()
        || original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .any(|((_, _, _, left), (_, _, _, right))| {
                match (
                    metadata.control_source(left),
                    metadata.control_source(right),
                ) {
                    (Some(left), Some(right)) => left != right,
                    _ => true,
                }
            })
    {
        return false;
    }

    let equal_fields = original
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some((field, field.is_complex()))
            } else {
                None
            }
        })
        .eq(edited
            .runs
            .iter()
            .flat_map(|run| &run.content)
            .filter_map(|content| {
                if let RunContent::Field(field) = content {
                    Some((field, field.is_complex()))
                } else {
                    None
                }
            }));
    (original.properties == edited.properties
        || original
            .properties
            .clone()
            .and_then(nonempty_paragraph_properties)
            == edited
                .properties
                .clone()
                .and_then(nonempty_paragraph_properties))
        && equal_fields
        && coalesced_paragraph_signature(original, options)
            == coalesced_paragraph_signature(edited, options)
}

fn compare_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    if !metadata.leaf_capture
        && coalesced_paragraphs_equal(original, edited, metadata.options, metadata)
        && let Some(source) = original_source
    {
        return Ok(source.to_owned());
    }
    let output = (|| -> Result<String> {
        if original.bookmark_markers == edited.bookmark_markers
            && paragraph_boundaries_differ(original, edited, metadata.options)
        {
            return Ok(format!(
                "{}{}",
                deleted_paragraph(original, metadata)?,
                inserted_paragraph(edited, metadata)?
            ));
        }
        if metadata.options.ignore_formatting
            && metadata.options.ignore_fields
            && paragraph_signature_with_options(original, metadata.options)
                == paragraph_signature_with_options(edited, metadata.options)
            && let Some(source) = original_source
        {
            return Ok(source.to_owned());
        }
        if original == edited
            && let Some((left, right)) = metadata
                .paragraph_source(original)
                .zip(metadata.paragraph_source(edited))
            && left == right
        {
            return Ok(original_source.unwrap_or(left).to_owned());
        }
        let detached = detach_field_spans(original, original_source)?;
        let original_source = detached.as_deref().or(original_source);
        if uses_attributed_run_path(metadata.options) {
            return compare_granular_paragraph(
                original,
                edited,
                original_source,
                location,
                metadata,
                diagnostics,
            );
        }
        if !original.hyperlinks.is_empty()
            || !edited.hyperlinks.is_empty()
            || !original.comment_ranges.is_empty()
            || !edited.comment_ranges.is_empty()
            || !original.bookmark_markers.is_empty()
            || !edited.bookmark_markers.is_empty()
            || !original.content_controls.is_empty()
            || !edited.content_controls.is_empty()
            || original
                .extra_xml
                .iter()
                .any(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
            || edited
                .extra_xml
                .iter()
                .any(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
        {
            return compare_complex_paragraph(
                original,
                edited,
                original_source,
                location,
                metadata,
                diagnostics,
            );
        }

        let (mut output, paragraph_close) = paragraph_shell(original_source)?;
        let destination_parent = root_namespace_bindings(&output, "p", &[])?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        output.push_str(&paragraph_properties_xml(
            original,
            edited,
            location,
            metadata,
            diagnostics,
        )?);
        let original_signatures = original
            .runs
            .iter()
            .map(|run| run_signature_with_options(run, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited
            .runs
            .iter()
            .map(|run| run_signature_with_options(run, metadata.options))
            .collect::<Vec<_>>();
        let aligned = align(&original_signatures, &edited_signatures);
        if !metadata.options.ignore_fields {
            validate_field_alignment(
                &aligned,
                &original.runs,
                &edited.runs,
                &original_signatures,
                &edited_signatures,
                location,
            )?;
        }
        let original_run_spans = original_source
            .map(|source| modeled_paragraph_run_spans(original, source))
            .transpose()?
            .unwrap_or_default();
        if original_source.is_some() && original_run_spans.len() != original.runs.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate paragraph run owners at {location}"
            )));
        }
        for (left, right) in aligned {
            match (left, right) {
                (Some(i), Some(j)) => {
                    if original_signatures[i] == edited_signatures[j] {
                        if !run_is_field(&original.runs[i])
                            && original.runs[i] == edited.runs[j]
                            && let Some(source) = original_source
                        {
                            output.push_str(&source[original_run_spans[i].clone()]);
                        } else {
                            output.push_str(&compared_run_xml(
                                &original.runs[i],
                                &edited.runs[j],
                                None,
                                None,
                                &format!("{location}/run[{j}]"),
                                &destination_parent,
                                metadata,
                                diagnostics,
                            )?);
                        }
                    } else {
                        output.push_str(&paired_run_replacement(
                            [&original.runs[i], &edited.runs[j]],
                            [None, None],
                            [None, None],
                            &format!("{location}/run[{j}]"),
                            &destination_parent,
                            metadata,
                            diagnostics,
                        )?);
                    }
                }
                (Some(i), None) => {
                    if run_is_ignored(&original.runs[i], metadata.options) {
                        if let Some(source) = original_source {
                            output.push_str(&source[original_run_spans[i].clone()]);
                        } else {
                            output.push_str(&paragraph_owned_run_xml(&original.runs[i])?);
                        }
                        continue;
                    }
                    let run = publish_run_source(
                        &original.runs[i],
                        &original.runs[i],
                        deleted_run_xml(&original.runs[i])?,
                        None,
                        None,
                        None,
                        &revision_parent,
                        metadata,
                    )?;
                    output.push_str(&metadata.ids.revision(
                        "del",
                        metadata.author,
                        metadata.timestamp,
                        &run,
                    )?);
                }
                (None, Some(j)) => {
                    if run_is_ignored(&edited.runs[j], metadata.options) {
                        continue;
                    }
                    let run = publish_run_source(
                        &edited.runs[j],
                        &edited.runs[j],
                        paragraph_owned_run_xml(&edited.runs[j])?,
                        None,
                        None,
                        None,
                        &revision_parent,
                        metadata,
                    )?;
                    output.push_str(&metadata.ids.revision(
                        "ins",
                        metadata.author,
                        metadata.timestamp,
                        &run,
                    )?);
                }
                (None, None) => unreachable!(),
            }
        }
        output.push_str(&paragraph_close);
        Ok(output)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_P as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn element_opening(xml: &str) -> Result<(usize, bool, String)> {
    let mut reader = Reader::from_str(xml);
    let event = reader
        .read_event()
        .map_err(|error| Error::Other(format!("comparison owner start failed: {error}")))?;
    let (element, empty) = match event {
        Event::Start(element) => (element, false),
        Event::Empty(element) => (element, true),
        _ => return Err(Error::Other("comparison owner has no start element".into())),
    };
    let name = std::str::from_utf8(element.name().as_ref())
        .map_err(utf8_error)?
        .to_owned();
    Ok((reader.buffer_position() as usize, empty, name))
}

fn insert_first_child(xml: &str, child: &str) -> Result<String> {
    let (end, empty, name) = element_opening(xml)?;
    if empty {
        return Ok(format!(
            "{}>{child}</{name}>{}",
            &xml[..end - 2],
            &xml[end..]
        ));
    }
    Ok(format!("{}{child}{}", &xml[..end], &xml[end..]))
}

fn paragraph_shell(source: Option<&str>) -> Result<(String, String)> {
    let Some(source) = source else {
        return Ok(("<w:p>".to_owned(), "</w:p>".to_owned()));
    };
    let (end, empty, name) = element_opening(source)?;
    let mut opening = source[..end].to_owned();
    if empty {
        opening.truncate(opening.len() - 2);
        opening.push('>');
    }
    Ok((opening, format!("</{name}>")))
}

fn uses_attributed_run_path(options: &ComparisonOptions) -> bool {
    options.granularity != ComparisonGranularity::Run
        || options.ignore_formatting
        || options.ignore_whitespace
        || options.ignore_fields
        || options.ignore_comments
        || story_ignored(options, ComparisonStoryKind::TextBox)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GranularAction {
    Preserve,
    Equal,
    Replace,
    Delete,
    Insert,
    Drop,
}

fn compare_granular_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let actual_source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(original), Ok)?;
    let actual_spans = modeled_paragraph_run_spans(original, &actual_source)?;
    // One read of the paragraph answers the scope of every run, on first use.
    // Reading it again for each written piece is quadratic in its runs.
    let run_parents = ParentScopes::new(
        &actual_source,
        actual_spans
            .iter()
            .map(|span| span.start)
            .chain(element_opening(&actual_source).ok().map(|(end, ..)| end))
            .collect(),
    );
    let parent_for = |owner: usize| {
        let at = actual_spans
            .get(owner)
            .map_or(element_opening(&actual_source)?.0, |span| span.start);
        run_parents.at(at)
    };
    let boundary_error = || {
        Error::Other(format!(
            "comparison cannot revise paragraph boundary structures at {location}"
        ))
    };
    if paragraph_boundaries_differ(original, edited, metadata.options) {
        return Err(boundary_error());
    }
    let original_boundaries = shell_run_boundaries(original, metadata.options);
    let edited_boundaries = shell_run_boundaries(edited, metadata.options);

    let original_run_signatures = original
        .runs
        .iter()
        .map(attributed_run_signature)
        .collect::<Vec<_>>();
    let edited_run_signatures = edited
        .runs
        .iter()
        .map(attributed_run_signature)
        .collect::<Vec<_>>();
    // Runs that all match stay whole even when a control differs, so a
    // control's metadata or content never moves run-indexed markers.
    if original_run_signatures == edited_run_signatures
        && original_boundaries == edited_boundaries
        && !SourceMetadata::paragraph_contains_fields(original, metadata.options)?
    {
        let properties =
            paragraph_properties_xml(original, edited, location, metadata, diagnostics)?;
        let spans = original_source
            .map(|source| modeled_paragraph_run_spans(original, source))
            .transpose()?
            .unwrap_or_default();
        if original_source.is_some() && spans.len() != original.runs.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate granular run owners at {location}"
            )));
        }
        let replacements = original
            .runs
            .iter()
            .zip(&edited.runs)
            .enumerate()
            .map(|(index, (left, right))| {
                if left == right
                    && let Some(source) = original_source
                {
                    Ok(source[spans[index].clone()].to_owned())
                } else {
                    compared_run_xml(
                        left,
                        right,
                        None,
                        None,
                        &format!("{location}/run[{index}]"),
                        &parent_for(index)?,
                        metadata,
                        diagnostics,
                    )
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let output = replace_paragraph_properties_and_runs(
            original,
            original_source,
            &properties,
            &replacements,
        )?;
        if original.content_controls == edited.content_controls {
            return Ok(output);
        }
        return compare_granular_controls(
            original,
            edited,
            &output,
            location,
            metadata,
            diagnostics,
            leaf_mark,
        );
    }

    let projection_pairs = metadata.leaf_capture.then(|| {
        projection_leaf_pairs(
            [&original.runs[..], &edited.runs[..]],
            &original_boundaries
                .iter()
                .copied()
                .zip(edited_boundaries.iter().copied())
                .collect::<Vec<_>>(),
        )
    });
    let projection_sources = [&original.runs[..], &edited.runs[..]]
        .into_iter()
        .enumerate()
        .map(|(side, runs)| {
            runs.iter()
                .enumerate()
                .map(|(owner, run)| {
                    let _ = (side, owner);
                    metadata.run_source(run).cloned()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let projection_sources = [projection_sources[0].clone(), projection_sources[1].clone()];
    let mut projection_ledgers = if let Some(pairs) = &projection_pairs {
        projection_action_ledgers(
            pairs,
            [&original.runs[..], &edited.runs[..]],
            &projection_sources,
        )?
    } else {
        Vec::new()
    };
    let mut coalesced_words =
        !metadata.leaf_capture && metadata.options.granularity == ComparisonGranularity::Word;
    let unit_options = ComparisonOptions {
        granularity: if coalesced_words {
            ComparisonGranularity::Character
        } else {
            metadata.options.granularity
        },
        ..metadata.options.clone()
    };
    let mut original_units = attributed_run_units(&original.runs, &unit_options);
    let mut edited_units = attributed_run_units(&edited.runs, &unit_options);
    validate_raw_attribution(&original.runs, &original_units)?;
    validate_raw_attribution(&edited.runs, &edited_units)?;
    let mut original_signatures = original_units
        .iter()
        .map(|unit| {
            if coalesced_words
                && metadata.options.ignore_formatting
                && unit_plain_text(unit).is_some()
            {
                run_signature_with_options(&unit.run, metadata.options)
            } else {
                attributed_unit_signature(unit)
            }
        })
        .collect::<Vec<_>>();
    let mut edited_signatures = edited_units
        .iter()
        .map(|unit| {
            if coalesced_words
                && metadata.options.ignore_formatting
                && unit_plain_text(unit).is_some()
            {
                run_signature_with_options(&unit.run, metadata.options)
            } else {
                attributed_unit_signature(unit)
            }
        })
        .collect::<Vec<_>>();

    let properties = paragraph_properties_xml(original, edited, location, metadata, diagnostics)?;
    let Some(mut cuts) = shell_unit_cuts(
        &original_units,
        &edited_units,
        original_boundaries
            .iter()
            .copied()
            .zip(edited_boundaries.iter().copied()),
    ) else {
        return Err(boundary_error());
    };
    // Actual original formatting owner for a paired logical word. This is
    // independent of every edited scalar's physical content/source owner.
    let mut word_format_owners = vec![None; edited_units.len()];
    let (aligned, segments) = if let Some(pairs) = &projection_pairs {
        projection_unit_alignment(
            pairs,
            [&original_units, &edited_units],
            [&original_signatures, &edited_signatures],
        )?
    } else {
        if !metadata.leaf_capture && metadata.options.granularity == ComparisonGranularity::Word {
            if let Some(alignment) = align_words_between_shells(
                &original_units,
                &edited_units,
                &original_signatures,
                &edited_signatures,
                &cuts,
                metadata.options.ignore_formatting,
                &mut word_format_owners,
            ) {
                alignment
            } else {
                coalesced_words = false;
                word_format_owners.clear();
                // A barrier paired with text uses the existing whole Word unit,
                // never a newly invented scalar-to-object correspondence.
                original_units = attributed_run_units(&original.runs, metadata.options);
                edited_units = attributed_run_units(&edited.runs, metadata.options);
                validate_raw_attribution(&original.runs, &original_units)?;
                validate_raw_attribution(&edited.runs, &edited_units)?;
                original_signatures = original_units
                    .iter()
                    .map(attributed_unit_signature)
                    .collect();
                edited_signatures = edited_units.iter().map(attributed_unit_signature).collect();
                cuts = shell_unit_cuts(
                    &original_units,
                    &edited_units,
                    original_boundaries
                        .iter()
                        .copied()
                        .zip(edited_boundaries.iter().copied()),
                )
                .ok_or_else(boundary_error)?;
                align_between_shells(&original_signatures, &edited_signatures, &cuts)
            }
        } else {
            align_between_shells(&original_signatures, &edited_signatures, &cuts)
        }
    };
    // The original run each shell segment starts at, before which the
    // redline copies the original bytes, shell tags included.
    let segment_runs = if let Some(pairs) = &projection_pairs {
        pairs
            .iter()
            .enumerate()
            .map(|(at, pair)| {
                pair[0]
                    .map(|(owner, _)| owner)
                    .or_else(|| {
                        pairs[at..]
                            .iter()
                            .find_map(|next| next[0].map(|(owner, _)| owner))
                    })
                    .unwrap_or(original.runs.len())
            })
            .collect::<Vec<_>>()
    } else {
        std::iter::once(0)
            .chain(cuts.iter().map(|&(cut, _)| {
                original_units
                    .get(cut)
                    .map_or(original.runs.len(), |unit| unit.owner)
            }))
            .collect::<Vec<_>>()
    };
    if !metadata.options.ignore_fields {
        validate_field_alignment(
            &aligned,
            &original_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &edited_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &original_signatures,
            &edited_signatures,
            location,
        )?;
    }
    let (aligned, segments) = grouped_text_alignment(
        &aligned,
        &segments,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
        metadata.options,
    );
    let mut grouped = coalesced_granular_alignment(
        &aligned,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
    );
    if coalesced_words && metadata.options.ignore_formatting {
        grouped = grouped
            .into_iter()
            .flat_map(|(action, members)| {
                let mut pieces = Vec::new();
                let mut start = members.start;
                let owner = |at: usize| aligned[at].1.and_then(|index| word_format_owners[index]);
                for at in members.start + 1..members.end {
                    if owner(at) != owner(start) {
                        pieces.push((action, start..at));
                        start = at;
                    }
                }
                pieces.push((action, start..members.end));
                pieces
            })
            .collect();
    }
    if let Some(pairs) = &projection_pairs {
        retain_projection_separators(
            &mut grouped,
            &aligned,
            &segments,
            pairs,
            [&original_units, &edited_units],
            &projection_sources,
            metadata.options,
        )?;
    }
    validate_fragment_partition(&aligned, &grouped)?;
    let mut grouped_alignment = Vec::with_capacity(grouped.len());
    let mut grouped_segment_runs = Vec::with_capacity(grouped.len());
    let mut replacements = Vec::with_capacity(grouped.len());
    let mut fragment_publications = Vec::with_capacity(grouped.len());
    let mut word_revisions = Vec::<
        Option<(
            GranularAction,
            usize,
            Option<oxoffice_docx_oxml::properties::CT_RPr>,
            i32,
        )>,
    >::with_capacity(grouped.len());
    for (action, members) in grouped {
        if metadata.leaf_capture {
            record_projection_actions(
                &mut projection_ledgers,
                action,
                members.clone(),
                &aligned,
                [&original_units, &edited_units],
                &projection_sources,
            )?;
        }
        let members_start = members.start;
        let first = aligned[members.start];
        grouped_alignment.push(first);
        grouped_segment_runs.push(segment_runs[segments[members.start]]);
        let left_indices = members
            .clone()
            .filter_map(|index| aligned[index].0)
            .collect::<Vec<_>>();
        let right_indices = members
            .filter_map(|index| aligned[index].1)
            .collect::<Vec<_>>();
        let word_key = if coalesced_words
            && matches!(action, GranularAction::Delete | GranularAction::Insert)
        {
            let (indices, units) = if action == GranularAction::Delete {
                (&left_indices, &original_units)
            } else {
                (&right_indices, &edited_units)
            };
            if indices.iter().all(|index| {
                unit_plain_text(&units[*index]).is_some_and(|text| {
                    !text.is_empty()
                        && text
                            .chars()
                            .all(|character| character.is_alphanumeric() || character == '_')
                })
            }) {
                Some((
                    action,
                    segments[members_start],
                    if action == GranularAction::Insert && metadata.options.ignore_formatting {
                        word_format_owners[indices[0]]
                            .map(|owner| original.runs[owner].properties.clone())
                            .unwrap_or_else(|| units[indices[0]].run.properties.clone())
                    } else {
                        units[indices[0]].run.properties.clone()
                    },
                ))
            } else {
                None
            }
        } else {
            None
        };
        let mut word_revision_id = None;
        let mut left_raw = Vec::new();
        let mut left_typed = Vec::new();
        let mut right_raw = Vec::new();
        let mut right_typed = Vec::new();
        let left = merge_unit_runs(
            &original.runs,
            &original_units,
            &left_indices,
            &mut left_raw,
            &mut left_typed,
        )?;
        let right = merge_unit_runs(
            &edited.runs,
            &edited_units,
            &right_indices,
            &mut right_raw,
            &mut right_typed,
        )?;
        let left_source = left_indices
            .first()
            .and_then(|index| metadata.run_source(&original.runs[original_units[*index].owner]))
            .cloned();
        let right_source = right_indices
            .first()
            .and_then(|index| metadata.run_source(&edited.runs[edited_units[*index].owner]))
            .cloned();
        let destination_owner = left_indices
            .first()
            .map(|index| original_units[*index].owner)
            .unwrap_or(segment_runs[segments[members_start]]);
        let destination_parent = parent_for(destination_owner)?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        let policy_shell = projection_pairs
            .as_ref()
            .and_then(|pairs| {
                right_indices.first().and_then(|index| {
                    let unit = &edited_units[*index];
                    if !matches!(
                        unit.run.content.first(),
                        Some(RunContent::Text(_) | RunContent::DeletedText(_))
                    ) {
                        return None;
                    }
                    let ordinal = unit.typed_sources.first().and_then(|slot| *slot)?.ordinal;
                    pairs.iter().find_map(|pair| {
                        (pair[1] == Some((unit.owner, ordinal)))
                            .then_some(pair[0])
                            .flatten()
                    })
                })
            })
            .map(|(owner, ordinal)| {
                let run = &original.runs[owner];
                let source = metadata.run_source(run).cloned().ok_or_else(|| {
                    Error::Other("paired projection shell has no actual source".into())
                })?;
                if source
                    .typed_slots
                    .get(ordinal)
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    return Err(Error::Other(
                        "paired projection shell leaf has no physical slot".into(),
                    ));
                }
                Ok((run, source))
            })
            .transpose()?;
        let policy_shell =
            if policy_shell.is_none() && coalesced_words && metadata.options.ignore_formatting {
                right_indices
                    .first()
                    .and_then(|index| word_format_owners[*index])
                    .map(|owner| -> Result<_> {
                        let run = &original.runs[owner];
                        let source = metadata.run_source(run).cloned().ok_or_else(|| {
                            Error::Other("logical word formatting has no actual source".into())
                        })?;
                        Ok((run, source))
                    })
                    .transpose()?
            } else {
                policy_shell
            };
        let mut field_bindings = Vec::new();
        for (indices, units, originals, cloned, selected) in [
            (
                &left_indices,
                &original_units,
                &original.runs,
                left.as_ref(),
                &left_typed,
            ),
            (
                &right_indices,
                &edited_units,
                &edited.runs,
                right.as_ref(),
                &right_typed,
            ),
        ] {
            if let Some(cloned) = cloned
                && run_is_field(cloned)
            {
                let owner = units[indices[0]].owner;
                match metadata.bind_selected_run_fields(&originals[owner], cloned, selected) {
                    Ok(bindings) => field_bindings.extend(bindings),
                    Err(error) => {
                        metadata.unbind_cloned_control_runs(field_bindings)?;
                        return Err(error);
                    }
                }
            }
        }
        let replacement = (|| -> Result<String> {
            Ok(match action {
                GranularAction::Preserve => publish_run_source(
                    left.as_ref().unwrap(),
                    left.as_ref().unwrap(),
                    paragraph_owned_run_xml(left.as_ref().unwrap())?,
                    left_source.as_ref(),
                    None,
                    Some((&left_raw, &left_typed)),
                    &destination_parent,
                    metadata,
                )?,
                GranularAction::Equal => compared_run_xml(
                    left.as_ref().unwrap(),
                    right.as_ref().unwrap(),
                    left_source.as_ref(),
                    Some((&left_raw, &left_typed)),
                    &format!("{location}/run-unit[{}]", right_indices[0]),
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Replace => paired_run_replacement(
                    [left.as_ref().unwrap(), right.as_ref().unwrap()],
                    [left_source.as_ref(), right_source.as_ref()],
                    [
                        Some((&left_raw, &left_typed)),
                        Some((&right_raw, &right_typed)),
                    ],
                    location,
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Delete => {
                    let payload = publish_run_source(
                        left.as_ref().unwrap(),
                        left.as_ref().unwrap(),
                        deleted_run_xml(left.as_ref().unwrap())?,
                        left_source.as_ref(),
                        None,
                        Some((&left_raw, &left_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    if let Some((kind, segment, properties)) = &word_key {
                        let id = match word_revisions.last().and_then(Option::as_ref) {
                            Some((previous_kind, previous_segment, previous_properties, id))
                                if previous_kind == kind
                                    && previous_segment == segment
                                    && previous_properties == properties =>
                            {
                                *id
                            }
                            _ => metadata.ids.allocate()?,
                        };
                        word_revision_id = Some(id);
                        payload
                    } else {
                        metadata.ids.revision(
                            "del",
                            metadata.author,
                            metadata.timestamp,
                            &payload,
                        )?
                    }
                }
                GranularAction::Insert => {
                    let mut content = right.as_ref().unwrap().clone();
                    let (shell, shell_source) = if let Some((shell, source)) = &policy_shell {
                        let before = usize::from(content.properties.is_some());
                        content.properties = shell.properties.clone();
                        let after = usize::from(content.properties.is_some());
                        for encoded in &mut content.extra_xml_positions {
                            if !CT_R::raw_child_is_root_attributes(*encoded) {
                                let at = CT_R::raw_child_position(*encoded).saturating_sub(before)
                                    + after;
                                CT_R::set_raw_child_position(encoded, at);
                            }
                        }
                        (*shell, Some(source))
                    } else {
                        (right.as_ref().unwrap(), right_source.as_ref())
                    };
                    let payload = publish_run_source(
                        shell,
                        &content,
                        paragraph_owned_run_xml(&content)?,
                        shell_source,
                        right_source.as_ref(),
                        Some((&right_raw, &right_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    if let Some((kind, segment, properties)) = &word_key {
                        let id = match word_revisions.last().and_then(Option::as_ref) {
                            Some((previous_kind, previous_segment, previous_properties, id))
                                if previous_kind == kind
                                    && previous_segment == segment
                                    && previous_properties == properties =>
                            {
                                *id
                            }
                            _ => metadata.ids.allocate()?,
                        };
                        word_revision_id = Some(id);
                        payload
                    } else {
                        metadata.ids.revision(
                            "ins",
                            metadata.author,
                            metadata.timestamp,
                            &payload,
                        )?
                    }
                }
                GranularAction::Drop => String::new(),
            })
        })();
        metadata.unbind_cloned_control_runs(field_bindings)?;
        replacements.push(replacement?);
        word_revisions.push(
            word_key
                .zip(word_revision_id)
                .map(|((kind, segment, properties), id)| (kind, segment, properties, id)),
        );
        let mut leaves = Vec::new();
        if metadata.leaf_capture {
            let selected = match action {
                GranularAction::Replace | GranularAction::Insert => right
                    .as_ref()
                    .zip(right_source.as_ref())
                    .map(|(run, source)| {
                        (
                            run,
                            source,
                            &right_raw[..],
                            &right_typed[..],
                            matches!(action, GranularAction::Replace | GranularAction::Insert),
                        )
                    }),
                GranularAction::Equal | GranularAction::Preserve => left
                    .as_ref()
                    .zip(left_source.as_ref())
                    .map(|(run, source)| (run, source, &left_raw[..], &left_typed[..], false)),
                _ => None,
            };
            if let Some((run, source, raw, typed, inserted)) = selected {
                leaves = emitted_fragment_leaves(
                    replacements.last().unwrap(),
                    run,
                    source,
                    raw,
                    typed,
                    inserted,
                    metadata.options.ignore_whitespace,
                    &destination_parent,
                )?;
            }
        }
        fragment_publications.push(leaves);
    }
    // Payloads retain their separate physical owners and source publication.
    // Only the enclosing generated revision is shared by adjacent pieces of
    // one ordinary word, without crossing a shell, formatting or token boundary.
    let mut start = 0;
    while start < replacements.len() {
        let Some((action, _, _, id)) = &word_revisions[start] else {
            start += 1;
            continue;
        };
        let id = *id;
        let kind = if *action == GranularAction::Delete {
            "del"
        } else {
            "ins"
        };
        let mut end = start + 1;
        while end < word_revisions.len()
            && word_revisions[end]
                .as_ref()
                .is_some_and(|(_, _, _, other)| *other == id)
        {
            end += 1;
        }
        let mut payload = std::mem::take(&mut replacements[start]);
        for replacement in &mut replacements[start + 1..end] {
            payload.push_str(&std::mem::take(replacement));
        }
        replacements[start] = metadata.ids.generated(IdAllocator::revision_with_id(
            kind,
            metadata.author,
            metadata.timestamp,
            &payload,
            id,
        ))?;
        start = end;
    }
    if metadata.leaf_capture {
        attach_projection_actions(&mut fragment_publications, &projection_ledgers)?;
    }
    interleave_granular_paragraph(
        original,
        edited,
        original_source,
        &original_units,
        &edited_units,
        &grouped_alignment,
        &grouped_segment_runs,
        &replacements,
        &fragment_publications,
        &properties,
        location,
        metadata,
        diagnostics,
    )
}

/// The hyperlink owners without their place in the paragraph, which
/// [`shell_unit_cuts`] follows through the unit alignment.
fn hyperlink_shells(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<String> {
    paragraph
        .hyperlinks
        .iter()
        .map(|link| {
            format!(
                "{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
                link.rel_id,
                link.anchor,
                link.tooltip,
                link.doc_location,
                link.extra_attributes,
                hyperlink_raw_boundaries(paragraph, link, options),
                link.preserved_raw_before,
            )
        })
        .collect()
}

/// The run boundaries of the shells that the attributed path keeps from the
/// original: where each hyperlink starts and ends and where each inline
/// control or opaque child sits.
fn shell_run_boundaries(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<usize> {
    paragraph
        .hyperlinks
        .iter()
        .flat_map(|link| [link.run_start, link.run_end])
        .chain(paragraph.content_controls.iter().map(|(at, ..)| *at))
        .chain(paragraph.extra_xml.iter().filter_map(|(at, raw)| {
            (options.granularity != ComparisonGranularity::Run
                && !CT_P::raw_is_root_attributes(*at, raw))
            .then_some(*at)
        }))
        .chain(
            paragraph
                .bookmark_markers
                .iter()
                .map(|marker| marker.run_index()),
        )
        .chain(
            paragraph
                .comment_ranges
                .iter()
                .filter(|_| !options.ignore_comments)
                .map(|marker| match marker {
                    CommentRangeMarker::Start { run_index, .. }
                    | CommentRangeMarker::End { run_index, .. } => *run_index,
                }),
        )
        .collect()
}

/// The `(original, edited)` unit indices where the shell boundaries fall, in
/// document order, for `(original, edited)` run boundaries.
///
/// The units between two consecutive boundaries form a segment, and the
/// redline copies the original bytes before the first run of a segment,
/// shell tags included, before anything of that segment, so each boundary
/// moves with the words inserted or deleted around it. `None` when the
/// shells are not in the same order on both sides, or when the edited side
/// writes a unit between two boundaries that fall between the same two
/// original runs, because the original bytes there are not split. Ignorable
/// and empty units are left out, as in the accept and reject postconditions.
fn shell_unit_cuts(
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    boundaries: impl IntoIterator<Item = (usize, usize)>,
) -> Option<Vec<(usize, usize)>> {
    let mut boundaries = boundaries.into_iter().collect::<Vec<_>>();
    boundaries.sort_unstable();
    if boundaries.windows(2).any(|pair| pair[0].1 > pair[1].1) {
        return None;
    }
    let cuts = boundaries
        .into_iter()
        .map(|(original, edited)| {
            (
                original_units.partition_point(|unit| unit.owner < original),
                edited_units.partition_point(|unit| unit.owner < edited),
            )
        })
        .collect::<Vec<_>>();
    let mut start = (0, 0);
    cuts.iter()
        .all(|&end| {
            let writable = start.0 < end.0
                || edited_units[start.1..end.1]
                    .iter()
                    .all(|unit| unit_is_ignorable(unit) || unit_is_empty(unit));
            start = end;
            writable
        })
        .then_some(cuts)
}

/// The unit alignment made segment by segment between the shell boundaries
/// at `cuts`, so no unit is matched across a hyperlink or an inline control,
/// and the segment of each pair.
#[allow(clippy::type_complexity)]
fn align_between_shells(
    original: &[String],
    edited: &[String],
    cuts: &[(usize, usize)],
) -> (Vec<(Option<usize>, Option<usize>)>, Vec<usize>) {
    let mut aligned = Vec::with_capacity(original.len().max(edited.len()));
    let mut segments = Vec::with_capacity(aligned.capacity());
    let mut start = (0, 0);
    for (segment, &end) in cuts
        .iter()
        .chain(std::iter::once(&(original.len(), edited.len())))
        .enumerate()
    {
        for (left, right) in align(&original[start.0..end.0], &edited[start.1..end.1]) {
            aligned.push((
                left.map(|index| index + start.0),
                right.map(|index| index + start.1),
            ));
            segments.push(segment);
        }
        start = end;
    }
    (aligned, segments)
}

/// Align logical words across plain physical runs, retaining each scalar's real
/// owner for source publication. Shell cuts and nontext units stay barriers.
fn align_words_between_shells(
    original: &[AttributedRunUnit],
    edited: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
    cuts: &[(usize, usize)],
    ignore_formatting: bool,
    format_owners: &mut [Option<usize>],
) -> Option<(Alignment, Vec<usize>)> {
    fn groups(
        units: &[AttributedRunUnit],
        signatures: &[String],
        span: Range<usize>,
    ) -> Vec<(String, Range<usize>)> {
        let class = |unit: &AttributedRunUnit| {
            let text = unit_plain_text(unit)?;
            let character = text.chars().next()?;
            Some(if character.is_alphanumeric() || character == '_' {
                0
            } else if character.is_whitespace() {
                1
            } else {
                2
            })
        };
        let mut result = Vec::new();
        let mut at = span.start;
        while at < span.end {
            let mut end = at + 1;
            if let Some(kind) = class(&units[at]) {
                while end < span.end
                    && class(&units[end]) == Some(kind)
                    && run_formatting(&units[end].run) == run_formatting(&units[at].run)
                {
                    end += 1;
                }
            }
            let signature = format!("{:?}", &signatures[at..end]);
            result.push((signature, at..end));
            at = end;
        }
        result
    }
    let mut result = Vec::new();
    let mut segments = Vec::new();
    let mut start = (0, 0);
    for (segment, &end) in cuts
        .iter()
        .chain(std::iter::once(&(original.len(), edited.len())))
        .enumerate()
    {
        let left = groups(original, original_signatures, start.0..end.0);
        let right = groups(edited, edited_signatures, start.1..end.1);
        let left_signatures = left
            .iter()
            .map(|(signature, _)| signature.clone())
            .collect::<Vec<_>>();
        let right_signatures = right
            .iter()
            .map(|(signature, _)| signature.clone())
            .collect::<Vec<_>>();
        for (a, b) in align(&left_signatures, &right_signatures) {
            if !ignore_formatting
                && let (Some(a), Some(b)) = (a, b)
                && run_formatting(&original[left[a].1.start].run)
                    != run_formatting(&edited[right[b].1.start].run)
            {
                // Formatting transitions retain the existing Word replacement
                // owner and its ignore-formatting left bias.
                return None;
            }

            if ignore_formatting
                && let (Some(a), Some(b)) = (a, b)
                && unit_plain_text(&original[left[a].1.start]).is_some()
                && unit_plain_text(&edited[right[b].1.start]).is_some()
            {
                for index in right[b].1.clone() {
                    format_owners[index] = Some(original[left[a].1.start].owner);
                }
            }
            if let (Some(a), Some(b)) = (a, b)
                && left[a].0 == right[b].0
            {
                result.extend(
                    left[a]
                        .1
                        .clone()
                        .zip(right[b].1.clone())
                        .map(|(a, b)| (Some(a), Some(b))),
                );
            } else if let (Some(a), Some(b)) = (a, b)
                && (unit_plain_text(&original[left[a].1.start]).is_none()
                    || unit_plain_text(&edited[right[b].1.start]).is_none())
            {
                if unit_plain_text(&original[left[a].1.start]).is_some()
                    || unit_plain_text(&edited[right[b].1.start]).is_some()
                {
                    return None;
                }
                // Both groups are one actual atomic unit. Retain Replace so
                // the existing Field/textbox/token publisher owns its change.
                result.push((Some(left[a].1.start), Some(right[b].1.start)));
            } else {
                if let Some(a) = a {
                    result.extend(left[a].1.clone().map(|a| (Some(a), None)));
                }
                if let Some(b) = b {
                    result.extend(right[b].1.clone().map(|b| (None, Some(b))));
                }
            }
            segments.resize(result.len(), segment);
        }
        start = end;
    }
    Some((result, segments))
}

fn hyperlink_raw_boundaries(
    paragraph: &CT_P,
    link: &oxoffice_docx_oxml::text::HyperlinkSpan,
    options: &ComparisonOptions,
) -> Vec<(usize, usize, Vec<u8>)> {
    let logical_start = policy_run_boundary(paragraph, link.run_start, options);
    link.extra_xml
        .iter()
        .map(|(boundary, revisions_before, raw)| {
            let absolute = link.run_start.saturating_add(*boundary);
            (
                policy_run_boundary(paragraph, absolute, options).saturating_sub(logical_start),
                *revisions_before,
                raw.clone(),
            )
        })
        .collect()
}

fn policy_run_boundary(
    paragraph: &CT_P,
    run_boundary: usize,
    options: &ComparisonOptions,
) -> usize {
    attributed_run_units(
        &paragraph.runs[..run_boundary.min(paragraph.runs.len())],
        options,
    )
    .iter()
    .filter(|unit| !unit_is_ignorable(unit) && !unit_is_empty(unit))
    .count()
}

fn unit_is_ignorable(unit: &AttributedRunUnit) -> bool {
    unit.ignored && semantic_run_raw(&unit.run).is_empty() && unit.run.alt_drawings.is_empty()
}

fn unit_is_empty(unit: &AttributedRunUnit) -> bool {
    unit.run.content.is_empty()
        && semantic_run_raw(&unit.run).is_empty()
        && unit.run.alt_drawings.is_empty()
        && run_formatting(&unit.run).is_none()
}

/// The properties of a run, none when empty: LibreOffice writes
/// `<w:rPr></w:rPr>`, which the model writes back as nothing, and which
/// carries no format.
fn run_formatting(run: &CT_R) -> Option<&oxoffice_docx_oxml::properties::CT_RPr> {
    run.properties
        .as_ref()
        .filter(|properties| **properties != oxoffice_docx_oxml::properties::CT_RPr::default())
}

fn granular_action(
    pair: (Option<usize>, Option<usize>),
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
) -> GranularAction {
    match pair {
        (Some(i), Some(j))
            if original_units[i].ignored
                && edited_units[j].ignored
                && original_signatures[i] == edited_signatures[j] =>
        {
            GranularAction::Preserve
        }
        (Some(i), Some(j)) if original_signatures[i] == edited_signatures[j] => {
            GranularAction::Equal
        }
        (Some(_), Some(_)) => GranularAction::Replace,
        (Some(i), None) if unit_is_ignorable(&original_units[i]) => GranularAction::Preserve,
        (None, Some(j)) if unit_is_ignorable(&edited_units[j]) => GranularAction::Drop,
        (Some(_), None) => GranularAction::Delete,
        (None, Some(_)) => GranularAction::Insert,
        (None, None) => unreachable!(),
    }
}

fn validate_fragment_partition(
    aligned: &[(Option<usize>, Option<usize>)],
    groups: &[(GranularAction, Range<usize>)],
) -> Result<()> {
    let mut end = 0;
    for (_, range) in groups {
        if range.start != end || range.start >= range.end || range.end > aligned.len() {
            return Err(Error::Other(
                "fragment groups do not partition their input roles".into(),
            ));
        }
        end = range.end;
    }
    if end != aligned.len() {
        return Err(Error::Other(
            "fragment groups omit aligned input roles".into(),
        ));
    }
    for side in [0, 1] {
        let mut next = 0;
        for &(left, right) in aligned {
            if let Some(index) = if side == 0 { left } else { right } {
                if index != next {
                    return Err(Error::Other(
                        "fragment input role repeats, reorders or loses a unit".into(),
                    ));
                }
                next += 1;
            }
        }
    }
    Ok(())
}

/// Only isolated paired-body projection selects these ordered source-slot pairs.
/// Scalar alignment is confined to each pair, including ignored whitespace.
fn projection_leaf_pairs(
    runs: [&[CT_R]; 2],
    boundaries: &[(usize, usize)],
) -> Vec<[Option<(usize, usize)>; 2]> {
    let signatures = runs.map(|runs| {
        runs.iter()
            .map(attributed_run_signature)
            .collect::<Vec<_>>()
    });
    let mut output = Vec::new();
    for (left, right) in align_between_shells(&signatures[0], &signatures[1], boundaries).0 {
        let slots = [left, right]
            .into_iter()
            .enumerate()
            .map(|(side, owner)| {
                owner
                    .map(|owner| {
                        let run = &runs[side][owner];
                        if run.content.is_empty() {
                            vec!["raw-run".to_owned()]
                        } else {
                            run.content.iter().map(run_content_signature).collect()
                        }
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<Vec<String>>>();
        let raw_cuts = [left, right]
            .into_iter()
            .enumerate()
            .map(|(side, owner)| {
                owner
                    .map(|owner| {
                        let run = &runs[side][owner];
                        run.extra_xml_positions
                            .iter()
                            .filter(|encoded| !CT_R::raw_child_is_root_attributes(**encoded))
                            .map(|encoded| {
                                CT_R::raw_child_position(*encoded)
                                    .saturating_sub(usize::from(run.properties.is_some()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let cuts = if raw_cuts[0].len() == raw_cuts[1].len() {
            raw_cuts[0]
                .iter()
                .copied()
                .zip(raw_cuts[1].iter().copied())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for (a, b) in align_between_shells(&slots[0], &slots[1], &cuts).0 {
            output.push([
                left.zip(a).map(|(owner, ordinal)| {
                    (
                        owner,
                        if runs[0][owner].content.is_empty() {
                            usize::MAX
                        } else {
                            ordinal
                        },
                    )
                }),
                right.zip(b).map(|(owner, ordinal)| {
                    (
                        owner,
                        if runs[1][owner].content.is_empty() {
                            usize::MAX
                        } else {
                            ordinal
                        },
                    )
                }),
            ]);
        }
    }
    output
}

/// One logical occurrence has independent original and edited scalar domains.
/// Capture source membership before unit grouping, then record actual actions.
struct ProjectionActionLedger {
    domains: [Option<(usize, usize, usize)>; 2],
    source: String,
    context: NamespaceBindings,
    actions: Vec<([Option<Range<usize>>; 2], GranularAction)>,
}

fn projection_action_ledgers(
    pairs: &[[Option<(usize, usize)>; 2]],
    runs: [&[CT_R]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
) -> Result<Vec<ProjectionActionLedger>> {
    let mut result = Vec::new();
    for pair in pairs {
        let mut domains = [None, None];
        let mut template = None;
        for side in 0..2 {
            let Some((owner, ordinal)) = pair[side] else {
                continue;
            };
            if !matches!(
                runs[side][owner].content.get(ordinal),
                Some(RunContent::Text(_) | RunContent::DeletedText(_))
            ) {
                continue;
            }
            let source = sources[side][owner]
                .as_ref()
                .ok_or_else(|| Error::Other("projection leaf ledger has no source owner".into()))?;
            let span = source
                .typed_slots
                .get(ordinal)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    Error::Other("projection leaf ledger has no actual lexical slot".into())
                })?;
            let context = root_namespace_bindings(&source.scoped, "r", &source.parent)?;
            let raw = source.scoped[span.clone()].to_owned();
            let Some(RunContent::Text(text) | RunContent::DeletedText(text)) =
                source_content_model(&raw, &context)?
            else {
                return Err(Error::Other(
                    "projection leaf ledger has incompatible source grammar".into(),
                ));
            };
            domains[side] = Some((source.owner, ordinal, text.text.chars().count()));
            if template.is_none() {
                template = Some((raw, context));
            }
        }
        if let Some((source, context)) = template {
            result.push(ProjectionActionLedger {
                domains,
                source,
                context,
                actions: Vec::new(),
            });
        }
    }
    Ok(result)
}

fn record_projection_actions(
    ledgers: &mut [ProjectionActionLedger],
    action: GranularAction,
    members: Range<usize>,
    aligned: &Alignment,
    units: [&[AttributedRunUnit]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
) -> Result<()> {
    for at in members {
        let pair = [aligned[at].0, aligned[at].1];
        let mut inputs = [None, None];
        let mut selected = None;
        for side in 0..2 {
            let Some(index) = pair[side] else {
                continue;
            };
            let unit = &units[side][index];
            let Some(source) = sources[side][unit.owner].as_ref() else {
                continue;
            };
            for slot in unit.typed_sources.iter().flatten() {
                if let Some((ledger_index, domain)) =
                    ledgers.iter().enumerate().find_map(|(i, ledger)| {
                        ledger.domains[side]
                            .filter(|(owner, ordinal, _)| {
                                *owner == source.owner && *ordinal == slot.ordinal
                            })
                            .map(|domain| (i, domain))
                    })
                {
                    if selected.is_some_and(|previous| previous != ledger_index)
                        || inputs[side].is_some()
                    {
                        return Err(Error::Other(
                            "projection action crosses selected logical occurrences".into(),
                        ));
                    }
                    selected = Some(ledger_index);
                    let (start, end) = slot.scalars.unwrap_or((0, domain.2));
                    inputs[side] = Some(start..end);
                }
            }
        }
        if let Some(index) = selected {
            ledgers[index].actions.push((inputs, action));
        }
    }
    Ok(())
}

fn attach_projection_actions(
    leaves: &mut [Vec<LeafPublication>],
    ledgers: &[ProjectionActionLedger],
) -> Result<()> {
    for ledger in ledgers {
        validate_projection_actions(&ledger.domains, &ledger.actions)?;
    }
    for leaf in leaves.iter_mut().flatten() {
        let ledger = ledgers
            .iter()
            .find(|ledger| {
                ledger.domains.iter().flatten().any(|(owner, ordinal, _)| {
                    *owner == leaf.physical.0 && *ordinal == leaf.physical.1
                })
            })
            .ok_or_else(|| {
                Error::Other("emitted leaf lost its selected logical occurrence".into())
            })?;
        let (owner, ordinal, _) = ledger.domains.iter().flatten().next().copied().unwrap();
        leaf.owner = owner;
        leaf.ordinal = ordinal;
        leaf.source = ledger.source.clone();
        leaf.context = ledger.context.clone();
        leaf.domains = ledger.domains;
        leaf.actions = ledger.actions.clone();
    }
    Ok(())
}

fn validate_projection_actions(
    domains: &[Option<(usize, usize, usize)>; 2],
    actions: &[([Option<Range<usize>>; 2], GranularAction)],
) -> Result<()> {
    for side in 0..2 {
        let ranges = actions
            .iter()
            .filter_map(|(inputs, _)| inputs[side].clone())
            .collect::<Vec<_>>();
        let Some((_, _, length)) = domains[side] else {
            if !ranges.is_empty() {
                return Err(Error::Other(
                    "projection actions invent an absent input domain".into(),
                ));
            }
            continue;
        };
        let mut covered = 0;
        for range in &ranges {
            if range.start != covered || range.end < range.start || range.end > length {
                return Err(Error::Other(format!(
                    "projection input {side} action coverage is incomplete or repeated"
                )));
            }
            covered = range.end;
        }
        if covered != length || (length == 0 && ranges.len() != 1) {
            return Err(Error::Other(format!(
                "projection input {side} action coverage omits source scalars"
            )));
        }
    }
    for (inputs, action) in actions {
        let valid = match action {
            GranularAction::Preserve => inputs[0].is_some(),
            GranularAction::Equal | GranularAction::Replace => inputs.iter().all(Option::is_some),
            GranularAction::Delete => inputs[0].is_some() && inputs[1].is_none(),
            GranularAction::Insert | GranularAction::Drop => {
                inputs[0].is_none() && inputs[1].is_some()
            }
        };
        if !valid {
            return Err(Error::Other(
                "projection action has inconsistent input roles".into(),
            ));
        }
    }
    Ok(())
}

fn projection_unit_alignment(
    pairs: &[[Option<(usize, usize)>; 2]],
    units: [&[AttributedRunUnit]; 2],
    signatures: [&[String]; 2],
) -> Result<(Alignment, Vec<usize>)> {
    let mut output = Vec::new();
    let mut segments = Vec::new();
    let mut seen = [vec![false; units[0].len()], vec![false; units[1].len()]];
    for (segment, pair) in pairs.iter().enumerate() {
        let indices = [0, 1].map(|side| {
            units[side]
                .iter()
                .enumerate()
                .filter_map(|(index, unit)| {
                    pair[side]
                        .filter(|(owner, ordinal)| {
                            unit.owner == *owner
                                && (if *ordinal == usize::MAX {
                                    unit.typed_sources.is_empty()
                                } else {
                                    unit.typed_sources
                                        .iter()
                                        .flatten()
                                        .any(|slot| slot.ordinal == *ordinal)
                                })
                        })
                        .map(|_| index)
                })
                .collect::<Vec<_>>()
        });
        for side in 0..2 {
            for &index in &indices[side] {
                if std::mem::replace(&mut seen[side][index], true) {
                    return Err(Error::Other("projection source slot repeats a unit".into()));
                }
            }
        }
        let values = [0, 1].map(|side| {
            indices[side]
                .iter()
                .map(|index| signatures[side][*index].clone())
                .collect::<Vec<_>>()
        });
        for (left, right) in align(&values[0], &values[1]) {
            output.push((left.map(|i| indices[0][i]), right.map(|i| indices[1][i])));
            segments.push(segment);
        }
    }
    if seen.iter().flatten().any(|seen| !seen) {
        return Err(Error::Other("projection source slots omit a unit".into()));
    }
    Ok((output, segments))
}

/// Publish a source separator only when one-occurrence assembly would otherwise
/// merge two surviving Run/Word tokens. Ordinary comparison never calls this.
fn retain_projection_separators(
    grouped: &mut [(GranularAction, Range<usize>)],
    aligned: &Alignment,
    segments: &[usize],
    pairs: &[[Option<(usize, usize)>; 2]],
    units: [&[AttributedRunUnit]; 2],
    sources: &[Vec<Option<RunSource>>; 2],
    options: &ComparisonOptions,
) -> Result<()> {
    if !options.ignore_whitespace || options.granularity == ComparisonGranularity::Character {
        return Ok(());
    }
    let mut actions = vec![GranularAction::Drop; aligned.len()];
    for (action, members) in grouped.iter() {
        actions[members.clone()].fill(*action);
    }
    // Source raw slots are ordered relative to the actual selected typed slot.
    // A right edge may retain a suffix, but that suffix cannot precede the edge.
    let edge = |at: usize, before: bool| -> Result<Option<char>> {
        let (side, index) = match actions[at] {
            GranularAction::Preserve | GranularAction::Equal => (0, aligned[at].0),
            GranularAction::Replace | GranularAction::Insert => (1, aligned[at].1),
            GranularAction::Delete | GranularAction::Drop => return Ok(None),
        };
        let unit = &units[side][index.unwrap()];
        let [RunContent::Text(text)] = unit.run.content.as_slice() else {
            return Ok(None);
        };
        let [Some(selected)] = unit.typed_sources.as_slice() else {
            return Ok(None);
        };
        if !unit.run.alt_drawings.is_empty() {
            return Ok(None);
        }
        let source = sources[side][unit.owner]
            .as_ref()
            .ok_or_else(|| Error::Other("separator neighbor has no actual source owner".into()))?;
        let span = source
            .typed_slots
            .get(selected.ordinal)
            .and_then(Option::as_ref)
            .ok_or_else(|| Error::Other("separator neighbor has no typed source slot".into()))?;
        for &(_, ordinal, _) in &unit.raw_sources {
            let raw = source
                .raw_slots
                .get(ordinal)
                .and_then(Option::as_ref)
                .ok_or_else(|| Error::Other("separator neighbor has no raw source slot".into()))?;
            if if before {
                raw.0.end > span.start
            } else {
                raw.0.start < span.end
            } {
                return Ok(None);
            }
        }
        Ok(if before {
            text.text.chars().next_back()
        } else {
            text.text.chars().next()
        })
    };
    for (action, members) in grouped {
        if *action != GranularAction::Drop {
            continue;
        }
        if members.len() != 1 {
            return Err(Error::Other(
                "separator Drop has ambiguous unit coverage".into(),
            ));
        }
        let at = members.start;
        let pair = pairs[segments[at]];
        let (Some(_), Some((right_owner, right_ordinal))) = (pair[0], pair[1]) else {
            continue;
        };
        let (None, Some(right_index)) = aligned[at] else {
            return Err(Error::Other(
                "separator Drop has inconsistent source roles".into(),
            ));
        };
        let right = &units[1][right_index];
        let [Some(selected)] = right.typed_sources.as_slice() else {
            continue;
        };
        if right.owner != right_owner || selected.ordinal != right_ordinal {
            return Err(Error::Other(
                "separator crosses its selected source leaf".into(),
            ));
        }
        let source = sources[1][right.owner]
            .as_ref()
            .ok_or_else(|| Error::Other("separator has no actual source owner".into()))?;
        let span = source
            .typed_slots
            .get(selected.ordinal)
            .and_then(Option::as_ref)
            .ok_or_else(|| Error::Other("separator has no typed source slot".into()))?;
        if source_content_metadata(source, span.clone())?.is_empty() {
            continue;
        }
        let [RunContent::Text(text)] = right.run.content.as_slice() else {
            continue;
        };
        if text.text.is_empty() || !text.text.chars().all(char::is_whitespace) {
            return Err(Error::Other(
                "separator is not actual ignored whitespace".into(),
            ));
        }
        let neighbor = |before| -> Result<Option<char>> {
            let mut index = at;
            loop {
                let next = if before {
                    index.checked_sub(1)
                } else {
                    index.checked_add(1).filter(|index| *index < aligned.len())
                };
                let Some(next) = next else { return Ok(None) };
                index = next;
                if segments[index] != segments[at] {
                    return Ok(None);
                }
                if matches!(
                    actions[index],
                    GranularAction::Delete | GranularAction::Drop
                ) {
                    continue;
                }
                return edge(index, before);
            }
        };
        let (Some(left), Some(right_edge)) = (neighbor(true)?, neighbor(false)?) else {
            continue;
        };
        if left.is_whitespace() || right_edge.is_whitespace() {
            continue;
        }
        let joined = CT_Text {
            text: format!("{left}{right_edge}"),
            preserve_space: false,
        };
        if granular_text(&joined, options).len() != 1 {
            continue;
        }
        *action = GranularAction::Insert;
    }
    Ok(())
}

/// The aligned pairs grouped into the pieces the redline writes as one run.
///
/// The granularity sets what the diff compares, never how many runs carry the
/// result. A change takes every consecutive pair of its kind that the same
/// physical runs own. Unchanged text takes every consecutive pair that
/// continues the same original and edited text element, so it stays one run
/// between two revisions whether the units are words or characters.
fn coalesced_granular_alignment(
    aligned: &[(Option<usize>, Option<usize>)],
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
) -> Vec<(GranularAction, Range<usize>)> {
    let owners = |at: usize| {
        (
            aligned[at].0.map(|index| original_units[index].owner),
            aligned[at].1.map(|index| edited_units[index].owner),
        )
    };
    // The text element each side of an unchanged pair is a fragment of.
    let elements = |at: usize| {
        aligned[at]
            .0
            .and_then(|index| unit_text_element(&original_units[index]))
            .zip(
                aligned[at]
                    .1
                    .and_then(|index| unit_text_element(&edited_units[index])),
            )
    };
    let mut groups = Vec::new();
    let mut start = 0usize;
    while start < aligned.len() {
        let action = granular_action(
            aligned[start],
            original_units,
            edited_units,
            original_signatures,
            edited_signatures,
        );
        let continues = |at: usize| match action {
            GranularAction::Replace | GranularAction::Delete | GranularAction::Insert => true,
            GranularAction::Equal => elements(start).is_some() && elements(at) == elements(start),
            GranularAction::Preserve | GranularAction::Drop => false,
        };
        let mut end = start + 1;
        while end < aligned.len()
            && continues(end)
            && granular_action(
                aligned[end],
                original_units,
                edited_units,
                original_signatures,
                edited_signatures,
            ) == action
            && owners(end) == owners(start)
        {
            end += 1;
        }
        groups.push((action, start..end));
        start = end;
    }
    groups
}

/// The content ordinal of the text element a plain unit is a fragment of.
fn unit_text_element(unit: &AttributedRunUnit) -> Option<usize> {
    unit_plain_text(unit)?;
    match unit.typed_sources.as_slice() {
        [Some(selected)] => Some(selected.ordinal),
        _ => None,
    }
}

/// The text of a unit that holds nothing but text, which grouping may move
/// into a deletion or an insertion.
fn unit_plain_text(unit: &AttributedRunUnit) -> Option<&str> {
    match unit.run.content.as_slice() {
        [RunContent::Text(text)]
            if !unit.ignored
                && semantic_run_raw(&unit.run).is_empty()
                && unit.run.alt_drawings.is_empty() =>
        {
            Some(&text.text)
        }
        _ => None,
    }
}

/// Count word starts while carrying state across adjacent attributed units.
/// Character fragments from one word must not count as separate words.
fn text_words(text: Option<&str>, in_word: &mut bool) -> usize {
    let mut words = 0;
    if let Some(text) = text {
        for character in text.chars() {
            let word = character.is_alphanumeric() || character == '_';
            words += usize::from(word && !*in_word);
            *in_word = word;
        }
    }
    words
}

/// The word and character alignment with each stretch of changes written as
/// all its deleted units, then all its inserted units, as Word shows a
/// rewritten passage, and the segment of each pair.
///
/// A stretch is consecutive changed text units. It grows over an unchanged
/// stretch between two changes when that stretch is only whitespace, then
/// over one that has fewer words than the deletion and the insertion on each
/// side of it, in the spirit of diff-match-patch `diff_cleanupSemantic`, so a
/// one-word anchor between one-word replacements stays. Anything but plain
/// text ends a stretch: a field, a drawing, a preserved child, an ignored
/// unit, an unchanged unit whose formatting changed, and a shell segment
/// boundary, where a hyperlink, an inline control, a bookmark or a comment
/// range sits. Run granularity keeps its alignment.
fn grouped_text_alignment(
    aligned: &[(Option<usize>, Option<usize>)],
    segments: &[usize],
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    original_signatures: &[String],
    edited_signatures: &[String],
    options: &ComparisonOptions,
) -> (Alignment, Vec<usize>) {
    /// Consecutive pairs of one kind in one segment: `changed` is `None`
    /// for a pair grouping must not move.
    struct Block {
        pairs: Range<usize>,
        changed: Option<bool>,
        deleted_words: usize,
        inserted_words: usize,
        whitespace: bool,
        deleted_in_word: bool,
        inserted_in_word: bool,
        original_owner: Option<usize>,
        edited_owner: Option<usize>,
    }
    if options.granularity == ComparisonGranularity::Run {
        return (aligned.to_vec(), segments.to_vec());
    }
    let same_owner_formatting =
        |left: Option<usize>, right: Option<usize>, units: &[AttributedRunUnit]| {
            left.zip(right).is_none_or(|(left, right)| {
                units[left].owner == units[right].owner
                    || run_formatting(&units[left].run) == run_formatting(&units[right].run)
            })
        };
    let mut blocks = Vec::<Block>::new();
    for (index, &(left, right)) in aligned.iter().enumerate() {
        let left_text = left.map(|index| unit_plain_text(&original_units[index]));
        let right_text = right.map(|index| unit_plain_text(&edited_units[index]));
        let plain = left_text.is_none_or(|text| text.is_some())
            && right_text.is_none_or(|text| text.is_some());
        let (left_text, right_text) = (left_text.flatten(), right_text.flatten());
        let same_formatting = match (left, right) {
            (Some(i), Some(j)) => {
                run_formatting(&original_units[i].run) == run_formatting(&edited_units[j].run)
            }
            _ => true,
        };
        let changed = match granular_action(
            (left, right),
            original_units,
            edited_units,
            original_signatures,
            edited_signatures,
        ) {
            // A replacement that ignores formatting writes its insertion in
            // the formatting of its deletion, which an insertion of its own
            // would not.
            GranularAction::Replace if plain && (same_formatting || !options.ignore_formatting) => {
                Some(true)
            }
            GranularAction::Delete | GranularAction::Insert if plain => Some(true),
            GranularAction::Equal if plain && same_formatting => Some(false),
            _ => None,
        };
        let inserted_text = right_text.filter(|_| changed == Some(true));
        let whitespace = left_text.is_some_and(|text| text.trim().is_empty());
        match blocks.last_mut() {
            Some(block)
                if changed.is_some()
                    && block.changed == changed
                    && segments[block.pairs.start] == segments[index]
                    && same_owner_formatting(block.original_owner, left, original_units)
                    && same_owner_formatting(block.edited_owner, right, edited_units) =>
            {
                block.pairs.end = index + 1;
                block.deleted_words += text_words(left_text, &mut block.deleted_in_word);
                block.inserted_words += text_words(inserted_text, &mut block.inserted_in_word);
                block.whitespace &= whitespace;
                block.original_owner = left.or(block.original_owner);
                block.edited_owner = right.or(block.edited_owner);
            }
            _ => {
                let mut deleted_in_word = false;
                let mut inserted_in_word = false;
                blocks.push(Block {
                    pairs: index..index + 1,
                    changed,
                    deleted_words: text_words(left_text, &mut deleted_in_word),
                    inserted_words: text_words(inserted_text, &mut inserted_in_word),
                    whitespace,
                    deleted_in_word,
                    inserted_in_word,
                    original_owner: left,
                    edited_owner: right,
                });
            }
        }
    }
    let between_changes = |block: usize| {
        let segment = segments[blocks[block].pairs.start];
        block > 0
            && block + 1 < blocks.len()
            && blocks[block].changed == Some(false)
            && [&blocks[block - 1], &blocks[block + 1]].iter().all(|side| {
                side.changed == Some(true)
                    && segments[side.pairs.start] == segment
                    && same_owner_formatting(
                        side.original_owner,
                        blocks[block].original_owner,
                        original_units,
                    )
                    && same_owner_formatting(
                        side.edited_owner,
                        blocks[block].edited_owner,
                        edited_units,
                    )
            })
    };
    let mut folded = (0..blocks.len())
        .map(|block| between_changes(block) && blocks[block].whitespace)
        .collect::<Vec<_>>();
    // The words deleted and inserted by the change, grown over whitespace,
    // that holds each change block.
    let mut stretch = vec![0usize; blocks.len()];
    let mut stretch_words = vec![(0usize, 0usize); blocks.len()];
    for block in 0..blocks.len() {
        if blocks[block].changed == Some(true) {
            stretch[block] = if block >= 2 && folded[block - 1] {
                stretch[block - 2]
            } else {
                block
            };
            stretch_words[stretch[block]].0 += blocks[block].deleted_words;
            stretch_words[stretch[block]].1 += blocks[block].inserted_words;
        }
    }
    for block in 0..blocks.len() {
        if between_changes(block) && !folded[block] {
            let (before, after) = (
                stretch_words[stretch[block - 1]],
                stretch_words[stretch[block + 1]],
            );
            folded[block] =
                blocks[block].deleted_words < before.0.min(before.1).min(after.0).min(after.1);
        }
    }
    let mut grouped = Vec::with_capacity(aligned.len());
    let mut grouped_segments = Vec::with_capacity(aligned.len());
    let mut block = 0;
    while block < blocks.len() {
        let mut end = block + 1;
        if blocks[block].changed == Some(true) {
            while end + 1 < blocks.len() && folded[end] {
                end += 2;
            }
        }
        let pairs = blocks[block].pairs.start..blocks[end - 1].pairs.end;
        if blocks[block].changed == Some(true) && pairs.len() > 1 {
            let deleted = pairs.clone().filter_map(|index| aligned[index].0);
            let inserted = pairs.clone().filter_map(|index| aligned[index].1);
            grouped.extend(deleted.map(|index| (Some(index), None)));
            grouped.extend(inserted.map(|index| (None, Some(index))));
            grouped_segments.resize(grouped.len(), segments[pairs.start]);
        } else {
            grouped.extend_from_slice(&aligned[pairs.clone()]);
            grouped_segments.extend_from_slice(&segments[pairs]);
        }
        block = end;
    }
    (grouped, grouped_segments)
}

/// The units at `indices`, all fragments of one run of `runs`, as one run.
///
/// Consecutive fragments of one text element are written back as one text
/// with one source selection, so a merged run holds one `w:t` or `w:delText`
/// per stretch of its source text, never one per compared unit.
fn merge_unit_runs(
    runs: &[CT_R],
    units: &[AttributedRunUnit],
    indices: &[usize],
    raw_sources: &mut Vec<(usize, usize, usize)>,
    typed_sources: &mut Vec<Option<TypedSourceSelection>>,
) -> Result<Option<CT_R>> {
    let Some(first) = indices.first().and_then(|index| units.get(*index)) else {
        return Ok(None);
    };
    let mut merged = first.run.clone();
    raw_sources.extend_from_slice(&first.raw_sources);
    typed_sources.extend_from_slice(&first.typed_sources);
    let property_boundary = usize::from(merged.properties.is_some());
    let mut joined = false;
    for &index in &indices[1..] {
        let unit = &units[index];
        if unit.owner != first.owner {
            return Err(Error::Other(
                "comparison merged different physical run owners".into(),
            ));
        }
        let next = &unit.run;
        let continued = join_text_fragment(&mut merged, typed_sources, unit, property_boundary);
        joined |= continued;
        // The raw children of a joined fragment follow the text it extends.
        let content_offset = merged.content.len() - usize::from(continued);
        if !continued {
            merged.content.extend(next.content.iter().cloned());
            typed_sources.extend_from_slice(&unit.typed_sources);
        }
        merged
            .alt_drawings
            .extend(next.alt_drawings.iter().cloned());
        for (local, (raw, &encoded)) in next
            .extra_xml
            .iter()
            .zip(&next.extra_xml_positions)
            .enumerate()
        {
            if CT_R::raw_child_is_root_attributes(encoded) {
                continue;
            }
            if let Some(&(_, ordinal, original_boundary)) =
                unit.raw_sources.iter().find(|(at, _, _)| *at == local)
            {
                raw_sources.push((merged.extra_xml.len(), ordinal, original_boundary));
            }
            let boundary = CT_R::raw_child_position(encoded);
            let mut rebased = encoded;
            CT_R::set_raw_child_position(&mut rebased, boundary + content_offset);
            merged.extra_xml.push(raw.clone());
            merged.extra_xml_positions.push(rebased);
        }
        if property_boundary == 0 && next.properties.is_some() {
            merged.properties = next.properties.clone();
        }
    }
    if raw_sources.windows(2).any(|pair| pair[0].1 >= pair[1].1) {
        return Err(Error::Other(
            "fragment raw source order or multiplicity is invalid".into(),
        ));
    }
    if joined {
        // A joined text reads its space flag as a unit of its own would, and
        // one that is its whole source element again selects it whole.
        let source = runs
            .get(first.owner)
            .ok_or_else(|| Error::Other("merged run owner is absent".into()))?;
        for (content, selected) in merged.content.iter_mut().zip(typed_sources.iter_mut()) {
            let (
                RunContent::Text(text) | RunContent::DeletedText(text),
                Some(TypedSourceSelection {
                    ordinal,
                    scalars: scalars @ Some(_),
                }),
            ) = (content, selected)
            else {
                continue;
            };
            let Some(RunContent::Text(actual) | RunContent::DeletedText(actual)) =
                source.content.get(*ordinal)
            else {
                return Err(Error::Other(
                    "joined text has no actual source element".into(),
                ));
            };
            text.preserve_space = actual.preserve_space || has_edge_whitespace(&text.text);
            if *scalars == Some((0, actual.text.chars().count())) {
                *scalars = None;
            }
        }
    }
    Ok(Some(merged))
}

/// Append `unit` to the text that ends `merged` when it holds the next scalars
/// of the same source text element with no raw child between the two, and
/// report whether it did. `typed` holds one selection per content of `merged`.
fn join_text_fragment(
    merged: &mut CT_R,
    typed: &mut [Option<TypedSourceSelection>],
    unit: &AttributedRunUnit,
    property_boundary: usize,
) -> bool {
    let content_end = property_boundary + merged.content.len();
    let raw_positions = |run: &CT_R| {
        run.extra_xml_positions
            .iter()
            .filter(|encoded| !CT_R::raw_child_is_root_attributes(**encoded))
            .map(|encoded| CT_R::raw_child_position(*encoded))
            .collect::<Vec<_>>()
    };
    if !unit.run.alt_drawings.is_empty()
        || raw_positions(merged)
            .iter()
            .any(|position| *position >= content_end)
        || raw_positions(&unit.run)
            .iter()
            .any(|position| *position <= property_boundary)
    {
        return false;
    }
    let (
        Some(TypedSourceSelection {
            ordinal: previous,
            scalars: Some((_, previous_end)),
        }),
        [
            Some(TypedSourceSelection {
                ordinal,
                scalars: Some((start, end)),
            }),
        ],
    ) = (
        typed.last_mut().and_then(Option::as_mut),
        unit.typed_sources.as_slice(),
    )
    else {
        return false;
    };
    if *previous != *ordinal || *previous_end != *start {
        return false;
    }
    match (merged.content.last_mut(), unit.run.content.as_slice()) {
        (Some(RunContent::Text(text)), [RunContent::Text(fragment)])
        | (Some(RunContent::DeletedText(text)), [RunContent::DeletedText(fragment)]) => {
            text.text.push_str(&fragment.text);
            *previous_end = *end;
            true
        }
        _ => false,
    }
}

/// The paragraph source with `properties` in place of its own `w:pPr`. A
/// self-closing paragraph, such as an empty one, is opened first, so the
/// properties and runs go inside it.
fn paragraph_with_properties(
    paragraph: &CT_P,
    original_source: Option<&str>,
    properties: &str,
) -> Result<String> {
    let mut source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(paragraph), Ok)?;
    // The shell opens a self-closing start tag, which is then the whole source.
    let (opening, closing) = paragraph_shell(Some(&source))?;
    if !source.starts_with(&opening) {
        source = format!("{opening}{closing}");
    }
    let property_spans = direct_word_element_spans(&source, "pPr")?;
    match (property_spans.first(), properties.is_empty()) {
        (Some(span), false) => source.replace_range(span.clone(), properties),
        (Some(span), true) => source.replace_range(span.clone(), ""),
        (None, false) => source.insert_str(opening.len(), properties),
        (None, true) => {}
    }
    Ok(source)
}

fn replace_paragraph_properties_and_runs(
    paragraph: &CT_P,
    original_source: Option<&str>,
    properties: &str,
    runs: &[String],
) -> Result<String> {
    let source = paragraph_with_properties(paragraph, original_source, properties)?;
    replace_paragraph_run_elements(paragraph, &source, runs)
}

#[allow(clippy::too_many_arguments)]
fn interleave_granular_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    original_units: &[AttributedRunUnit],
    edited_units: &[AttributedRunUnit],
    aligned: &[(Option<usize>, Option<usize>)],
    segment_runs: &[usize],
    replacements: &[String],
    fragment_publications: &[Vec<LeafPublication>],
    properties: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let source = paragraph_with_properties(original, original_source, properties)?;
    let spans = modeled_paragraph_run_spans(original, &source)?;
    if spans.len() != original.runs.len()
        || aligned.len() != replacements.len()
        || aligned.len() != segment_runs.len()
    {
        return Err(Error::Other(format!(
            "comparison could not correlate granular run owners at {location}"
        )));
    }
    let run_start = |run: usize| {
        spans
            .get(run)
            .map_or_else(|| paragraph_close_start(&source), |span| Ok(span.start))
    };
    let insertion_boundary = run_start(0)?;
    let mut output = source[..insertion_boundary].to_owned();
    let mut cursor = insertion_boundary;
    let mut consumed_owner = None;
    let mut original_owner_units = vec![0usize; original.runs.len()];
    let mut edited_owner_units = vec![0usize; edited.runs.len()];
    for unit in original_units {
        original_owner_units[unit.owner] += 1;
    }
    for unit in edited_units {
        edited_owner_units[unit.owner] += 1;
    }
    for (replacement_index, (((left, right), replacement), &segment_run)) in aligned
        .iter()
        .zip(replacements)
        .zip(segment_runs)
        .enumerate()
    {
        // Text inserted at the start of a shell segment comes after the
        // shell tags that open it.
        let segment_start = run_start(segment_run)?;
        if segment_start > cursor {
            output.push_str(&source[cursor..segment_start]);
            cursor = segment_start;
        }
        let mut exact_run = None;
        if let Some(unit) = left.map(|index| &original_units[index])
            && consumed_owner != Some(unit.owner)
        {
            let span = &spans[unit.owner];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
            consumed_owner = Some(unit.owner);
            if let Some(right_owner) = right.map(|index| edited_units[index].owner)
                && !run_is_field(&original.runs[unit.owner])
                && original.runs[unit.owner] == edited.runs[right_owner]
                && original_owner_units[unit.owner] == 1
                && edited_owner_units[right_owner] == 1
            {
                exact_run = Some(&source[span.clone()]);
            }
        }
        if exact_run.is_none() {
            let mut leaves = fragment_publications[replacement_index].clone();
            shift_leaf_publications(&mut leaves, output.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
        }
        output.push_str(exact_run.unwrap_or(replacement));
    }
    output.push_str(&source[cursor..]);
    compare_granular_controls(
        original,
        edited,
        &output,
        location,
        metadata,
        diagnostics,
        leaf_mark,
    )
}

/// Compare the inline controls of a paragraph whose revised runs are in
/// `output`, which still holds the original controls.
fn compare_granular_controls(
    original: &CT_P,
    edited: &CT_P,
    output: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    leaf_mark: usize,
) -> Result<String> {
    let control_spans = direct_word_element_spans(output, "sdt")?;
    if control_spans.len() != original.content_controls.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate granular content controls at {location}"
        )));
    }
    let mut child_leaves = Vec::new();
    let mut control_replacements = Vec::with_capacity(control_spans.len());
    let mut sources = source_children(output, &control_spans).into_iter();
    for (index, ((_, _, _, left), (_, _, _, right))) in original
        .content_controls
        .iter()
        .zip(&edited.content_controls)
        .enumerate()
    {
        let child_mark = metadata.leaf_publications.len();
        let source = sources.next().expect("one source per control span");
        control_replacements.push(compare_control_from_xml(
            left,
            right,
            &format!("{location}/content-control[{index}]"),
            metadata,
            diagnostics,
            &source?,
        )?);
        child_leaves.push(metadata.leaf_publications.split_off(child_mark));
    }
    splice_leaf_publications(
        metadata,
        leaf_mark,
        &control_spans,
        &control_replacements,
        child_leaves,
    )?;
    replace_direct_word_elements(output, "sdt", &control_replacements)
}

fn attributed_run_units(runs: &[CT_R], options: &ComparisonOptions) -> Vec<AttributedRunUnit> {
    let mut units = Vec::new();
    for (owner, run) in runs.iter().enumerate() {
        let unit_start = units.len();
        let mut content_units = Vec::<Range<usize>>::with_capacity(run.content.len());
        for (content_index, content) in run.content.iter().enumerate() {
            let start = units.len() - unit_start;
            let fragments = match content {
                RunContent::Text(text) => granular_text(text, options)
                    .into_iter()
                    .map(RunContent::Text)
                    .collect::<Vec<_>>(),
                RunContent::DeletedText(text) => granular_text(text, options)
                    .into_iter()
                    .map(RunContent::DeletedText)
                    .collect::<Vec<_>>(),
                content => vec![content.clone()],
            };
            let whole = fragments.len() == 1;
            let mut scalar_cursor = 0;
            for fragment in fragments {
                let scalars = match &fragment {
                    RunContent::Text(text) | RunContent::DeletedText(text) if !whole => {
                        let end = scalar_cursor + text.text.chars().count();
                        let range = Some((scalar_cursor, end));
                        scalar_cursor = end;
                        range
                    }
                    _ => None,
                };
                let ignored = ignored_run_content(&fragment, options);
                units.push(AttributedRunUnit {
                    run: CT_R {
                        properties: run.properties.clone(),
                        content: vec![fragment],
                        extra_xml: Vec::new(),
                        extra_xml_positions: Vec::new(),
                        alt_drawings: Vec::new(),
                    },
                    ignored,
                    owner,
                    raw_sources: Vec::new(),
                    typed_sources: vec![Some(TypedSourceSelection {
                        ordinal: content_index,
                        scalars,
                    })],
                });
            }
            content_units.push(start..units.len() - unit_start);
        }
        if units.len() == unit_start {
            units.push(AttributedRunUnit {
                run: CT_R {
                    properties: run.properties.clone(),
                    content: Vec::new(),
                    extra_xml: Vec::new(),
                    extra_xml_positions: Vec::new(),
                    alt_drawings: Vec::new(),
                },
                ignored: false,
                owner,
                raw_sources: Vec::new(),
                typed_sources: Vec::new(),
            });
        }
        attribute_raw_children(run, &content_units, &mut units[unit_start..]);
        let property_boundary = usize::from(run.properties.is_some());
        let mut inserted = 0usize;
        for (ordinal, (raw, &encoded)) in run
            .extra_xml
            .iter()
            .zip(&run.extra_xml_positions)
            .enumerate()
        {
            if !is_text_box_host_marker(raw) {
                continue;
            }
            let boundary = CT_R::raw_child_position(encoded);
            let offset = if boundary <= property_boundary || content_units.is_empty() {
                0
            } else {
                let content_index = (boundary - property_boundary - 1).min(content_units.len() - 1);
                content_units[content_index].end
            };
            let mut position = encoded;
            CT_R::set_raw_child_position(&mut position, property_boundary);
            units.insert(
                unit_start + offset + inserted,
                AttributedRunUnit {
                    run: CT_R {
                        properties: run.properties.clone(),
                        content: Vec::new(),
                        extra_xml: vec![raw.clone()],
                        extra_xml_positions: vec![position],
                        alt_drawings: Vec::new(),
                    },
                    ignored: false,
                    owner,
                    raw_sources: vec![(0, ordinal, boundary)],
                    typed_sources: Vec::new(),
                },
            );
            inserted += 1;
        }
        preserve_marker_raw_order(run, &mut units[unit_start..]);
        // Each AlternateContent projection follows its actual flagged raw slot.
        let projected = run
            .extra_xml_positions
            .iter()
            .enumerate()
            .filter(|(_, encoded)| CT_R::raw_child_has_alternate_content_drawing(**encoded))
            .map(|(ordinal, _)| ordinal)
            .collect::<Vec<_>>();
        for unit in &mut units[unit_start..] {
            for &(_, ordinal, _) in &unit.raw_sources {
                if let Some(index) = projected.iter().position(|candidate| *candidate == ordinal)
                    && let Some(drawing) = run.alt_drawings.get(index)
                {
                    unit.run.alt_drawings.push(drawing.clone());
                }
            }
        }
        if projected.len() < run.alt_drawings.len() {
            units[unit_start]
                .run
                .alt_drawings
                .extend_from_slice(&run.alt_drawings[projected.len()..]);
        }
    }
    units
}

fn attribute_raw_children(
    run: &CT_R,
    content_units: &[Range<usize>],
    units: &mut [AttributedRunUnit],
) {
    let property_boundary = usize::from(run.properties.is_some());
    for (ordinal, (raw, &encoded)) in run
        .extra_xml
        .iter()
        .zip(&run.extra_xml_positions)
        .enumerate()
    {
        if is_text_box_host_marker(raw) {
            continue;
        }
        let boundary = CT_R::raw_child_position(encoded);
        let comment = raw_comment_content_index(run, ordinal);
        let (unit_index, position) = if let Some(index) = comment {
            (content_units[index].start, property_boundary)
        } else if boundary <= property_boundary || content_units.is_empty() {
            (0, boundary.min(property_boundary))
        } else {
            let content_index = (boundary - property_boundary - 1).min(content_units.len() - 1);
            let range = &content_units[content_index];
            (range.end.saturating_sub(1), property_boundary + 1)
        };
        let local = units[unit_index].run.extra_xml.len();
        if !CT_R::raw_child_is_root_attributes(encoded) {
            units[unit_index]
                .raw_sources
                .push((local, ordinal, boundary));
        }
        units[unit_index].run.extra_xml.push(raw.clone());
        let mut attributed = encoded;
        CT_R::set_raw_child_position(&mut attributed, position);
        units[unit_index].run.extra_xml_positions.push(attributed);
    }
}

fn preserve_marker_raw_order(run: &CT_R, units: &mut [AttributedRunUnit]) {
    let hosts = units
        .iter()
        .enumerate()
        .flat_map(|(index, unit)| {
            unit.raw_sources
                .iter()
                .filter_map(move |&(local, ordinal, boundary)| {
                    is_text_box_host_marker(&unit.run.extra_xml[local])
                        .then_some((index, ordinal, boundary))
                })
        })
        .collect::<Vec<_>>();
    for (host, host_ordinal, host_boundary) in hosts {
        let mut moved = Vec::new();
        for (index, unit) in units.iter_mut().enumerate() {
            if index == host {
                continue;
            }
            let provenance = std::mem::take(&mut unit.raw_sources);
            let raw = std::mem::take(&mut unit.run.extra_xml);
            let positions = std::mem::take(&mut unit.run.extra_xml_positions);
            for (local, (raw, position)) in raw.into_iter().zip(positions).enumerate() {
                let source = provenance.iter().find(|(at, _, _)| *at == local).copied();
                if let Some((_, ordinal, boundary)) = source
                    && boundary == host_boundary
                    && ordinal > host_ordinal
                    && !is_text_box_host_marker(&raw)
                {
                    moved.push((ordinal, boundary, raw, position));
                } else {
                    if let Some((_, ordinal, boundary)) = source {
                        unit.raw_sources
                            .push((unit.run.extra_xml.len(), ordinal, boundary));
                    }
                    unit.run.extra_xml.push(raw);
                    unit.run.extra_xml_positions.push(position);
                }
            }
        }
        moved.sort_by_key(|(ordinal, _, _, _)| *ordinal);
        for (ordinal, boundary, raw, mut position) in moved {
            let unit = &mut units[host];
            CT_R::set_raw_child_position(&mut position, usize::from(run.properties.is_some()));
            unit.raw_sources
                .push((unit.run.extra_xml.len(), ordinal, boundary));
            unit.run.extra_xml.push(raw);
            unit.run.extra_xml_positions.push(position);
        }
    }
}

fn raw_comment_content_index(run: &CT_R, ordinal: usize) -> Option<usize> {
    let raw = run.extra_xml.get(ordinal)?;
    let mut reader = NsReader::from_reader(raw.as_slice());
    let (namespace, event) = reader.read_resolved_event().ok()?;
    if !matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == W_NS.as_bytes()) {
        return None;
    }
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return None;
    };
    if root.local_name().as_ref() != b"commentReference" {
        return None;
    }
    run.content.iter().position(|content| matches!(content, RunContent::CommentReference {raw_before, ..} if *raw_before == ordinal + 1))
}

/// Prove the attribution partition once per physical input role. This validates
/// identities supplied by the parser, never a lookup by generated XML spelling.
fn validate_raw_attribution(runs: &[CT_R], units: &[AttributedRunUnit]) -> Result<()> {
    let mut seen = runs
        .iter()
        .map(|run| vec![false; run.extra_xml.len()])
        .collect::<Vec<_>>();
    let mut previous = vec![None; runs.len()];
    let mut typed_coverage = runs
        .iter()
        .map(|run| vec![(false, 0usize); run.content.len()])
        .collect::<Vec<_>>();
    for unit in units {
        let run = runs
            .get(unit.owner)
            .ok_or_else(|| Error::Other("attributed run owner is absent".into()))?;
        if unit.typed_sources.len() != unit.run.content.len() {
            return Err(Error::Other(
                "typed attribution coverage is incomplete".into(),
            ));
        }
        for (content, selected) in unit.run.content.iter().zip(&unit.typed_sources) {
            let selected = selected
                .ok_or_else(|| Error::Other("typed attribution has no source role".into()))?;
            let original = run
                .content
                .get(selected.ordinal)
                .ok_or_else(|| Error::Other("typed source ordinal is absent".into()))?;
            let (whole, cursor) = &mut typed_coverage[unit.owner][selected.ordinal];
            if *whole {
                return Err(Error::Other(
                    "typed attribution repeats a whole source role".into(),
                ));
            }
            if let Some((start, end)) = selected.scalars {
                let (
                    RunContent::Text(actual) | RunContent::DeletedText(actual),
                    RunContent::Text(fragment) | RunContent::DeletedText(fragment),
                ) = (original, content)
                else {
                    return Err(Error::Other(
                        "typed scalar selection has no text source".into(),
                    ));
                };
                if start != *cursor
                    || end < start
                    || end > actual.text.chars().count()
                    || actual
                        .text
                        .chars()
                        .skip(start)
                        .take(end - start)
                        .collect::<String>()
                        != fragment.text
                {
                    return Err(Error::Other(
                        "typed source scalar partition is incomplete or reordered".into(),
                    ));
                }
                *cursor = end;
            } else {
                if *cursor != 0
                    || match (original, content) {
                        (RunContent::Text(a), RunContent::Text(b))
                        | (RunContent::DeletedText(a), RunContent::DeletedText(b)) => {
                            a.text != b.text
                        }
                        _ => original != content,
                    }
                {
                    return Err(Error::Other("typed whole source authority changed".into()));
                }
                *whole = true;
            }
        }
        for &(local, ordinal, boundary) in &unit.raw_sources {
            if ordinal >= run.extra_xml.len()
                || local >= unit.run.extra_xml.len()
                || seen[unit.owner][ordinal]
                || previous[unit.owner].is_some_and(|previous| ordinal <= previous)
                || CT_R::raw_child_position(run.extra_xml_positions[ordinal]) != boundary
                || run.extra_xml[ordinal] != unit.run.extra_xml[local]
            {
                return Err(Error::Other(
                    "raw attribution has an invalid or repeated source occurrence".into(),
                ));
            }
            seen[unit.owner][ordinal] = true;
            previous[unit.owner] = Some(ordinal);
        }
    }
    for (run, roles) in runs.iter().zip(typed_coverage) {
        for (content, (whole, covered)) in run.content.iter().zip(roles) {
            let count = match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => text.text.chars().count(),
                _ => 0,
            };
            if !whole && (count == 0 || covered != count) {
                return Err(Error::Other(
                    "typed attribution lost an input occurrence".into(),
                ));
            }
        }
    }
    for (run, seen) in runs.iter().zip(seen) {
        if seen
            .iter()
            .zip(&run.extra_xml_positions)
            .any(|(seen, encoded)| !*seen && !CT_R::raw_child_is_root_attributes(*encoded))
        {
            return Err(Error::Other(
                "raw attribution lost an input occurrence".into(),
            ));
        }
    }
    Ok(())
}

fn is_text_box_host_marker(raw: &[u8]) -> bool {
    raw.windows(b"urn:rdocx:comparison:private".len())
        .any(|window| window == b"urn:rdocx:comparison:private")
        && raw
            .windows(b"textBoxHost".len())
            .any(|window| window == b"textBoxHost")
}

fn granular_text(text: &CT_Text, options: &ComparisonOptions) -> Vec<CT_Text> {
    let fragments = match options.granularity {
        ComparisonGranularity::Run if options.ignore_whitespace => whitespace_fragments(&text.text),
        ComparisonGranularity::Run => vec![text.text.clone()],
        ComparisonGranularity::Character => text.text.chars().map(String::from).collect(),
        ComparisonGranularity::Word => word_fragments(&text.text),
    };
    if fragments.is_empty() {
        return vec![text.clone()];
    }
    fragments
        .into_iter()
        .map(|value| CT_Text {
            // A unit is written back as its own `w:t`, where edge whitespace
            // needs the flag to survive. Whitespace in a unit then reads as
            // whitespace whatever the source flag was.
            preserve_space: text.preserve_space || has_edge_whitespace(&value),
            text: value,
        })
        .collect()
}

/// The whole-run signature that agrees with the attributed units.
///
/// It reads the space flag the way `granular_text` writes it on every unit,
/// so a run that differs only by the flag stays whole instead of being
/// rewritten one unit per run, which would move run-indexed bookmark ends.
fn attributed_run_signature(run: &CT_R) -> String {
    let mut run = run.clone();
    for content in &mut run.content {
        if let RunContent::Text(text) | RunContent::DeletedText(text) = content {
            text.preserve_space |= has_edge_whitespace(&text.text);
        }
    }
    run_signature(&run)
}

fn whitespace_fragments(text: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut whitespace = None;
    for character in text.chars() {
        let next = character.is_whitespace();
        if whitespace.is_some_and(|value| value != next) {
            output.push(std::mem::take(&mut current));
        }
        current.push(character);
        whitespace = Some(next);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn word_fragments(text: &str) -> Vec<String> {
    fn class(character: char) -> u8 {
        if character.is_alphanumeric() || character == '_' {
            0
        } else if character.is_whitespace() {
            1
        } else {
            2
        }
    }
    let mut output = Vec::new();
    let mut current = String::new();
    let mut current_class = None;
    for character in text.chars() {
        let next_class = class(character);
        if current_class.is_some_and(|value| value != next_class) {
            output.push(std::mem::take(&mut current));
        }
        current.push(character);
        current_class = Some(next_class);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

fn ignored_run_content(content: &RunContent, options: &ComparisonOptions) -> bool {
    (options.ignore_fields && matches!(content, RunContent::Field(_)))
        || (options.ignore_comments && matches!(content, RunContent::CommentReference { .. }))
        || (options.ignore_whitespace
            && matches!(
                content,
                RunContent::Text(text) | RunContent::DeletedText(text)
                    if text.text.chars().all(char::is_whitespace)
            ))
}

fn run_is_ignored(run: &CT_R, options: &ComparisonOptions) -> bool {
    !run.content.is_empty()
        && run
            .content
            .iter()
            .all(|content| ignored_run_content(content, options))
        && semantic_run_raw(run).is_empty()
}

fn attributed_unit_signature(unit: &AttributedRunUnit) -> String {
    if unit.ignored {
        let kind = match unit.run.content.first() {
            Some(RunContent::Field(_)) => "ignored:field".to_owned(),
            Some(RunContent::CommentReference { .. }) => "ignored:comment".to_owned(),
            _ => "ignored:whitespace".to_owned(),
        };
        let raw = semantic_run_raw(&unit.run);
        format!("{kind}:{raw:?}:{:?}", unit.run.alt_drawings)
    } else {
        match unit.run.content.first() {
            Some(RunContent::CommentReference { id, .. }) if unit.run.content.len() == 1 => {
                let raw = semantic_run_raw(&unit.run);
                format!(
                    "comment:{id}:{:?}:{raw:?}:{:?}",
                    unit.run.properties, unit.run.alt_drawings
                )
            }
            _ => run_signature(&unit.run),
        }
    }
}

fn compare_complex_paragraph(
    original: &CT_P,
    edited: &CT_P,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if paragraph_signature_with_options(original, metadata.options)
        == paragraph_signature_with_options(edited, metadata.options)
        && !SourceMetadata::paragraph_contains_fields(original, metadata.options)?
    {
        if !metadata.options.ignore_formatting
            && paragraph_formatting(original) != paragraph_formatting(edited)
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        for (index, (left, right)) in original.runs.iter().zip(&edited.runs).enumerate() {
            if !metadata.options.ignore_formatting && run_formatting(left) != run_formatting(right)
            {
                formatting_diagnostic(diagnostics, format!("{location}/run[{index}]"));
            }
        }
        for (index, ((_, _, _, left), (_, _, _, right))) in original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .enumerate()
        {
            compare_control(
                left,
                right,
                &format!("{location}/content-control[{index}]"),
                metadata,
                diagnostics,
            )?;
        }
        return original_source
            .map(str::to_owned)
            .map_or_else(|| paragraph_xml(original), Ok);
    }
    if paragraph_boundaries_differ(original, edited, metadata.options)
        || (!metadata.options.ignore_formatting
            && paragraph_properties_differ(
                original.properties.as_ref(),
                edited.properties.as_ref(),
            ))
    {
        return Err(Error::Other(format!(
            "comparison cannot revise paragraph boundary structures at {location}"
        )));
    }

    let source = original_source
        .map(str::to_owned)
        .map_or_else(|| paragraph_xml(original), Ok)?;
    let run_spans = modeled_paragraph_run_spans(original, &source)?;
    if run_spans.len() != original.runs.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate complex paragraph runs at {location}"
        )));
    }
    let original_signatures = original
        .runs
        .iter()
        .map(|run| run_signature_with_options(run, metadata.options))
        .collect::<Vec<_>>();
    let edited_signatures = edited
        .runs
        .iter()
        .map(|run| run_signature_with_options(run, metadata.options))
        .collect::<Vec<_>>();
    let aligned = align(&original_signatures, &edited_signatures);
    if !metadata.options.ignore_fields {
        validate_field_alignment(
            &aligned,
            &original.runs,
            &edited.runs,
            &original_signatures,
            &edited_signatures,
            location,
        )?;
    }
    // One read of the paragraph answers the scope at every run edge.
    let parents = ParentScopes::new(
        &source,
        std::iter::once(0)
            .chain(run_spans.iter().flat_map(|span| [span.start, span.end]))
            .collect(),
    );
    let mut output = String::new();
    let mut cursor = 0usize;
    for (left, right) in aligned {
        let at = left.map_or(cursor, |index| run_spans[index].start);
        let destination_parent = parents.at(at)?;
        let revision_parent = metadata.ids.revision_parent_scope(&destination_parent);
        if let Some(index) = left {
            let span = &run_spans[index];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
        }
        match (left, right) {
            (Some(i), Some(j)) if original_signatures[i] == edited_signatures[j] => {
                if !run_is_field(&original.runs[i]) && original.runs[i] == edited.runs[j] {
                    output.push_str(&source[run_spans[i].clone()]);
                } else {
                    output.push_str(&compared_run_xml(
                        &original.runs[i],
                        &edited.runs[j],
                        None,
                        None,
                        &format!("{location}/run[{j}]"),
                        &destination_parent,
                        metadata,
                        diagnostics,
                    )?);
                }
            }
            (Some(i), Some(j)) => {
                output.push_str(&paired_run_replacement(
                    [&original.runs[i], &edited.runs[j]],
                    [None, None],
                    [None, None],
                    &format!("{location}/run[{j}]"),
                    &destination_parent,
                    metadata,
                    diagnostics,
                )?);
            }
            (Some(i), None) => {
                if run_is_ignored(&original.runs[i], metadata.options) {
                    output.push_str(&source[run_spans[i].clone()]);
                    continue;
                }
                let deleted = publish_run_source(
                    &original.runs[i],
                    &original.runs[i],
                    deleted_run_xml(&original.runs[i])?,
                    None,
                    None,
                    None,
                    &revision_parent,
                    metadata,
                )?;
                output.push_str(&metadata.ids.revision(
                    "del",
                    metadata.author,
                    metadata.timestamp,
                    &deleted,
                )?);
            }
            (None, Some(j)) => {
                if run_is_ignored(&edited.runs[j], metadata.options) {
                    continue;
                }
                let inserted = publish_run_source(
                    &edited.runs[j],
                    &edited.runs[j],
                    paragraph_owned_run_xml(&edited.runs[j])?,
                    None,
                    None,
                    None,
                    &revision_parent,
                    metadata,
                )?;
                output.push_str(&metadata.ids.revision(
                    "ins",
                    metadata.author,
                    metadata.timestamp,
                    &inserted,
                )?);
            }
            (None, None) => unreachable!(),
        }
    }
    output.push_str(&source[cursor..]);
    let spans = direct_word_element_spans(&output, "sdt")?;
    if spans.len() != original.content_controls.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate paragraph content controls at {location}"
        )));
    }
    let mut replacements = Vec::with_capacity(spans.len());
    let mut sources = source_children(&output, &spans).into_iter();
    for (index, ((_, _, _, left), (_, _, _, right))) in original
        .content_controls
        .iter()
        .zip(&edited.content_controls)
        .enumerate()
    {
        let source = sources.next().expect("one source per control span");
        replacements.push(compare_control_from_xml(
            left,
            right,
            &format!("{location}/content-control[{index}]"),
            metadata,
            diagnostics,
            &source?,
        )?);
    }
    replace_direct_word_elements(&output, "sdt", &replacements)
}

fn paragraph_control_boundaries(paragraph: &CT_P) -> Vec<(usize, usize, usize)> {
    paragraph
        .content_controls
        .iter()
        .map(|(at, raw_before, markers_before, _)| (*at, *raw_before, *markers_before))
        .collect()
}

/// Whether two paragraphs differ in an inline structure that revising the
/// paragraph in place cannot express: a hyperlink, comment range, bookmark,
/// preserved raw child or inline content control.
///
/// The word and character paths follow a hyperlink or control through the
/// text alignment, so only the shells and their slots count there.
/// Boundary coordinates in the comparison-only plain-run view. Source owners
/// remain untouched, and every structural shell still terminates a group.
fn coalesced_run_boundaries(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<usize> {
    let cuts = shell_run_boundaries(paragraph, options)
        .into_iter()
        .collect::<HashSet<_>>();
    let mut boundaries = vec![0];
    for (index, run) in paragraph.runs.iter().enumerate() {
        let joins = index > 0
            && !cuts.contains(&index)
            && coalescible_plain_run(run)
            && coalescible_plain_run(&paragraph.runs[index - 1])
            && run_formatting(run) == run_formatting(&paragraph.runs[index - 1]);
        boundaries.push(boundaries[index] + usize::from(!joins));
    }
    boundaries
}

fn paragraph_boundaries_differ(
    original: &CT_P,
    edited: &CT_P,
    options: &ComparisonOptions,
) -> bool {
    let original_boundaries = coalesced_run_boundaries(original, options);
    let edited_boundaries = coalesced_run_boundaries(edited, options);
    let comments = |paragraph: &CT_P, boundaries: &[usize]| {
        paragraph
            .comment_ranges
            .iter()
            .cloned()
            .map(|mut marker| {
                let (CommentRangeMarker::Start { run_index, .. }
                | CommentRangeMarker::End { run_index, .. }) = &mut marker;
                *run_index = boundaries.get(*run_index).copied().unwrap_or(*run_index);
                marker
            })
            .collect::<Vec<_>>()
    };
    let bookmarks = |paragraph: &CT_P, boundaries: &[usize]| {
        paragraph
            .current_bookmark_projection()
            .iter()
            .map(|marker| {
                (
                    marker.is_start(),
                    marker.id(),
                    marker.name().map(str::to_owned),
                    marker.raw_before(),
                    marker.has_child_content(),
                    boundaries
                        .get(marker.run_index())
                        .copied()
                        .unwrap_or(marker.run_index()),
                    boundaries
                        .get(marker.projected_run_index())
                        .copied()
                        .unwrap_or(marker.projected_run_index()),
                    boundaries
                        .get(marker.tracked_run_index())
                        .copied()
                        .unwrap_or(marker.tracked_run_index()),
                )
            })
            .collect::<Vec<_>>()
    };
    if (!options.ignore_comments
        && comments(original, &original_boundaries) != comments(edited, &edited_boundaries))
        || bookmarks(original, &original_boundaries) != bookmarks(edited, &edited_boundaries)
    {
        return true;
    }
    if uses_attributed_run_path(options) {
        let control_slots = |paragraph: &CT_P| {
            paragraph
                .content_controls
                .iter()
                .map(|(_, raw_before, markers_before, _)| (*raw_before, *markers_before))
                .collect::<Vec<_>>()
        };
        return hyperlink_shells(original, options) != hyperlink_shells(edited, options)
            || control_slots(original) != control_slots(edited);
    }
    original.hyperlinks != edited.hyperlinks
        || original
            .extra_xml
            .iter()
            .filter(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
            .ne(edited
                .extra_xml
                .iter()
                .filter(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw)))
        || paragraph_control_boundaries(original) != paragraph_control_boundaries(edited)
}

fn paragraph_properties_xml(
    original: &CT_P,
    edited: &CT_P,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let source = metadata
        .paragraph_source(original)
        .map(|xml| direct_property(xml, "pPr"))
        .transpose()?
        .flatten();
    let generated =
        paragraph_properties_xml_impl(original, edited, location, metadata, diagnostics)?;
    let before = original
        .properties
        .as_ref()
        .map(property_xml)
        .transpose()?
        .unwrap_or_default();
    let output = patch_property_source(&before, &generated, source.as_deref(), &metadata.ids)?;
    let output = if let Some(source) = source {
        preserve_prior_property_sources(&output, &source)?
    } else {
        metadata.ids.generated(output)?
    };
    if let Some(paragraph) = metadata.paragraph_source(original)
        && let Some(span) = direct_word_element_spans(paragraph, "pPr")?.first()
    {
        let context = root_namespace_bindings(paragraph, "p", &[])?;
        restore_current_property_roots(&output, &paragraph[span.clone()], &context)
    } else {
        Ok(output)
    }
}

fn paragraph_properties_xml_impl(
    original: &CT_P,
    edited: &CT_P,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let original_section = original
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref());
    let edited_section = edited
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref());
    let tracked_section = section_properties_xml(
        original_section,
        edited_section,
        &format!("{location}/section"),
        metadata,
        diagnostics,
    )?;
    if metadata.options.ignore_formatting {
        return original
            .properties
            .as_ref()
            .map(property_xml)
            .transpose()
            .map(Option::unwrap_or_default);
    }

    let original_modeled = modeled_paragraph_properties(original.properties.as_ref());
    let edited_modeled = modeled_paragraph_properties(edited.properties.as_ref());
    let changed = original_modeled != edited_modeled;
    let mut current = if changed {
        edited.properties.clone().unwrap_or_default()
    } else {
        original.properties.clone().unwrap_or_default()
    };
    current.sect_pr = None;
    current.change = None;
    let (
        original_numbering_xml,
        original_numbering_positions,
        original_numbering_position,
        original_revision_xml,
        original_revision_positions,
    ) = original
        .properties
        .as_ref()
        .map(|properties| {
            (
                properties.numbering_revision_xml.clone(),
                properties.numbering_revision_xml_positions.clone(),
                properties.numbering_revision_position,
                properties.revision_xml.clone(),
                properties.revision_xml_positions.clone(),
            )
        })
        .unwrap_or_default();
    let edited_properties = edited.properties.as_ref().cloned().unwrap_or_default();
    if edited_properties.numbering_revision_xml != original_numbering_xml
        || edited_properties.numbering_revision_xml_positions != original_numbering_positions
        || edited_properties.numbering_revision_position != original_numbering_position
        || edited_properties.revision_xml != original_revision_xml
        || edited_properties.revision_xml_positions != original_revision_positions
    {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    current.numbering_revision_xml = original_numbering_xml;
    current.numbering_revision_xml_positions = original_numbering_positions;
    current.numbering_revision_position = original_numbering_position;
    current.revision_xml = original_revision_xml;
    current.revision_xml_positions = original_revision_positions;
    let needs_owner = changed || !tracked_section.is_empty() || original.properties.is_some();
    if !needs_owner {
        return Ok(String::new());
    }
    let mut current_xml = property_xml(&current)?;
    if current_xml.is_empty() {
        current_xml.push_str("<w:pPr></w:pPr>");
    }
    // `w:pPrChange` holds only the base properties, so the mark records its
    // own change in a `w:rPrChange`, as Word writes it.
    let (original_base, original_mark) = split_paragraph_mark(original_modeled);
    let (edited_base, edited_mark) = split_paragraph_mark(edited_modeled);
    if original_mark != edited_mark {
        let previous = run_property_xml(original_mark.as_ref())?;
        let change =
            metadata
                .ids
                .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
        if let Some(span) = direct_word_element_spans(&current_xml, "rPr")?
            .into_iter()
            .next()
        {
            let updated = append_word_child(&current_xml[span.clone()], "rPr", &change)?;
            current_xml.replace_range(span, &updated);
        } else {
            current_xml = inject_before_close(
                &current_xml,
                "</w:pPr>",
                &format!("<w:rPr>{change}</w:rPr>"),
            )?;
        }
    }
    if !tracked_section.is_empty() {
        current_xml = inject_before_close(&current_xml, "</w:pPr>", &tracked_section)?;
    }
    if original_base != edited_base {
        let previous = original_base
            .as_ref()
            .map(property_xml)
            .transpose()?
            .filter(|xml| !xml.is_empty())
            .unwrap_or_else(|| "<w:pPr/>".to_owned());
        let change =
            metadata
                .ids
                .revision("pPrChange", metadata.author, metadata.timestamp, &previous)?;
        current_xml = inject_before_close(&current_xml, "</w:pPr>", &change)?;
    }
    metadata.ids.generated(current_xml)
}

/// Modeled paragraph properties without the paragraph mark, and the mark.
fn split_paragraph_mark(
    properties: Option<CT_PPr>,
) -> (
    Option<CT_PPr>,
    Option<oxoffice_docx_oxml::properties::CT_RPr>,
) {
    match properties {
        Some(mut properties) => {
            let mark = properties.rpr.take();
            (nonempty_paragraph_properties(properties), mark)
        }
        None => (None, None),
    }
}

/// Whether `source` is one element without children or attributes, namespace
/// declarations aside, such as LibreOffice's `<w:rPr></w:rPr>`.
fn bare_empty_element(source: &str) -> bool {
    let bare = |element: &BytesStart<'_>| {
        element.attributes().flatten().all(|attribute| {
            attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:")
        })
    };
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(true);
    match reader.read_event() {
        Ok(Event::Empty(element)) => bare(&element),
        Ok(Event::Start(element)) if bare(&element) => {
            matches!(reader.read_event(), Ok(Event::End(_)))
        }
        _ => false,
    }
}

/// Paragraph properties without an empty mark `w:rPr`, which LibreOffice
/// writes `<w:rPr></w:rPr>` and the model writes back as nothing, and none
/// when nothing else is left.
fn without_empty_mark(xml: Option<&str>) -> Result<Option<String>> {
    let Some(xml) = xml else {
        return Ok(None);
    };
    let mut stripped = xml.to_owned();
    for span in direct_word_element_spans(xml, "rPr")?.into_iter().rev() {
        if bare_empty_element(&xml[span.clone()]) {
            stripped.replace_range(span, "");
        }
    }
    Ok((!bare_empty_element(&stripped)).then_some(stripped))
}

/// The `w:pPr` of a [`final_paragraph`]: the `own` properties when both
/// resolutions agree, otherwise the `accepted` ones with the section break of
/// `own`, a `w:pPrChange` holding the rejected paragraph properties, and a
/// mark `w:rPrChange` holding the rejected mark formatting.
///
/// Rejecting a `w:pPrChange` keeps a mark `w:rPr` only when it holds a
/// revision, so a mark with formatting also carries the `w:rPrChange` when
/// only the paragraph properties differ.
fn final_paragraph_properties_xml(
    own: &PropertySide<'_>,
    accepted: &PropertySide<'_>,
    rejected: &PropertySide<'_>,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let accepted_modeled = modeled_paragraph_properties(accepted.typed);
    let rejected_modeled = modeled_paragraph_properties(rejected.typed);
    if metadata.options.ignore_formatting
        || (accepted_modeled == rejected_modeled
            && without_empty_mark(accepted.xml.as_deref())?
                == without_empty_mark(rejected.xml.as_deref())?)
    {
        return Ok(own.xml.clone().unwrap_or_default());
    }
    let mut current = accepted
        .xml
        .clone()
        .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
    let own_section = own
        .xml
        .as_deref()
        .map(|xml| direct_property(xml, "sectPr"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    current = replace_property_slot(&current, "sectPr", &own_section)?;
    // An empty mark carries no format, written either way.
    let mark = |side: &PropertySide<'_>| -> Result<Option<String>> {
        Ok(side
            .xml
            .as_deref()
            .map(|xml| direct_property(xml, "rPr"))
            .transpose()?
            .flatten()
            .filter(|mark| !bare_empty_element(mark)))
    };
    let accepted_mark = mark(accepted)?;
    let rejected_mark = mark(rejected)?;
    if accepted_mark != rejected_mark {
        let previous = rejected_mark
            .clone()
            .unwrap_or(metadata.ids.generated("<w:rPr/>".to_owned())?);
        let change =
            metadata
                .ids
                .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
        let mark = accepted_mark
            .clone()
            .unwrap_or(metadata.ids.generated("<w:rPr/>".to_owned())?);
        let updated = append_word_child(&mark, "rPr", &change)?;
        current = replace_property_slot(&current, "rPr", &updated)?;
    }
    let base = |side: &PropertySide<'_>| -> Result<Option<String>> {
        side.xml
            .as_deref()
            .map(|xml| without_property_children(xml, &["rPr", "sectPr", "pPrChange"]))
            .transpose()
    };
    let accepted_base = base(accepted)?;
    let rejected_base = base(rejected)?;
    if accepted_base != rejected_base {
        let previous = rejected_base.unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
        let change =
            metadata
                .ids
                .revision("pPrChange", metadata.author, metadata.timestamp, &previous)?;
        current = append_word_child(&current, "pPr", &change)?;
    }
    Ok(current)
}

fn modeled_paragraph_properties(properties: Option<&CT_PPr>) -> Option<CT_PPr> {
    properties.cloned().and_then(|mut properties| {
        properties.sect_pr = None;
        properties.num_ilvl_raw = None;
        properties.num_id_raw = None;
        properties.numbering_revision = None;
        properties.numbering_revision_xml.clear();
        properties.numbering_revision_xml_positions.clear();
        properties.numbering_revision_position = None;
        properties.change = None;
        properties.revision_xml.clear();
        properties.revision_xml_positions.clear();
        nonempty_paragraph_properties(properties)
    })
}

fn nonempty_paragraph_properties(mut properties: CT_PPr) -> Option<CT_PPr> {
    if properties
        .rpr
        .as_ref()
        .is_some_and(|mark| *mark == oxoffice_docx_oxml::properties::CT_RPr::default())
    {
        properties.rpr = None;
    }
    (properties != CT_PPr::default()).then_some(properties)
}

fn paragraph_properties_differ(original: Option<&CT_PPr>, edited: Option<&CT_PPr>) -> bool {
    let modeled = |properties: Option<&CT_PPr>| {
        properties.cloned().and_then(|mut properties| {
            if let Some(section) = properties.sect_pr.as_mut() {
                clear_default_orientation(section);
            }
            nonempty_paragraph_properties(properties)
        })
    };
    modeled(original) != modeled(edited)
}

/// Portrait is the schema default, which the section writer omits.
fn clear_default_orientation(section: &mut oxoffice_docx_oxml::document::CT_SectPr) {
    if section.orientation == Some(ST_PageOrientation::Portrait) {
        section.orientation = None;
    }
}

fn section_properties_xml(
    original: Option<&oxoffice_docx_oxml::document::CT_SectPr>,
    edited: Option<&oxoffice_docx_oxml::document::CT_SectPr>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let (Some(original), Some(edited)) = (original, edited) else {
        return match (original, edited) {
            (None, None) => Ok(String::new()),
            _ => Err(Error::Other(format!(
                "comparison cannot create or remove a section shell at {location}"
            ))),
        };
    };
    if original.header_refs != edited.header_refs || original.footer_refs != edited.footer_refs {
        return Err(Error::Other(format!(
            "comparison cannot change related-story references at {location}"
        )));
    }
    if metadata.options.ignore_formatting {
        return section_property_xml(original);
    }
    let mut original_modeled = original.clone();
    original_modeled.change = None;
    let mut edited_modeled = edited.clone();
    edited_modeled.change = None;
    clear_default_orientation(&mut original_modeled);
    clear_default_orientation(&mut edited_modeled);
    // The prefixes in scope where a page-number restart was read, which a
    // save declares differently, are not the section's.
    let read_context_free = |section: &oxoffice_docx_oxml::document::CT_SectPr| {
        let mut section = section.clone();
        if let Some(page_number) = &mut section.page_number {
            page_number.reserved_insertion_prefixes.clear();
        }
        section
    };
    if original_modeled == edited_modeled
        || read_context_free(&original_modeled) == read_context_free(&edited_modeled)
    {
        return section_property_xml(original);
    }
    if original.extra_xml != edited.extra_xml {
        formatting_diagnostic(diagnostics, location.to_owned());
        edited_modeled.extra_xml = original.extra_xml.clone();
    }
    let previous = section_property_xml(&original_modeled)?;
    let change = metadata.ids.revision(
        "sectPrChange",
        metadata.author,
        metadata.timestamp,
        &previous,
    )?;
    let current = section_property_xml(&edited_modeled)?;
    metadata
        .ids
        .generated(inject_before_close(&current, "</w:sectPr>", &change)?)
}

fn deleted_paragraph(paragraph: &CT_P, metadata: &mut Metadata<'_>) -> Result<String> {
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&paragraph_mark_properties(paragraph, "del", metadata)?);
    let children = paragraph_children_xml(paragraph, true, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        true,
        false,
        &mut |content| {
            metadata
                .ids
                .revision("del", metadata.author, metadata.timestamp, content)
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

fn inserted_paragraph(paragraph: &CT_P, metadata: &mut Metadata<'_>) -> Result<String> {
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&paragraph_mark_properties(paragraph, "ins", metadata)?);
    let children = paragraph_children_xml(paragraph, false, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        false,
        false,
        &mut |content| {
            metadata
                .ids
                .revision("ins", metadata.author, metadata.timestamp, content)
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

/// Stage a whole paragraph without its properties, retaining actual run source
/// and destination namespace custody before the shared revision publisher wraps
/// eligible payloads. Field and hyperlink shells remain that publisher's owners.
fn paragraph_children_xml(
    paragraph: &CT_P,
    deleted: bool,
    metadata: &Metadata<'_>,
) -> Result<String> {
    let mut children = paragraph.clone();
    children.properties = None;
    if deleted {
        children.runs = children.runs.iter().map(deleted_run).collect();
    }
    let mut xml = paragraph_xml(&children)?;
    let spans = modeled_paragraph_run_spans(&children, &xml)?;
    // One read of the paragraph answers the scope of every run.
    let parents = ParentScopes::new(&xml, spans.iter().map(|span| span.start).collect());
    let mut replacements = Vec::new();
    let mut index = 0;
    while index < children.runs.len() {
        if let Some((coverage, _)) = children.field_source_run_fragments_at(index)? {
            index += coverage;
            continue;
        }
        let run = &paragraph.runs[index];
        if let Some(source) = metadata.run_source(run) {
            let span = spans[index].clone();
            replacements.push((span.clone(), {
                let (raw, typed) = complete_run_selection(&paragraph.runs[index]);
                preserve_run_source(
                    &xml[span.clone()],
                    &run_property_xml(run.properties.as_ref())?,
                    [source, source],
                    &children.runs[index],
                    &raw,
                    &typed,
                    &metadata.ids.revision_parent_scope(&parents.at(span.start)?),
                    &metadata.ids,
                )
            }?));
        }
        index += 1;
    }
    for (span, replacement) in replacements.into_iter().rev() {
        xml.replace_range(span, &replacement);
    }
    Ok(xml)
}

/// Expanded Word names in a serialized owner. The writer supplies canonical
/// `w` names, while preserved children can shadow that binding or use aliases.
fn word_element_starts(xml: &str) -> Result<HashSet<usize>> {
    let opening = format!(r#"<scope xmlns:w="{W_NS}">"#);
    let scoped = format!("{opening}{xml}</scope>");
    let mut reader = NsReader::from_reader(scoped.as_bytes());
    let mut starts = HashSet::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        if matches!(event, Event::Start(_) | Event::Empty(_))
            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            && let Some(start) = before.checked_sub(opening.len())
        {
            starts.insert(start);
        }
        if matches!(event, Event::Eof) {
            return Ok(starts);
        }
        buffer.clear();
    }
}

fn paragraph_mark_properties(
    paragraph: &CT_P,
    kind: &str,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let source = PropertySide::of(paragraph, metadata)?;
    let properties = source
        .xml
        .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
    marked_paragraph_properties_xml(&properties, &marker, &metadata.ids)
}

fn compare_table(
    original: &CT_Tbl,
    edited: &CT_Tbl,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if original.grid != edited.grid {
            let deleted = marked_table(original, "del", metadata)?;
            let inserted = marked_table(edited, "ins", metadata)?;
            return Ok(format!("{deleted}{inserted}"));
        }
        if original.extra_xml != edited.extra_xml
            || table_control_boundaries(original) != table_control_boundaries(edited)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise table boundary structures at {location}"
            )));
        }
        let original_signatures = original
            .rows
            .iter()
            .map(|row| row_signature_with_options(row, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited
            .rows
            .iter()
            .map(|row| row_signature_with_options(row, metadata.options))
            .collect::<Vec<_>>();
        let mut source = original_source
            .map(str::to_owned)
            .map_or_else(|| table_xml(original), Ok)?;
        let properties = table_properties_xml(
            original.properties.as_ref(),
            edited.properties.as_ref(),
            location,
            metadata,
            diagnostics,
        )?;
        let template = direct_property(&source, "tblPr")?;
        let before_properties = original
            .properties
            .as_ref()
            .map(table_property_xml)
            .transpose()?
            .unwrap_or_default();
        let properties = patch_property_source(
            &before_properties,
            &properties,
            template.as_deref(),
            &metadata.ids,
        )?;
        let properties = if let Some(template) = template {
            preserve_prior_property_sources(&properties, &template)?
        } else {
            metadata.ids.generated(properties)?
        };
        let property_spans = direct_word_element_spans(&source, "tblPr")?;
        match (property_spans.first(), properties.is_empty()) {
            (Some(span), false) => source.replace_range(span.clone(), &properties),
            (Some(span), true) => source.replace_range(span.clone(), ""),
            (None, false) => {
                source = insert_first_child(&source, &properties)?;
            }
            (None, true) => {}
        }
        let row_spans = direct_word_element_spans(&source, "tr")?;
        if row_spans.len() != original.rows.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate direct table rows at {location}"
            )));
        }
        let aligned = align(&original_signatures, &edited_signatures);
        // One read of the table answers the source of every row.
        let mut row_sources = source_children(&source, &row_spans)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        let mut output = String::new();
        let mut cursor = 0usize;
        for (alignment_index, &(left, right)) in aligned.iter().enumerate() {
            if let Some(index) = left {
                let span = &row_spans[index];
                output.push_str(&source[cursor..span.start]);
                cursor = span.end;
            } else {
                let next_row_start = aligned[alignment_index + 1..]
                    .iter()
                    .find_map(|(next_left, _)| next_left.map(|index| row_spans[index].start))
                    .map_or_else(|| paragraph_close_start(&source), Ok)?;
                output.push_str(&source[cursor..next_row_start]);
                cursor = next_row_start;
            }
            let child_mark = metadata.leaf_publications.len();
            let child_offset = output.len();
            match (left, right) {
                (Some(i), Some(j)) => {
                    output.push_str(&compare_row(
                        &original.rows[i],
                        &edited.rows[j],
                        Some(&row_sources[i].take().expect("a row is compared once")?),
                        &format!("{location}/row[{j}]"),
                        metadata,
                        diagnostics,
                    )?);
                }
                (Some(i), None) => {
                    output.push_str(&marked_row(&original.rows[i], "del", metadata)?)
                }
                (None, Some(j)) => output.push_str(&marked_row(&edited.rows[j], "ins", metadata)?),
                (None, None) => unreachable!(),
            }
            shift_leaf_publications(
                &mut metadata.leaf_publications[child_mark..],
                child_offset as isize,
            )?;
        }
        output.push_str(&source[cursor..]);
        let spans = direct_word_element_spans(&output, "sdt")?;
        if spans.len() != original.content_controls.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate table content controls at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut replacements = Vec::with_capacity(spans.len());
        let mut sources = source_children(&output, &spans).into_iter();
        for (index, ((_, _, left), (_, _, right))) in original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .enumerate()
        {
            let child_mark = metadata.leaf_publications.len();
            let source = sources.next().expect("one source per control span");
            replacements.push(compare_control_from_xml(
                left,
                right,
                &format!("{location}/content-control[{index}]"),
                metadata,
                diagnostics,
                &source?,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(metadata, leaf_mark, &spans, &replacements, child_leaves)?;
        replace_direct_word_elements(&output, "sdt", &replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Tbl as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn table_properties_xml(
    original: Option<&CT_TblPr>,
    edited: Option<&CT_TblPr>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if metadata.options.ignore_formatting {
        return original
            .map(table_property_xml)
            .transpose()
            .map(Option::unwrap_or_default)
            .and_then(|xml| metadata.ids.generated(xml));
    }
    let original_modeled = modeled_table_properties(original);
    let edited_modeled = modeled_table_properties(edited);
    let unmodeled = |properties: &CT_TblPr| {
        (
            properties.extra_xml.clone(),
            properties.revision_xml.clone(),
        )
    };
    if original.map(unmodeled).unwrap_or_default() != edited.map(unmodeled).unwrap_or_default() {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    if original_modeled == edited_modeled {
        return original
            .map(table_property_xml)
            .transpose()
            .map(Option::unwrap_or_default)
            .and_then(|xml| metadata.ids.generated(xml));
    }
    let mut current = edited.cloned().unwrap_or_default();
    current.change = None;
    let (original_extra_xml, original_revision_xml) = original.map(unmodeled).unwrap_or_default();
    current.extra_xml = original_extra_xml;
    current.revision_xml = original_revision_xml;
    let previous = original_modeled
        .as_ref()
        .map(table_property_xml)
        .transpose()?
        .unwrap_or_else(|| "<w:tblPr/>".to_owned());
    let change = metadata.ids.revision(
        "tblPrChange",
        metadata.author,
        metadata.timestamp,
        &previous,
    )?;
    let current = table_property_xml(&current)?;
    metadata
        .ids
        .generated(inject_before_close(&current, "</w:tblPr>", &change)?)
}

fn modeled_table_properties(properties: Option<&CT_TblPr>) -> Option<CT_TblPr> {
    properties.cloned().map(|mut properties| {
        properties.change = None;
        properties.revision_xml.clear();
        properties.extra_xml.clear();
        // An empty border container, as LibreOffice writes it, is no border.
        if properties
            .borders
            .as_ref()
            .is_some_and(oxoffice_docx_oxml::table::CT_TblBorders::is_empty)
        {
            properties.borders = None;
        }
        properties
    })
}

fn table_property_xml(properties: &CT_TblPr) -> Result<String> {
    let mut bytes = Vec::new();
    properties.to_xml(&mut Writer::new(&mut bytes))?;
    String::from_utf8(bytes).map_err(utf8_error)
}

fn table_control_boundaries(table: &CT_Tbl) -> Vec<(usize, usize)> {
    table
        .content_controls
        .iter()
        .map(|(at, raw_before, _)| (*at, *raw_before))
        .collect()
}

fn marked_table(table: &CT_Tbl, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let source = metadata
        .table_source(table)
        .map(str::to_owned)
        .map_or_else(|| table_xml(table), Ok)?;
    let replacements = table
        .rows
        .iter()
        .map(|row| marked_row(row, kind, metadata))
        .collect::<Result<Vec<_>>>()?;
    let output = replace_direct_word_elements(&source, "tr", &replacements)?;
    let control_spans = direct_word_element_spans(&output, "sdt")?;
    if control_spans.len() != table.content_controls.len() {
        return Err(Error::Other(
            "comparison could not correlate table content controls while marking rows".to_owned(),
        ));
    }
    let replacements = table
        .content_controls
        .iter()
        .zip(&control_spans)
        .map(|((_, _, control), span)| {
            mark_control_owned_rows(control, kind, metadata, &output[span.clone()])
        })
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&output, "sdt", &replacements)?)
}

fn mark_control_owned_rows(
    control: &CT_Sdt,
    kind: &str,
    metadata: &mut Metadata<'_>,
    original_xml: &str,
) -> Result<String> {
    let content_span = direct_word_element_spans(original_xml, "sdtContent")?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("marked control has no actual content owner".into()))?;
    let actual_content = source_child(original_xml, content_span)?;
    let destination_parent = root_namespace_bindings(&actual_content, "sdtContent", &[])?;
    let mut content = String::new();
    for child in &control.content {
        content.push_str(&match child {
            SdtContent::Paragraph(paragraph) => metadata
                .paragraph_source(paragraph)
                .map(str::to_owned)
                .map_or_else(
                    || paragraph_xml(paragraph).and_then(|xml| metadata.ids.generated(xml)),
                    Ok,
                )?,
            SdtContent::Table(table) => marked_table(table, kind, metadata)?,
            SdtContent::Row(row) => marked_row(row, kind, metadata)?,
            SdtContent::Cell(cell) => metadata.cell_source(cell).map(str::to_owned).map_or_else(
                || cell_xml(cell).and_then(|xml| metadata.ids.generated(xml)),
                Ok,
            )?,
            SdtContent::Run(run) => publish_run_source(
                run,
                run,
                paragraph_owned_run_xml(run)?,
                None,
                None,
                None,
                &destination_parent,
                metadata,
            )?,
            SdtContent::ContentControl(nested) => {
                let nested_xml = metadata
                    .control_source(nested)
                    .map(str::to_owned)
                    .map_or_else(|| control_xml(nested), Ok)?;
                mark_control_owned_rows(nested, kind, metadata, &nested_xml)?
            }
            SdtContent::RawXml(raw) => String::from_utf8(raw.clone()).map_err(utf8_error)?,
        });
    }
    metadata.ids.generated(replace_element_inner(
        original_xml,
        "w:sdtContent",
        &content,
    )?)
}

fn marked_row(row: &CT_Row, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let source = metadata
        .row_source(row)
        .map(str::to_owned)
        .map_or_else(|| row_xml(row), Ok)?;
    marked_row_xml(&source, kind, metadata)
}

/// Mark a serialized row as Word marks an inserted or deleted row: the row
/// marker in `w:trPr`, then every cell paragraph mark and every cell run,
/// nested tables included. Without the cell marks Word merges an adjacent
/// removed paragraph mark into the first cell and leaves the row unresolved.
fn marked_row_xml(row: &str, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let mut xml = row.to_owned();
    let properties = direct_word_element_spans(&xml, "trPr")?;
    if let Some(properties_span) = properties.first() {
        let updated = append_word_child(&xml[properties_span.clone()], "trPr", &marker)?;
        xml.replace_range(properties_span.clone(), &updated);
    } else {
        // `w:trPr` follows an optional `w:tblPrEx`.
        let properties = metadata
            .ids
            .generated(format!("<w:trPr>{marker}</w:trPr>"))?;
        if let Some(exception) = direct_word_element_spans(&xml, "tblPrEx")?.first() {
            xml.insert_str(exception.end, &properties);
        } else {
            xml = insert_first_child(&xml, &properties)?;
        }
    }
    let cell_spans = direct_word_element_spans(&xml, "tc")?;
    let cells = source_children(&xml, &cell_spans)
        .into_iter()
        .map(|source| marked_cell_xml(&source?, kind, metadata))
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&xml, "tc", &cells)?)
}

fn marked_cell_xml(cell: &str, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let paragraph_spans = direct_word_element_spans(cell, "p")?;
    let paragraphs = source_children(cell, &paragraph_spans)
        .into_iter()
        .map(|source| {
            let marker = metadata
                .ids
                .marker(kind, metadata.author, metadata.timestamp)?;
            let source = source?;
            let paragraph = marked_paragraph_xml(&source, &marker, &metadata.ids)?;
            let paragraph = marked_direct_runs_xml(&paragraph, kind, metadata)?;
            let hyperlink_spans = direct_word_element_spans(&paragraph, "hyperlink")?;
            let hyperlinks = hyperlink_spans
                .iter()
                .cloned()
                .zip(source_children(&paragraph, &hyperlink_spans))
                .map(|(span, scoped)| {
                    let scoped = scoped?;
                    let marked = marked_direct_runs_xml(&scoped, kind, metadata)?;
                    let destination = root_namespace_bindings(&paragraph, "p", &[])?;
                    retain_source_root_namespaces(&marked, &paragraph[span], &destination)
                })
                .collect::<Result<Vec<_>>>()?;
            replace_direct_word_elements(&paragraph, "hyperlink", &hyperlinks)
        })
        .collect::<Result<Vec<_>>>()?;
    let cell = replace_direct_word_elements(cell, "p", &paragraphs)?;
    let table_spans = direct_word_element_spans(&cell, "tbl")?;
    let tables = source_children(&cell, &table_spans)
        .into_iter()
        .map(|scoped_table| {
            let scoped_table = scoped_table?;
            let table = scoped_table.as_str();
            let row_spans = direct_word_element_spans(table, "tr")?;
            let rows = source_children(table, &row_spans)
                .into_iter()
                .map(|source| marked_row_xml(&source?, kind, metadata))
                .collect::<Result<Vec<_>>>()?;
            replace_direct_word_elements(table, "tr", &rows)
        })
        .collect::<Result<Vec<_>>>()?;
    metadata
        .ids
        .generated(replace_direct_word_elements(&cell, "tbl", &tables)?)
}

/// Wrap every direct `w:r` of `owner` in a revision, with deleted text as
/// `w:delText`.
fn marked_direct_runs_xml(owner: &str, kind: &str, metadata: &mut Metadata<'_>) -> Result<String> {
    let spans = direct_word_element_spans(owner, "r")?
        .into_iter()
        .map(|span| (None, span))
        .collect::<Vec<_>>();
    let runs = RunSource::capture_all(&spans, owner)
        .into_iter()
        .map(|source| {
            let source = source?;
            let mut run = source.scoped.clone();
            if kind == "del" {
                for (local, deleted_local) in [("t", "delText"), ("instrText", "delInstrText")] {
                    for text in direct_word_element_spans(&run, local)?.into_iter().rev() {
                        let raw = &run[text.clone()];
                        let (_, empty, name) = element_opening(raw)?;
                        let prefix = name
                            .rsplit_once(':')
                            .map(|(prefix, _)| format!("{prefix}:"))
                            .unwrap_or_default();
                        let mut deleted = raw.to_owned();
                        if !empty {
                            let close = deleted
                                .rfind(&format!("</{name}>"))
                                .ok_or_else(|| Error::Other("marked text has no end".into()))?;
                            deleted.replace_range(
                                close + 2..close + 2 + name.len(),
                                &format!("{prefix}{deleted_local}"),
                            );
                        }
                        deleted
                            .replace_range(1..1 + name.len(), &format!("{prefix}{deleted_local}"));
                        run.replace_range(text, &deleted);
                    }
                }
            }
            let revision_parent = metadata.ids.revision_parent_scope(&source.parent);
            run = retain_source_root_namespaces(&run, &source.raw, &revision_parent)?;
            metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, &run)
        })
        .collect::<Result<Vec<_>>>()?;
    replace_direct_word_elements(owner, "r", &runs)
}

fn compare_row(
    original: &CT_Row,
    edited: &CT_Row,
    original_source: Option<&str>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if original.cells.len() != edited.cells.len()
            || row_raw_children(original) != row_raw_children(edited)
            || row_control_boundaries(original) != row_control_boundaries(edited)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise row boundary structures at {location}"
            )));
        }
        if !metadata.options.ignore_formatting
            && row_formatting(original)? != row_formatting(edited)?
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        let mut output = original_source
            .map(str::to_owned)
            .map_or_else(|| row_xml(original), Ok)?;
        let cell_spans = direct_word_element_spans(&output, "tc")?;
        if cell_spans.len() != original.cells.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate row cells at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut cell_replacements = Vec::with_capacity(cell_spans.len());
        let mut sources = source_children(&output, &cell_spans).into_iter();
        for (index, (left, right)) in original.cells.iter().zip(&edited.cells).enumerate() {
            let child_mark = metadata.leaf_publications.len();
            let source = sources.next().expect("one source per cell span");
            cell_replacements.push(compare_cell_from_xml(
                left,
                right,
                &format!("{location}/cell[{index}]"),
                metadata,
                diagnostics,
                &source?,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(
            metadata,
            leaf_mark,
            &cell_spans,
            &cell_replacements,
            child_leaves,
        )?;
        output = replace_direct_word_elements(&output, "tc", &cell_replacements)?;

        let control_spans = direct_word_element_spans(&output, "sdt")?;
        if control_spans.len() != original.content_controls.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate row content controls at {location}"
            )));
        }
        let mut child_leaves = Vec::new();
        let mut control_replacements = Vec::with_capacity(control_spans.len());
        let mut sources = source_children(&output, &control_spans).into_iter();
        for (index, ((_, _, left), (_, _, right))) in original
            .content_controls
            .iter()
            .zip(&edited.content_controls)
            .enumerate()
        {
            let child_mark = metadata.leaf_publications.len();
            let source = sources.next().expect("one source per control span");
            control_replacements.push(compare_control_from_xml(
                left,
                right,
                &format!("{location}/content-control[{index}]"),
                metadata,
                diagnostics,
                &source?,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(
            metadata,
            leaf_mark,
            &control_spans,
            &control_replacements,
            child_leaves,
        )?;
        replace_direct_word_elements(&output, "sdt", &control_replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Row as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

/// Return the raw children of a row without the record of its start-tag
/// attributes, which carry producer identities and no content.
fn row_raw_children(row: &CT_Row) -> Vec<&(usize, Vec<u8>)> {
    row.extra_xml
        .iter()
        .filter(|(position, raw)| !CT_Row::raw_is_root_attributes(*position, raw))
        .collect()
}

fn row_control_boundaries(row: &CT_Row) -> Vec<(usize, usize)> {
    row.content_controls
        .iter()
        .map(|(at, raw_before, _)| (*at, *raw_before))
        .collect()
}

fn compare_cell_from_xml(
    original: &CT_Tc,
    edited: &CT_Tc,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        // Children compared index by index unless a pair there would refuse:
        // the counts differ, the kinds differ, or a paragraph pair needs a
        // whole replacement that the pair alone cannot hold.
        let last = original.content.len().saturating_sub(1);
        let aligned_children = original.extra_xml.is_empty()
            && edited.extra_xml.is_empty()
            && (original.content.len() != edited.content.len()
                || original
                    .content
                    .iter()
                    .zip(&edited.content)
                    .enumerate()
                    .any(|(index, pair)| match pair {
                        (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
                            paragraph_boundaries_differ(left, right, metadata.options)
                                && (left.bookmark_markers != right.bookmark_markers
                                    || index == last)
                        }
                        (CellContent::Table(_), CellContent::Table(_))
                        | (CellContent::ContentControl(_), CellContent::ContentControl(_)) => false,
                        _ => true,
                    }));
        if !aligned_children
            && (original.content.len() != edited.content.len()
                || original.extra_xml != edited.extra_xml)
        {
            return Err(Error::Other(format!(
                "comparison cannot revise cell boundary structures at {location}"
            )));
        }
        if !metadata.options.ignore_formatting
            && cell_formatting(original)? != cell_formatting(edited)?
        {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        if aligned_children {
            return compare_aligned_cell_children(
                original,
                edited,
                location,
                metadata,
                diagnostics,
                original_xml,
            );
        }
        let children = cell_child_spans(original, original_xml, location)?;
        let mut child_leaves = Vec::new();
        let mut replacements = Vec::with_capacity(children.len());
        let child_spans = children
            .iter()
            .map(|(_, span)| span.clone())
            .collect::<Vec<_>>();
        let mut sources = source_children(original_xml, &child_spans).into_iter();
        for (index, ((left, right), _)) in original
            .content
            .iter()
            .zip(&edited.content)
            .zip(&children)
            .enumerate()
        {
            let child_location = format!("{location}/content[{index}]");
            let source = sources.next().expect("one source per cell child span")?;
            let source = source.as_str();
            let child_mark = metadata.leaf_publications.len();
            replacements.push(compare_cell_child(
                left,
                right,
                source,
                &child_location,
                metadata,
                diagnostics,
            )?);
            child_leaves.push(metadata.leaf_publications.split_off(child_mark));
        }
        splice_leaf_publications(
            metadata,
            leaf_mark,
            &children
                .iter()
                .map(|(_, span)| span.clone())
                .collect::<Vec<_>>(),
            &replacements,
            child_leaves,
        )?;
        replace_ranges(original_xml, &children, &replacements)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Tc as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn compare_cell_child(
    original: &CellContent,
    edited: &CellContent,
    source: &str,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (CellContent::Paragraph(left), CellContent::Paragraph(right)) => {
            compare_paragraph(left, right, Some(source), location, metadata, diagnostics)
        }
        (CellContent::Table(left), CellContent::Table(right)) => {
            compare_table(left, right, Some(source), location, metadata, diagnostics)
        }
        (CellContent::ContentControl(left), CellContent::ContentControl(right)) => {
            compare_control_from_xml(left, right, location, metadata, diagnostics, source)
        }
        _ => Err(Error::Other(format!(
            "comparison cannot revise incompatible cell children at {location}"
        ))),
    }
}

/// Compare a cell whose number of children changed, aligning them as a block
/// content control aligns its own. A paragraph that one side inserts,
/// deletes, merges into another or splits off becomes a whole tracked
/// paragraph with its mark, and a pair whose inline boundaries differ is
/// replaced by the run of changed paragraphs around it. The last paragraph
/// mark stays the cell terminator: a final inserted or deleted paragraph
/// marks the paragraph mark before it instead, as [`final_paragraph`] does
/// in a story.
fn compare_aligned_cell_children(
    original: &CT_Tc,
    edited: &CT_Tc,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let options = metadata.options;
    let signatures = |content: &[CellContent]| {
        content
            .iter()
            .map(|child| match child {
                CellContent::Paragraph(paragraph) => {
                    format!("p:{}", paragraph_signature_with_options(paragraph, options))
                }
                CellContent::Table(table) => {
                    format!("t:{}", table_signature_with_options(table, options))
                }
                CellContent::ContentControl(control) => {
                    format!("s:{}", control_signature_with_options(control, options))
                }
            })
            .collect::<Vec<_>>()
    };
    fn paragraphs(content: &[CellContent]) -> Vec<Option<&CT_P>> {
        content
            .iter()
            .map(|child| match child {
                CellContent::Paragraph(paragraph) => Some(paragraph),
                _ => None,
            })
            .collect()
    }
    let original_signatures = signatures(&original.content);
    let edited_signatures = signatures(&edited.content);
    let original_paragraphs = paragraphs(&original.content);
    let edited_paragraphs = paragraphs(&edited.content);
    let (aligned, carried) = replace_changed_paragraph_runs(
        pair_gaps_by_kind(
            align(&original_signatures, &edited_signatures),
            &original_signatures,
            &edited_signatures,
        ),
        &original_paragraphs,
        &edited_paragraphs,
        &original_signatures,
        &edited_signatures,
        options,
    )?;
    refuse_uncarried_field_owners(
        &aligned,
        &original_paragraphs,
        &edited_paragraphs,
        &carried,
        location,
    )?;
    let is_paragraph =
        |child: Option<&CellContent>| matches!(child, Some(CellContent::Paragraph(_)));
    let trailing_paragraph_insert_start = aligned
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, (left, right))| {
            left.is_none() && is_paragraph(right.and_then(|index| edited.content.get(index)))
        })
        .map(|(position, _)| position)
        .last();
    let trailing_paragraph_delete_start = aligned
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, (left, right))| {
            right.is_none() && is_paragraph(left.and_then(|index| original.content.get(index)))
        })
        .map(|(position, _)| position)
        .last();
    let children = cell_child_spans(original, original_xml, location)?;
    let (Some((_, first)), Some((_, last))) = (children.first(), children.last()) else {
        return Err(Error::Other(format!(
            "comparison cannot revise cell boundary structures at {location}"
        )));
    };
    let (first, last) = (first.clone(), last.clone());
    // Source text between two children, which only whitespace can be when the
    // cell holds no unmodeled child. It stays before the original child it
    // preceded.
    let mut whitespace_slots = vec![String::new(); children.len()];
    for (index, pair) in children.windows(2).enumerate() {
        let between = &original_xml[pair[0].1.end..pair[1].1.start];
        if !between.bytes().all(|byte| byte.is_ascii_whitespace()) {
            return Err(Error::Other(format!(
                "comparison cannot revise cell boundary structures at {location}"
            )));
        }
        whitespace_slots[index + 1] = between.to_owned();
    }
    let child_spans = children
        .iter()
        .map(|(_, span)| span.clone())
        .collect::<Vec<_>>();
    let sources = source_children(original_xml, &child_spans)
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let bindings = root_namespace_bindings(original_xml, "tc", &[])?;
    let mut content: Vec<(EmittedOwner, String)> = Vec::new();
    let mut content_leaves: Vec<Vec<LeafPublication>> = Vec::new();
    let mut trailing_merged = PropertySide::default();
    let mut whitespace_emitted = vec![false; whitespace_slots.len()];
    for (position, (left, right)) in aligned.iter().copied().enumerate() {
        let whitespace_boundary = left.or_else(|| {
            let right = right?;
            let (Some(next_left), None) = aligned.get(position + 1).copied()? else {
                return None;
            };
            (matches!(original.content[next_left], CellContent::Paragraph(_))
                && matches!(edited.content[right], CellContent::Table(_)))
            .then_some(next_left)
        });
        if let Some(index) = whitespace_boundary
            && !whitespace_emitted[index]
            && !whitespace_slots[index].is_empty()
        {
            content.push((EmittedOwner::Whitespace, whitespace_slots[index].clone()));
            content_leaves.push(Vec::new());
            whitespace_emitted[index] = true;
        }
        let item = |(left, right): (Option<usize>, Option<usize>)| {
            right
                .and_then(|index| edited.content.get(index))
                .or_else(|| left.and_then(|index| original.content.get(index)))
        };
        let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
            matches!(item(pair), Some(CellContent::Table(_)))
        })
        .is_some_and(|pair| {
            matches!(
                item(pair),
                Some(CellContent::Paragraph(_) | CellContent::Table(_))
            )
        });
        let child_mark = metadata.leaf_publications.len();
        let child_location = format!("{location}/content[{position}]");
        match (left, right) {
            (Some(i), Some(j)) => content.push((
                EmittedOwner::of_cell(&original.content[i], &edited.content[j], None, metadata)?,
                compare_cell_child(
                    &original.content[i],
                    &edited.content[j],
                    &sources[i],
                    &child_location,
                    metadata,
                    diagnostics,
                )?,
            )),
            (Some(i), None) => {
                let child = &original.content[i];
                if let CellContent::Paragraph(paragraph) = child
                    && trailing_paragraph_delete_start.is_some_and(|start| position >= start)
                {
                    if trailing_paragraph_delete_start == Some(position) {
                        trailing_merged =
                            mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                    }
                    if next_takes_mark {
                        content.push((
                            EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                            marked_cell_child(child, "del", metadata)?,
                        ));
                    } else {
                        content.push(final_paragraph(
                            paragraph,
                            "del",
                            None,
                            trailing_merged.clone(),
                            metadata,
                        )?);
                    }
                } else if let CellContent::Paragraph(paragraph) = child
                    && !next_takes_mark
                {
                    let merged = mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                    content.push(final_paragraph(paragraph, "del", None, merged, metadata)?);
                } else {
                    content.push((
                        EmittedOwner::of_cell(child, child, Some("del"), metadata)?,
                        marked_cell_child(child, "del", metadata)?,
                    ));
                }
            }
            (None, Some(j)) => {
                let child = &edited.content[j];
                if let CellContent::Paragraph(paragraph) = child
                    && trailing_paragraph_insert_start.is_some_and(|start| position >= start)
                {
                    if trailing_paragraph_insert_start == Some(position) {
                        trailing_merged =
                            mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                    }
                    if next_takes_mark {
                        content.push((
                            EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                            marked_cell_child(child, "ins", metadata)?,
                        ));
                    } else {
                        content.push(final_paragraph(
                            paragraph,
                            "ins",
                            None,
                            trailing_merged.clone(),
                            metadata,
                        )?);
                    }
                } else if let CellContent::Paragraph(paragraph) = child
                    && !next_takes_mark
                {
                    let merged = mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                    content.push(final_paragraph(paragraph, "ins", None, merged, metadata)?);
                } else {
                    content.push((
                        EmittedOwner::of_cell(child, child, Some("ins"), metadata)?,
                        marked_cell_child(child, "ins", metadata)?,
                    ));
                }
            }
            (None, None) => unreachable!(),
        }
        content_leaves.push(metadata.leaf_publications.split_off(child_mark));
    }
    let mut output = original_xml[..first.start].to_owned();
    for ((_, xml), mut leaves) in content.into_iter().zip(content_leaves) {
        shift_leaf_publications(&mut leaves, output.len() as isize)?;
        metadata.leaf_publications.extend(leaves);
        output.push_str(&xml);
    }
    output.push_str(&original_xml[last.end..]);
    Ok(output)
}

fn marked_cell_child(
    child: &CellContent,
    kind: &str,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match child {
        CellContent::Paragraph(paragraph) if kind == "del" => {
            deleted_paragraph(paragraph, metadata)
        }
        CellContent::Paragraph(paragraph) => inserted_paragraph(paragraph, metadata),
        CellContent::Table(table) => marked_table(table, kind, metadata),
        CellContent::ContentControl(_) => Err(Error::Other(
            "comparison cannot add or remove an unmatched content control in a cell".to_owned(),
        )),
    }
}

fn compare_control(
    original: &CT_Sdt,
    edited: &CT_Sdt,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let original_xml = metadata
        .control_source(original)
        .map(str::to_owned)
        .map_or_else(|| control_xml(original), Ok)?;
    let result = compare_control_from_xml(
        original,
        edited,
        location,
        metadata,
        diagnostics,
        &original_xml,
    )?;
    metadata.ids.generated(result)
}

fn compare_control_from_xml(
    original: &CT_Sdt,
    edited: &CT_Sdt,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    original_xml: &str,
) -> Result<String> {
    let leaf_mark = metadata.leaf_publications.len();
    let output = (|| -> Result<String> {
        if control_property_signature(original) != control_property_signature(edited) {
            return Err(Error::Other(format!(
                "comparison cannot revise content-control properties at {location}"
            )));
        }
        control_metadata_diagnostics(original, edited, location, diagnostics)?;
        let original_content = modeled_control_content(original);
        let edited_content = modeled_control_content(edited);
        // Runs that only split the same text differently read as equal, as
        // they do in a paragraph, and keep the original source. Equal runs
        // keep the path below.
        let only_runs = |content: &[&SdtContent]| {
            !content.is_empty()
                && content
                    .iter()
                    .all(|child| matches!(child, SdtContent::Run(_)))
        };
        if !metadata.leaf_capture
            && only_runs(&original_content)
            && only_runs(&edited_content)
            && original_content != edited_content
        {
            let runs_of = |content: &[&SdtContent]| {
                let mut paragraph = CT_P::new();
                paragraph.runs = content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(run.clone()),
                        _ => None,
                    })
                    .collect();
                paragraph
            };
            if coalesced_paragraphs_equal(
                &runs_of(&original_content),
                &runs_of(&edited_content),
                metadata.options,
                metadata,
            ) {
                return Ok(original_xml.to_owned());
            }
        }
        let direct_run_or_raw =
            |content: &&SdtContent| matches!(content, SdtContent::Run(_) | SdtContent::RawXml(_));
        if uses_attributed_run_path(metadata.options)
            && original_content.iter().all(direct_run_or_raw)
            && edited_content.iter().all(direct_run_or_raw)
            && (!original_content.is_empty() || !edited_content.is_empty())
            && inline_control_raw_boundaries(&original_content, metadata.options)
                == inline_control_raw_boundaries(&edited_content, metadata.options)
        {
            let property_sources = [
                original_content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(metadata.run_source(run).cloned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                edited_content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(run) => Some(metadata.run_source(run).cloned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ];
            let original_runs = original_content
                .iter()
                .filter_map(|content| match content {
                    SdtContent::Run(run) => Some(run.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let edited_runs = edited_content
                .iter()
                .filter_map(|content| match content {
                    SdtContent::Run(run) => Some(run.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let content_spans = direct_word_element_spans(original_xml, "sdtContent")?;
            let content_span = content_spans.first().ok_or_else(|| {
                Error::Other(format!(
                    "comparison could not find inline-control content at {location}"
                ))
            })?;
            let scoped_content = source_child(original_xml, content_span.clone())?;
            let content_source = scoped_content.as_str();
            let inner = element_inner_range_any_prefix(content_source, "sdtContent")?;
            // Raw children remain physical shell boundaries after the runs are
            // extracted. Logical boundary equality above does not preserve them
            // in the run-only alignment by itself.
            let raw_runs = |content: &[&SdtContent]| {
                let mut runs = 0;
                content
                    .iter()
                    .filter_map(|child| match child {
                        SdtContent::Run(_) => {
                            runs += 1;
                            None
                        }
                        SdtContent::RawXml(_) => Some(runs),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
            };
            let boundaries = raw_runs(&original_content)
                .into_iter()
                .zip(raw_runs(&edited_content))
                .collect::<Vec<_>>();
            let modeled_spans = modeled_control_source_spans(&original_content, content_source)?;
            let run_spans = original_content
                .iter()
                .zip(modeled_spans)
                .filter_map(|(child, span)| {
                    matches!(child, SdtContent::Run(_))
                        .then_some(span.start - inner.start..span.end - inner.start)
                })
                .collect::<Vec<_>>();
            let child_mark = metadata.leaf_publications.len();
            let mut field_bindings =
                metadata.bind_cloned_control_runs(&original_content, &original_runs)?;
            field_bindings
                .extend(metadata.bind_cloned_control_runs(&edited_content, &edited_runs)?);
            let compared_result = compare_control_runs_with_options(
                [&original_runs, &edited_runs],
                Some((
                    &source_fragment(
                        &format!(
                            "<w:inlineControl>{}</w:inlineControl>",
                            &content_source[inner]
                        ),
                        &root_namespace_bindings(content_source, "sdtContent", &[])?,
                    )?,
                    &run_spans,
                )),
                Some(&property_sources),
                &boundaries,
                location,
                &root_namespace_bindings(content_source, "sdtContent", &[])?,
                metadata,
                diagnostics,
            );
            metadata.unbind_cloned_control_runs(field_bindings)?;
            let compared = compared_result?;
            let actual_inner =
                element_inner_range_any_prefix(&original_xml[content_span.clone()], "sdtContent")?;
            shift_leaf_publications(
                &mut metadata.leaf_publications[child_mark..],
                (content_span.start + actual_inner.start) as isize,
            )?;
            return replace_element_inner(original_xml, "w:sdtContent", &compared);
        }
        let whitespace_slots = control_whitespace_slots(original)?;
        let original_signatures = original_content
            .iter()
            .map(|content| control_content_signature_with_options(content, metadata.options))
            .collect::<Vec<_>>();
        let edited_signatures = edited_content
            .iter()
            .map(|content| control_content_signature_with_options(content, metadata.options))
            .collect::<Vec<_>>();
        fn paragraphs<'a>(content: &[&'a SdtContent]) -> Vec<Option<&'a CT_P>> {
            content
                .iter()
                .map(|content| match content {
                    SdtContent::Paragraph(paragraph) => Some(paragraph),
                    _ => None,
                })
                .collect()
        }
        let original_paragraphs = paragraphs(&original_content);
        let edited_paragraphs = paragraphs(&edited_content);
        let (aligned, carried) = replace_changed_paragraph_runs(
            pair_gaps_by_kind(
                align(&original_signatures, &edited_signatures),
                &original_signatures,
                &edited_signatures,
            ),
            &original_paragraphs,
            &edited_paragraphs,
            &original_signatures,
            &edited_signatures,
            metadata.options,
        )?;
        refuse_uncarried_field_owners(
            &aligned,
            &original_paragraphs,
            &edited_paragraphs,
            &carried,
            location,
        )?;
        let trailing_paragraph_insert_start = aligned
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, (left, right))| {
                left.is_none()
                    && right
                        .and_then(|index| edited_content.get(index))
                        .is_some_and(|content| matches!(content, SdtContent::Paragraph(_)))
            })
            .map(|(position, _)| position)
            .last();
        let trailing_paragraph_delete_start = aligned
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, (left, right))| {
                right.is_none()
                    && left
                        .and_then(|index| original_content.get(index))
                        .is_some_and(|content| matches!(content, SdtContent::Paragraph(_)))
            })
            .map(|(position, _)| position)
            .last();
        let content_spans = direct_word_element_spans(original_xml, "sdtContent")?;
        let content_span = content_spans.first().ok_or_else(|| {
            Error::Other(format!(
                "comparison could not find content-control content at {location}"
            ))
        })?;
        let scoped_content = source_child(original_xml, content_span.clone())?;
        let content_source = scoped_content.as_str();
        let content_range = element_inner_range_any_prefix(content_source, "sdtContent")?;
        let original_spans = modeled_control_source_spans(&original_content, content_source)?
            .into_iter()
            .map(|span| span.start - content_range.start..span.end - content_range.start)
            .collect::<Vec<_>>();
        let content_inner = &content_source[content_range];
        let bindings = root_namespace_bindings(original_xml, "sdt", &[])?;
        let bindings = root_namespace_bindings(content_source, "sdtContent", &bindings)?;
        if original_spans.len() != original_content.len() {
            return Err(Error::Other(format!(
                "comparison could not correlate content-control owners at {location}"
            )));
        }
        let mut content: Vec<(EmittedOwner, String)> = Vec::new();
        let mut trailing_merged = PropertySide::default();
        let mut whitespace_emitted = vec![false; whitespace_slots.len()];
        let mut child_offsets = Vec::new();
        for (position, (left, right)) in aligned.iter().copied().enumerate() {
            let whitespace_boundary = left.or_else(|| {
                let right = right?;
                let (Some(next_left), None) = aligned.get(position + 1).copied()? else {
                    return None;
                };
                (matches!(original_content[next_left], SdtContent::Paragraph(_))
                    && matches!(edited_content[right], SdtContent::Table(_)))
                .then_some(next_left)
            });
            if let Some(index) = whitespace_boundary
                && !whitespace_emitted[index]
                && !whitespace_slots[index].is_empty()
            {
                content.push((EmittedOwner::Whitespace, whitespace_slots[index].clone()));
                whitespace_emitted[index] = true;
            }
            let item = |(left, right): (Option<usize>, Option<usize>)| {
                right
                    .and_then(|index| edited_content.get(index).copied())
                    .or_else(|| left.and_then(|index| original_content.get(index).copied()))
            };
            let next_takes_mark = next_kept_owner(&aligned, position, |pair| {
                matches!(item(pair), Some(SdtContent::Table(_)))
            })
            .is_some_and(|pair| {
                matches!(
                    item(pair),
                    Some(SdtContent::Paragraph(_) | SdtContent::Table(_))
                )
            });
            let child_mark = metadata.leaf_publications.len();
            let child_offset = content.iter().map(|(_, xml)| xml.len()).sum::<usize>();
            let child_location = format!("{location}/content[{position}]");
            match (left, right) {
                (Some(i), Some(j)) => content.push((
                    EmittedOwner::of_control(
                        original_content[i],
                        edited_content[j],
                        None,
                        metadata,
                    )?,
                    compare_control_content(
                        original_content[i],
                        edited_content[j],
                        Some(&source_fragment(
                            &content_inner[original_spans[i].clone()],
                            &bindings,
                        )?),
                        &child_location,
                        &bindings,
                        metadata,
                        diagnostics,
                    )?,
                )),
                (Some(i), None) => {
                    if let SdtContent::Paragraph(paragraph) = original_content[i]
                        && trailing_paragraph_delete_start.is_some_and(|start| position >= start)
                    {
                        if trailing_paragraph_delete_start == Some(position) {
                            trailing_merged =
                                mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                        }
                        if next_takes_mark {
                            content.push((
                                EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                                marked_control_content(
                                    original_content[i],
                                    "del",
                                    &bindings,
                                    metadata,
                                )?,
                            ));
                        } else {
                            content.push(final_paragraph(
                                paragraph,
                                "del",
                                None,
                                trailing_merged.clone(),
                                metadata,
                            )?);
                        }
                    } else if let SdtContent::Paragraph(paragraph) = original_content[i]
                        && !next_takes_mark
                    {
                        let merged =
                            mark_previous_paragraph(&mut content, "del", &bindings, metadata)?;
                        content.push(final_paragraph(paragraph, "del", None, merged, metadata)?);
                    } else {
                        content.push((
                            EmittedOwner::of_control(
                                original_content[i],
                                original_content[i],
                                Some("del"),
                                metadata,
                            )?,
                            marked_control_content(
                                original_content[i],
                                "del",
                                &bindings,
                                metadata,
                            )?,
                        ));
                    }
                }
                (None, Some(j)) => {
                    if let SdtContent::Paragraph(paragraph) = edited_content[j]
                        && trailing_paragraph_insert_start.is_some_and(|start| position >= start)
                    {
                        if trailing_paragraph_insert_start == Some(position) {
                            trailing_merged =
                                mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                        }
                        if next_takes_mark {
                            content.push((
                                EmittedOwner::paragraph(paragraph, paragraph, metadata)?,
                                marked_control_content(
                                    edited_content[j],
                                    "ins",
                                    &bindings,
                                    metadata,
                                )?,
                            ));
                        } else {
                            content.push(final_paragraph(
                                paragraph,
                                "ins",
                                None,
                                trailing_merged.clone(),
                                metadata,
                            )?);
                        }
                    } else if let SdtContent::Paragraph(paragraph) = edited_content[j]
                        && !next_takes_mark
                    {
                        let merged =
                            mark_previous_paragraph(&mut content, "ins", &bindings, metadata)?;
                        content.push(final_paragraph(paragraph, "ins", None, merged, metadata)?);
                    } else {
                        content.push((
                            EmittedOwner::of_control(
                                edited_content[j],
                                edited_content[j],
                                Some("ins"),
                                metadata,
                            )?,
                            marked_control_content(edited_content[j], "ins", &bindings, metadata)?,
                        ));
                    }
                }
                (None, None) => unreachable!(),
            }
            child_offsets.push((child_mark, metadata.leaf_publications.len(), child_offset));
        }
        let container = direct_word_element_spans(original_xml, "sdtContent")?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Other("control has no actual content source".into()))?;
        let actual_content =
            element_inner_range_any_prefix(&original_xml[container.clone()], "sdtContent")?;
        let actual_content =
            container.start + actual_content.start..container.start + actual_content.end;
        for (start, end, offset) in child_offsets {
            shift_leaf_publications(
                &mut metadata.leaf_publications[start..end],
                (actual_content.start + offset) as isize,
            )?;
        }
        if let Some(trailing) = whitespace_slots.last()
            && !trailing.is_empty()
        {
            content.push((EmittedOwner::Whitespace, trailing.clone()));
        }
        let content = content.into_iter().map(|(_, xml)| xml).collect::<String>();
        replace_element_inner(original_xml, "w:sdtContent", &content)
    })()?;
    let old_open = element_opening(&output)?.0;
    let output = metadata.restore_owner_root(original as *const CT_Sdt as usize, output)?;
    shift_leaf_publications(
        &mut metadata.leaf_publications[leaf_mark..],
        element_opening(&output)?.0 as isize - old_open as isize,
    )?;
    Ok(output)
}

fn inline_control_raw_boundaries(
    content: &[&SdtContent],
    options: &ComparisonOptions,
) -> Vec<(usize, Vec<u8>)> {
    let mut runs = Vec::new();
    let mut raw = Vec::new();
    for child in content {
        match child {
            SdtContent::Run(run) => runs.push((*run).clone()),
            SdtContent::RawXml(bytes) => {
                let boundary = attributed_run_units(&runs, options)
                    .into_iter()
                    .filter(|unit| !unit_is_ignorable(unit) && !unit_is_empty(unit))
                    .count();
                raw.push((boundary, bytes.clone()));
            }
            _ => unreachable!(),
        }
    }
    raw
}

fn compare_control_content(
    original: &SdtContent,
    edited: &SdtContent,
    original_source: Option<&str>,
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    match (original, edited) {
        (SdtContent::Paragraph(left), SdtContent::Paragraph(right)) => compare_paragraph(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::Table(left), SdtContent::Table(right)) => compare_table(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::ContentControl(left), SdtContent::ContentControl(right)) => {
            if let Some(source) = original_source {
                compare_control_from_xml(left, right, location, metadata, diagnostics, source)
            } else {
                compare_control(left, right, location, metadata, diagnostics)
            }
        }
        (SdtContent::Row(left), SdtContent::Row(right)) => compare_row(
            left,
            right,
            original_source,
            location,
            metadata,
            diagnostics,
        ),
        (SdtContent::Cell(left), SdtContent::Cell(right)) => compare_cell_from_xml(
            left,
            right,
            location,
            metadata,
            diagnostics,
            &original_source
                .map(str::to_owned)
                .map_or_else(|| cell_xml(left), Ok)?,
        ),
        (SdtContent::Run(left), SdtContent::Run(right)) => {
            if uses_attributed_run_path(metadata.options) {
                compare_control_runs_with_options(
                    [std::slice::from_ref(left), std::slice::from_ref(right)],
                    None,
                    None,
                    &[],
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            } else if run_is_field(left) || run_is_field(right) {
                compared_run_xml(
                    left,
                    right,
                    None,
                    None,
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            } else if run_signature(left) == run_signature(right) {
                if !metadata.options.ignore_formatting
                    && run_formatting(left) != run_formatting(right)
                {
                    formatting_diagnostic(diagnostics, location.to_owned());
                }
                if left == right
                    && let Some(source) = original_source
                {
                    Ok(source.to_owned())
                } else {
                    publish_run_source(
                        left,
                        left,
                        run_xml(left)?,
                        metadata.run_source(left),
                        None,
                        None,
                        destination_parent,
                        metadata,
                    )
                }
            } else {
                paired_run_replacement(
                    [left, right],
                    [None, None],
                    [None, None],
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )
            }
        }
        (SdtContent::RawXml(left), SdtContent::RawXml(right)) if left == right => {
            String::from_utf8(left.clone()).map_err(utf8_error)
        }
        _ => Err(Error::Other(format!(
            "comparison cannot revise incompatible content-control children at {location}"
        ))),
    }
}

fn compare_control_runs_with_options(
    runs: [&[CT_R]; 2],
    original_source: Option<(&str, &[Range<usize>])>,
    property_sources: Option<&[Vec<Option<RunSource>>; 2]>,
    raw_boundaries: &[(usize, usize)],
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let revision_parent = metadata.ids.revision_parent_scope(destination_parent);
    let [original, edited] = runs;
    let projection_pairs = metadata
        .leaf_capture
        .then(|| projection_leaf_pairs([original, edited], raw_boundaries));
    let projection_sources = [original, edited]
        .into_iter()
        .enumerate()
        .map(|(side, runs)| {
            runs.iter()
                .enumerate()
                .map(|(owner, run)| {
                    let _ = (side, owner);
                    property_sources
                        .and_then(|sources| sources[side].get(owner).cloned().flatten())
                        .or_else(|| metadata.run_source(run).cloned())
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let projection_sources = [projection_sources[0].clone(), projection_sources[1].clone()];
    let mut projection_ledgers = if let Some(pairs) = &projection_pairs {
        projection_action_ledgers(pairs, [original, edited], &projection_sources)?
    } else {
        Vec::new()
    };
    let original_units = attributed_run_units(original, metadata.options);
    let edited_units = attributed_run_units(edited, metadata.options);
    validate_raw_attribution(original, &original_units)?;
    validate_raw_attribution(edited, &edited_units)?;
    let original_signatures = original_units
        .iter()
        .map(attributed_unit_signature)
        .collect::<Vec<_>>();
    let edited_signatures = edited_units
        .iter()
        .map(attributed_unit_signature)
        .collect::<Vec<_>>();
    let cuts = if metadata.options.granularity == ComparisonGranularity::Run {
        Vec::new()
    } else {
        shell_unit_cuts(
            &original_units,
            &edited_units,
            raw_boundaries.iter().copied(),
        )
        .ok_or_else(|| {
            Error::Other(format!(
                "comparison cannot revise inline-control raw boundaries at {location}"
            ))
        })?
    };
    let (aligned, segments) = if let Some(pairs) = &projection_pairs {
        projection_unit_alignment(
            pairs,
            [&original_units, &edited_units],
            [&original_signatures, &edited_signatures],
        )?
    } else {
        align_between_shells(&original_signatures, &edited_signatures, &cuts)
    };
    let segment_runs = if let Some(pairs) = &projection_pairs {
        pairs
            .iter()
            .enumerate()
            .map(|(at, pair)| {
                pair[0]
                    .map(|(owner, _)| owner)
                    .or_else(|| {
                        pairs[at..]
                            .iter()
                            .find_map(|next| next[0].map(|(owner, _)| owner))
                    })
                    .unwrap_or(original.len())
            })
            .collect::<Vec<_>>()
    } else {
        std::iter::once(0)
            .chain(cuts.iter().map(|&(cut, _)| {
                original_units
                    .get(cut)
                    .map_or(original.len(), |unit| unit.owner)
            }))
            .collect::<Vec<_>>()
    };
    if !metadata.options.ignore_fields {
        validate_field_alignment(
            &aligned,
            &original_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &edited_units
                .iter()
                .map(|unit| unit.run.clone())
                .collect::<Vec<_>>(),
            &original_signatures,
            &edited_signatures,
            location,
        )?;
    }
    let (aligned, segments) = grouped_text_alignment(
        &aligned,
        &segments,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
        metadata.options,
    );
    let mut grouped = coalesced_granular_alignment(
        &aligned,
        &original_units,
        &edited_units,
        &original_signatures,
        &edited_signatures,
    );
    if let Some(pairs) = &projection_pairs {
        retain_projection_separators(
            &mut grouped,
            &aligned,
            &segments,
            pairs,
            [&original_units, &edited_units],
            &projection_sources,
            metadata.options,
        )?;
    }
    validate_fragment_partition(&aligned, &grouped)?;
    let mut grouped_alignment = Vec::with_capacity(grouped.len());
    let mut grouped_segment_runs = Vec::with_capacity(grouped.len());
    let mut replacements = Vec::with_capacity(grouped.len());
    let mut fragment_publications = Vec::with_capacity(grouped.len());
    for (action, members) in grouped {
        if metadata.leaf_capture {
            record_projection_actions(
                &mut projection_ledgers,
                action,
                members.clone(),
                &aligned,
                [&original_units, &edited_units],
                &projection_sources,
            )?;
        }
        grouped_alignment.push(aligned[members.start]);
        grouped_segment_runs.push(segment_runs[segments[members.start]]);
        let left_indices = members
            .clone()
            .filter_map(|index| aligned[index].0)
            .collect::<Vec<_>>();
        let right_indices = members
            .filter_map(|index| aligned[index].1)
            .collect::<Vec<_>>();
        let mut left_raw = Vec::new();
        let mut left_typed = Vec::new();
        let mut right_raw = Vec::new();
        let mut right_typed = Vec::new();
        let left = merge_unit_runs(
            original,
            &original_units,
            &left_indices,
            &mut left_raw,
            &mut left_typed,
        )?;
        let right = merge_unit_runs(
            edited,
            &edited_units,
            &right_indices,
            &mut right_raw,
            &mut right_typed,
        )?;
        let left_source = left_indices.first().and_then(|index| {
            let owner = original_units[*index].owner;
            property_sources
                .and_then(|sources| sources[0].get(owner).cloned().flatten())
                .or_else(|| metadata.run_source(&original[owner]).cloned())
        });
        let right_source = right_indices.first().and_then(|index| {
            let owner = edited_units[*index].owner;
            property_sources
                .and_then(|sources| sources[1].get(owner).cloned().flatten())
                .or_else(|| metadata.run_source(&edited[owner]).cloned())
        });
        let policy_shell = projection_pairs
            .as_ref()
            .and_then(|pairs| {
                right_indices.first().and_then(|index| {
                    let unit = &edited_units[*index];
                    if !matches!(
                        unit.run.content.first(),
                        Some(RunContent::Text(_) | RunContent::DeletedText(_))
                    ) {
                        return None;
                    }
                    let ordinal = unit.typed_sources.first().and_then(|slot| *slot)?.ordinal;
                    pairs.iter().find_map(|pair| {
                        (pair[1] == Some((unit.owner, ordinal)))
                            .then_some(pair[0])
                            .flatten()
                    })
                })
            })
            .map(|(owner, ordinal)| {
                let run = &original[owner];
                let source = property_sources
                    .and_then(|sources| sources[0].get(owner).cloned().flatten())
                    .or_else(|| metadata.run_source(run).cloned())
                    .ok_or_else(|| {
                        Error::Other("paired controlled shell has no actual source".into())
                    })?;
                if source
                    .typed_slots
                    .get(ordinal)
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    return Err(Error::Other(
                        "paired controlled leaf has no physical source slot".into(),
                    ));
                }
                Ok((run, source))
            })
            .transpose()?;
        let mut field_bindings = Vec::new();
        for (indices, units, originals, cloned, selected) in [
            (
                &left_indices,
                &original_units,
                original,
                left.as_ref(),
                &left_typed,
            ),
            (
                &right_indices,
                &edited_units,
                edited,
                right.as_ref(),
                &right_typed,
            ),
        ] {
            if let Some(cloned) = cloned
                && run_is_field(cloned)
            {
                let owner = units[indices[0]].owner;
                match metadata.bind_selected_run_fields(&originals[owner], cloned, selected) {
                    Ok(bindings) => field_bindings.extend(bindings),
                    Err(error) => {
                        metadata.unbind_cloned_control_runs(field_bindings)?;
                        return Err(error);
                    }
                }
            }
        }
        let replacement = (|| -> Result<String> {
            Ok(match action {
                GranularAction::Preserve => publish_run_source(
                    left.as_ref().unwrap(),
                    left.as_ref().unwrap(),
                    paragraph_owned_run_xml(left.as_ref().unwrap())?,
                    left_source.as_ref(),
                    None,
                    Some((&left_raw, &left_typed)),
                    destination_parent,
                    metadata,
                )?,
                GranularAction::Equal => compared_run_xml(
                    left.as_ref().unwrap(),
                    right.as_ref().unwrap(),
                    left_source.as_ref(),
                    Some((&left_raw, &left_typed)),
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Replace => paired_run_replacement(
                    [left.as_ref().unwrap(), right.as_ref().unwrap()],
                    [left_source.as_ref(), right_source.as_ref()],
                    [
                        Some((&left_raw, &left_typed)),
                        Some((&right_raw, &right_typed)),
                    ],
                    location,
                    destination_parent,
                    metadata,
                    diagnostics,
                )?,
                GranularAction::Delete => {
                    let payload = publish_run_source(
                        left.as_ref().unwrap(),
                        left.as_ref().unwrap(),
                        deleted_run_xml(left.as_ref().unwrap())?,
                        left_source.as_ref(),
                        None,
                        Some((&left_raw, &left_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    metadata
                        .ids
                        .revision("del", metadata.author, metadata.timestamp, &payload)?
                }
                GranularAction::Insert => {
                    let mut content = right.as_ref().unwrap().clone();
                    let (shell, shell_source) = if let Some((shell, source)) = &policy_shell {
                        let before = usize::from(content.properties.is_some());
                        content.properties = shell.properties.clone();
                        let after = usize::from(content.properties.is_some());
                        for encoded in &mut content.extra_xml_positions {
                            if !CT_R::raw_child_is_root_attributes(*encoded) {
                                let at = CT_R::raw_child_position(*encoded).saturating_sub(before)
                                    + after;
                                CT_R::set_raw_child_position(encoded, at);
                            }
                        }
                        (*shell, Some(source))
                    } else {
                        (right.as_ref().unwrap(), right_source.as_ref())
                    };
                    let payload = publish_run_source(
                        shell,
                        &content,
                        paragraph_owned_run_xml(&content)?,
                        shell_source,
                        right_source.as_ref(),
                        Some((&right_raw, &right_typed)),
                        &revision_parent,
                        metadata,
                    )?;
                    metadata
                        .ids
                        .revision("ins", metadata.author, metadata.timestamp, &payload)?
                }
                GranularAction::Drop => String::new(),
            })
        })();
        metadata.unbind_cloned_control_runs(field_bindings)?;
        replacements.push(replacement?);
        let mut leaves = Vec::new();
        if metadata.leaf_capture {
            let selected = match action {
                GranularAction::Replace | GranularAction::Insert => right
                    .as_ref()
                    .zip(right_source.as_ref())
                    .map(|(run, source)| {
                        (
                            run,
                            source,
                            &right_raw[..],
                            &right_typed[..],
                            matches!(action, GranularAction::Replace | GranularAction::Insert),
                        )
                    }),
                GranularAction::Equal | GranularAction::Preserve => left
                    .as_ref()
                    .zip(left_source.as_ref())
                    .map(|(run, source)| (run, source, &left_raw[..], &left_typed[..], false)),
                _ => None,
            };
            if let Some((run, source, raw, typed, inserted)) = selected {
                leaves = emitted_fragment_leaves(
                    replacements.last().unwrap(),
                    run,
                    source,
                    raw,
                    typed,
                    inserted,
                    metadata.options.ignore_whitespace,
                    destination_parent,
                )?;
            }
        }
        fragment_publications.push(leaves);
    }
    if metadata.leaf_capture {
        attach_projection_actions(&mut fragment_publications, &projection_ledgers)?;
    }
    let Some((source, spans)) = original_source else {
        let mut output = String::new();
        for (replacement, mut leaves) in replacements.into_iter().zip(fragment_publications) {
            shift_leaf_publications(&mut leaves, output.len() as isize)?;
            metadata.leaf_publications.extend(leaves);
            output.push_str(&replacement);
        }
        return Ok(output);
    };
    let inner = element_inner_range_any_prefix(source, "inlineControl")?;
    let source = &source[inner];
    if spans.len() != original.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate inline-control run owners at {location}"
        )));
    }
    let insertion_boundary = spans.first().map_or(source.len(), |span| span.start);
    let mut output = source[..insertion_boundary].to_owned();
    let mut cursor = insertion_boundary;
    let mut consumed_owner = None;
    for (replacement_index, (((left, _), replacement), &segment_run)) in grouped_alignment
        .iter()
        .zip(&replacements)
        .zip(&grouped_segment_runs)
        .enumerate()
    {
        // An insertion can be the first action after a raw child, with no
        // original unit to consume. Copy that shell before its new text.
        let segment_start = spans
            .get(segment_run)
            .map_or(source.len(), |span| span.start);
        if segment_start > cursor {
            output.push_str(&source[cursor..segment_start]);
            cursor = segment_start;
        }
        if let Some(unit) = left.map(|index| &original_units[index])
            && consumed_owner != Some(unit.owner)
        {
            let span = &spans[unit.owner];
            output.push_str(&source[cursor..span.start]);
            cursor = span.end;
            consumed_owner = Some(unit.owner);
        }
        let mut leaves = fragment_publications[replacement_index].clone();
        shift_leaf_publications(&mut leaves, output.len() as isize)?;
        metadata.leaf_publications.extend(leaves);
        output.push_str(replacement);
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

fn marked_control_content(
    content: &SdtContent,
    kind: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    match content {
        SdtContent::Paragraph(paragraph) if kind == "del" => deleted_paragraph(paragraph, metadata),
        SdtContent::Paragraph(paragraph) => inserted_paragraph(paragraph, metadata),
        SdtContent::Table(table) => marked_table(table, kind, metadata),
        SdtContent::Row(row) => marked_row(row, kind, metadata),
        SdtContent::Run(run) => {
            let generated = if kind == "del" {
                deleted_run_xml(run)?
            } else {
                paragraph_owned_run_xml(run)?
            };
            let revision_parent = metadata.ids.revision_parent_scope(destination_parent);
            let payload = publish_run_source(
                run,
                run,
                generated,
                None,
                None,
                None,
                &revision_parent,
                metadata,
            )?;
            metadata
                .ids
                .revision(kind, metadata.author, metadata.timestamp, &payload)
        }
        SdtContent::RawXml(raw) => String::from_utf8(raw.clone()).map_err(utf8_error),
        SdtContent::Cell(_) | SdtContent::ContentControl(_) => Err(Error::Other(
            "comparison cannot add or remove an unmatched cell or nested content control"
                .to_owned(),
        )),
    }
}

fn publish_run_source(
    original: &CT_R,
    content: &CT_R,
    generated: String,
    source: Option<&RunSource>,
    content_source: Option<&RunSource>,
    selection: Option<RunSourceSelection<'_>>,
    destination_parent: &NamespaceBindings,
    metadata: &Metadata<'_>,
) -> Result<String> {
    let source = source.or_else(|| metadata.run_source(original));
    if let Some(source) = source {
        let content_source = content_source
            .or_else(|| metadata.run_source(content))
            .unwrap_or(source);
        let complete;
        let (raw, typed) = if let Some(selection) = selection {
            selection
        } else {
            complete = complete_run_selection(content);
            (&complete.0[..], &complete.1[..])
        };
        preserve_run_source(
            &generated,
            &run_property_xml(original.properties.as_ref())?,
            [source, content_source],
            content,
            raw,
            typed,
            destination_parent,
            &metadata.ids,
        )
    } else {
        metadata.ids.generated(generated)
    }
}

fn compared_run_xml(
    original: &CT_R,
    edited: &CT_R,
    source: Option<&RunSource>,
    selection: Option<RunSourceSelection<'_>>,
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if let (Some(original_field), Some(edited_field)) = (run_field(original), run_field(edited)) {
        // A virtual Field is not a physical run owner. Its producer qualifies
        // new revision nodes without rebinding the retained field's namespace.
        return if metadata.options.ignore_fields {
            field_xml(original_field)
        } else {
            compare_field_xml(original_field, edited_field, metadata)
        };
    }
    let source = source.or_else(|| metadata.run_source(original)).cloned();
    let generated = compared_run_xml_impl(original, edited, location, metadata, diagnostics)?;
    publish_run_source(
        original,
        original,
        generated,
        source.as_ref(),
        None,
        selection,
        destination_parent,
        metadata,
    )
}

fn compared_run_xml_impl(
    original: &CT_R,
    edited: &CT_R,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if metadata.options.ignore_fields && (run_is_field(original) || run_is_field(edited)) {
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    if metadata.options.ignore_formatting {
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    let original_properties = modeled_run_properties(original);
    let edited_properties = modeled_run_properties(edited);
    if original_properties == edited_properties {
        if run_formatting(original) != run_formatting(edited) {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    let previous = run_property_xml(original_properties.as_ref())?;
    let change =
        metadata
            .ids
            .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
    if unmodeled_run_properties_differ(original, edited) {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    let mut current_run = edited.clone();
    preserve_unmodeled_run_properties(&mut current_run, original);
    let mut current = run_xml(&current_run)?;
    let properties = direct_word_element_spans(&current, "rPr")?;
    if let Some(span) = properties.first() {
        let updated = append_word_child(&current[span.clone()], "rPr", &change)?;
        current.replace_range(span.clone(), &updated);
    } else {
        current = insert_first_child(&current, &format!("<w:rPr>{change}</w:rPr>"))?;
    }
    metadata.ids.generated(current)
}

fn field_result_scope(
    field: &oxoffice_docx_oxml::text::Field,
    metadata: &Metadata<'_>,
) -> NamespaceBindings {
    metadata
        .source_frames
        .iter()
        .rev()
        .find_map(|frame| {
            frame
                .field_scopes
                .get(&(std::ptr::from_ref(field) as usize))
        })
        .cloned()
        .unwrap_or_else(|| vec![("xmlns:w".into(), W_NS.into())])
}

fn comparison_instruction_eq(
    left: &oxoffice_docx_oxml::text::FieldInstruction,
    right: &oxoffice_docx_oxml::text::FieldInstruction,
) -> bool {
    use oxoffice_docx_oxml::text::FieldArgument;
    let same_operand = |left: &FieldArgument, right: &FieldArgument| match (left, right) {
        (FieldArgument::Text(left), FieldArgument::Text(right)) => left == right,
        (FieldArgument::Nested(left), FieldArgument::Nested(right)) => {
            comparison_instruction_eq(&left.instruction, &right.instruction)
        }
        _ => false,
    };
    left.name == right.name
        && left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(left, right)| same_operand(left, right))
        && left.switches.len() == right.switches.len()
        && left
            .switches
            .iter()
            .zip(&right.switches)
            .all(|(left, right)| {
                left.name == right.name
                    && match (&left.argument, &right.argument) {
                        (None, None) => true,
                        (Some(left), Some(right)) => same_operand(left, right),
                        _ => false,
                    }
            })
}

fn compare_field_xml(
    original: &oxoffice_docx_oxml::text::Field,
    edited: &oxoffice_docx_oxml::text::Field,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let original_xml = field_xml(original)?;
    let edited_xml = field_xml(edited)?;
    let old_scope = field_result_scope(original, metadata);
    let new_scope = field_result_scope(edited, metadata);

    let original_complex_form = actual_field_is_complex(&original_xml, &old_scope)?;
    let edited_complex_form = actual_field_is_complex(&edited_xml, &new_scope)?;
    let same_instruction = comparison_instruction_eq(
        &original.effective_instruction(),
        &edited.effective_instruction(),
    );
    let same_form = original_complex_form == edited_complex_form;
    // Representation dispatch precedes global Field equality, which deliberately
    // omits simple/complex form. Protected source facts remain independent.
    let same_source_facts =
        metadata.field_source_facts(original) == metadata.field_source_facts(edited);
    if same_instruction && same_form && original == edited && same_source_facts {
        return Ok(original_xml);
    }
    if same_instruction && same_form {
        if original_complex_form {
            let (before, old_result, after) = complex_field_result(&original_xml)?;
            let (_, new_result, _) = complex_field_result(&edited_xml)?;
            return Ok(format!(
                "{before}{}{after}",
                tracked_field_result(
                    [original, edited],
                    &old_result,
                    &new_result,
                    [&old_scope, &new_scope],
                    metadata
                )?
            ));
        }
        let original_xml = append_word_child(&original_xml, "fldSimple", "")?;
        let edited_xml = append_word_child(&edited_xml, "fldSimple", "")?;
        let original_inner = element_inner_range_any_prefix(&original_xml, "fldSimple")?;
        let edited_inner = element_inner_range_any_prefix(&edited_xml, "fldSimple")?;
        let old_data = direct_word_element_spans_in_scope(&original_xml, "fldData", &old_scope)?;
        let new_data = direct_word_element_spans_in_scope(&edited_xml, "fldData", &new_scope)?;
        if old_data.len() > 1 || new_data.len() > 1 {
            return Err(Error::Other(
                "field result comparison has ambiguous fldData ownership".into(),
            ));
        }
        let old_start = old_data
            .first()
            .map_or(original_inner.start, |span| span.end);
        let new_start = new_data.first().map_or(edited_inner.start, |span| span.end);
        let tracked = tracked_field_result(
            [original, edited],
            &original_xml[old_start..original_inner.end],
            &edited_xml[new_start..edited_inner.end],
            [&old_scope, &new_scope],
            metadata,
        )?;
        let mut output = original_xml;
        output.replace_range(old_start..original_inner.end, &tracked);
        return Ok(output);
    }

    if let Some(story) = &metadata.paired_field_story {
        let actual = |field: &oxoffice_docx_oxml::text::Field| {
            metadata.source_frames.iter().rev().find_map(|frame| {
                frame
                    .field_occurrences
                    .get(&(std::ptr::from_ref(field) as usize))
            })
        };
        let left = actual(original).ok_or_else(|| {
            Error::Other("paired field lacks its actual original source occurrence".into())
        })?;
        let right = actual(edited).ok_or_else(|| {
            Error::Other("paired field lacks its actual edited source occurrence".into())
        })?;
        if left.story != *story
            || right.story != *story
            || left.input_role != 0
            || right.input_role != 1
            || left.owner_path != right.owner_path
            || left.field_slot != right.field_slot
            || left.bounds.is_empty()
            || right.bounds.is_empty()
        {
            return Err(Error::Other(
                "paired field does not belong to the checked host interval".into(),
            ));
        }
        // The outer checked host owns the replacement. Its actual selected side
        // remains its own representation, with no ordinary conversion witness.
        return Ok(edited_xml);
    }

    let original_complex = if original_complex_form {
        original_xml
    } else {
        clean_simple_complex_source(&original_xml, &old_scope, original)?
    };
    let edited_complex = if edited_complex_form {
        edited_xml
    } else {
        clean_simple_complex_source(&edited_xml, &new_scope, edited)?
    };
    let deleted_source =
        qualified_complete_field_payload(&original_complex, &old_scope, true, false, true, &mut 0)?;
    let inserted_source =
        qualified_complete_field_payload(&edited_complex, &new_scope, false, false, true, &mut 0)?;
    let deleted_id = metadata.ids.allocate()?;
    let inserted_id = metadata.ids.allocate()?;
    let deleted = metadata.ids.generated(IdAllocator::revision_with_id(
        "del",
        metadata.author,
        metadata.timestamp,
        &deleted_source,
        deleted_id,
    ))?;
    let inserted = metadata.ids.generated(IdAllocator::revision_with_id(
        "ins",
        metadata.author,
        metadata.timestamp,
        &inserted_source,
        inserted_id,
    ))?;
    if !original_complex_form || !edited_complex_form {
        let occurrence = |field: &oxoffice_docx_oxml::text::Field| {
            metadata
                .source_frames
                .iter()
                .rev()
                .find_map(|frame| {
                    frame
                        .field_occurrences
                        .get(&(std::ptr::from_ref(field) as usize))
                })
                .cloned()
        };
        let left = occurrence(original).ok_or_else(|| {
            Error::Other(
                "ordinary field conversion has no original physical source occurrence".into(),
            )
        })?;
        let right = occurrence(edited).ok_or_else(|| {
            Error::Other(
                "ordinary field conversion has no edited physical source occurrence".into(),
            )
        })?;
        {
            if left.input_role != 0 || right.input_role != 1 || left.story != right.story {
                return Err(Error::Other(
                    "field conversion input roles or physical story association changed".into(),
                ));
            }
            metadata.field_conversions.push(FieldConversionWitness {
                sources: [left, right],
                instruction_changed: !same_instruction,
                form_changed: !same_form,
                input_complex: [original_complex_form, edited_complex_form],
                revision_ids: [deleted_id, inserted_id],
                generated_spans: [deleted_source, inserted_source],
            });
        }
    }
    Ok(format!("{deleted}{inserted}"))
}

fn actual_field_source_fragment(xml: &str, bindings: &NamespaceBindings) -> Result<String> {
    let scope = bindings
        .iter()
        .map(|(name, uri)| {
            (
                name.strip_prefix("xmlns:").unwrap_or("").to_owned(),
                uri.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    String::from_utf8(crate::document::close_content_fragment_namespaces(
        xml.as_bytes(),
        &scope,
    )?)
    .map_err(utf8_error)
}

fn actual_field_is_complex(xml: &str, bindings: &NamespaceBindings) -> Result<bool> {
    let closed = actual_field_source_fragment(xml, bindings)?;
    let mut reader = NsReader::from_str(&closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(element) | Event::Empty(element)) = event else {
        return Err(Error::Other("field has no actual source root".into()));
    };
    if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
        return Err(Error::Other(
            "field source root has no actual Word binding".into(),
        ));
    }
    match element.local_name().as_ref() {
        b"fldSimple" => Ok(false),
        b"r" => Ok(true),
        _ => Err(Error::Other(
            "field source root is not a qualified field carrier".into(),
        )),
    }
}

fn conversion_namespace_facts(bindings: NamespaceBindings) -> BTreeMap<String, String> {
    bindings
        .into_iter()
        .filter(|(name, uri)| !(name == "xmlns:w" && uri == W_NS))
        .collect()
}

fn field_source_sequence_budget(source: &str, scope: &NamespaceBindings) -> Result<Vec<String>> {
    let wrapped = format!("<sourceBudget>{source}</sourceBudget>");
    let mut output = Vec::new();
    let mut cursor = "<sourceBudget>".len();
    for span in direct_element_spans(&wrapped)? {
        output.push(format!("outside:{:?}", &wrapped[cursor..span.start]));
        let (lexical, bindings) = raw_occurrence_representation(&wrapped[span.clone()], scope)?;
        output.push(format!(
            "root:{:?}:{lexical:?}",
            conversion_namespace_facts(bindings)
        ));
        cursor = span.end;
    }
    output.push(format!(
        "outside:{:?}",
        &wrapped[cursor..wrapped.len() - "</sourceBudget>".len()]
    ));
    Ok(output)
}

fn field_carrier_source_budget(source: &str, scope: &NamespaceBindings) -> Result<Vec<String>> {
    let result = actual_field_result_range(source, scope)?;
    if actual_field_is_complex(source, scope)? {
        let mut facts = field_source_sequence_budget(&source[..result.start], scope)?;
        facts.push("result-boundary".into());
        facts.extend(field_source_sequence_budget(&source[result.end..], scope)?);
        Ok(facts)
    } else {
        let (lexical, bindings) = raw_occurrence_representation(source, scope)?;
        let growth = element_opening(source)?
            .0
            .checked_sub(element_opening(&lexical)?.0)
            .ok_or_else(|| {
                Error::Other("field carrier namespace projection grew its opening".into())
            })?;
        let start = result
            .start
            .checked_sub(growth)
            .ok_or_else(|| Error::Other("field carrier result precedes opening".into()))?;
        let end = result
            .end
            .checked_sub(growth)
            .ok_or_else(|| Error::Other("field carrier result precedes opening".into()))?;
        Ok(vec![
            format!("scope:{:?}", conversion_namespace_facts(bindings)),
            format!("before:{:?}", &lexical[..start]),
            format!("after:{:?}", &lexical[end..]),
        ])
    }
}

/// Read encoding facts from an actual carrier. The guard never invokes the
/// comparison converter to produce expected source or an expected model.
fn conversion_source_inventory(
    source: &str,
    inherited: &NamespaceBindings,
) -> Result<(String, BTreeMap<String, String>, String, String)> {
    fn element(
        source: &str,
        scope: &NamespaceBindings,
        expected: &str,
        allowed: &[&str],
    ) -> Result<BTreeMap<String, (String, String)>> {
        let closed = actual_field_source_fragment(source, scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let (Event::Start(root) | Event::Empty(root)) = event else {
            return Err(Error::Other("conversion inventory has no element".into()));
        };
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            || root.local_name().as_ref() != expected.as_bytes()
        {
            return Err(Error::Other(
                "conversion inventory has an unproved qualified carrier".into(),
            ));
        }
        let mut attributes = BTreeMap::new();
        for attribute in root.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            let xml_space = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == b"http://www.w3.org/XML/1998/namespace")
                && local.as_ref() == b"space";
            let name = if xml_space {
                "xml:space".to_owned()
            } else {
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    return Err(Error::Other(
                        "unconsumed conversion carrier attribute namespace".into(),
                    ));
                }
                std::str::from_utf8(local.as_ref())
                    .map_err(utf8_error)?
                    .to_owned()
            };
            if !allowed.contains(&name.as_str()) {
                return Err(Error::Other(
                    "unconsumed conversion carrier attribute".into(),
                ));
            }
            let lexical = std::str::from_utf8(attribute.value.as_ref())
                .map_err(utf8_error)?
                .to_owned();
            let decoded = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
                .map_err(|error| Error::Other(error.to_string()))?
                .into_owned();
            if attributes.insert(name, (lexical, decoded)).is_some() {
                return Err(Error::Other(
                    "duplicate conversion carrier attribute".into(),
                ));
            }
        }
        Ok(attributes)
    }
    fn run_child<'a>(run: &'a str, scope: &NamespaceBindings) -> Result<&'a str> {
        element(run, scope, "r", &[])?;
        let inner = element_inner_range_any_prefix(run, "r")?;
        let children = direct_element_spans(run)?;
        if children.len() != 1
            || !run[inner.start..children[0].start].trim().is_empty()
            || !run[children[0].end..inner.end].trim().is_empty()
        {
            return Err(Error::Other(
                "conversion marker run has additional source payload".into(),
            ));
        }
        Ok(&run[children[0].clone()])
    }
    fn marker(
        run: &str,
        scope: &NamespaceBindings,
        kind: &str,
    ) -> Result<(BTreeMap<String, String>, String)> {
        let raw = run_child(run, scope)?;
        let mut attributes = element(raw, scope, "fldChar", &["fldCharType", "dirty", "fldLock"])?;
        if attributes
            .remove("fldCharType")
            .map(|(_, value)| value)
            .as_deref()
            != Some(kind)
        {
            return Err(Error::Other(
                "conversion marker kind differs from actual carrier geometry".into(),
            ));
        }
        if kind != "begin" && !attributes.is_empty() {
            return Err(Error::Other(
                "conversion non-begin marker has unexpected flags".into(),
            ));
        }
        let data = if element_opening(raw)?.1 {
            String::new()
        } else {
            let inner = element_inner_range_any_prefix(raw, "fldChar")?;
            let children = direct_element_spans(raw)?;
            if kind != "begin"
                || children.len() != 1
                || !raw[inner.start..children[0].start].trim().is_empty()
                || !raw[children[0].end..inner.end].trim().is_empty()
            {
                return Err(Error::Other(
                    "conversion marker has unconsumed owned source".into(),
                ));
            }
            let data = &raw[children[0].clone()];
            let closed = actual_field_source_fragment(data, scope)?;
            let mut reader = NsReader::from_str(&closed);
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|error| Error::Other(error.to_string()))?;
            if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                || !matches!(event, Event::Start(ref root) | Event::Empty(ref root) if root.local_name().as_ref() == b"fldData")
            {
                return Err(Error::Other(
                    "conversion marker data has no actual fldData owner".into(),
                ));
            }
            data.to_owned()
        };
        Ok((
            attributes
                .into_iter()
                .map(|(name, (lexical, _))| (name, lexical))
                .collect(),
            data,
        ))
    }
    // Namespace declarations introduced by closure may differ at a result root.
    // Compare their actual effective bindings, every other root attribute and
    // the complete untouched descendant bytes independently.
    fn result_facts(result: &str, scope: &NamespaceBindings) -> Result<String> {
        let wrapped = format!("<result>{result}</result>");
        let spans = direct_element_spans(&wrapped)?;
        let mut output = String::new();
        let mut cursor = "<result>".len();
        for span in spans {
            output.push_str(&wrapped[cursor..span.start]);
            let (lexical, bindings) = raw_occurrence_representation(&wrapped[span.clone()], scope)?;
            let bindings = bindings
                .into_iter()
                .filter(|(name, uri)| !(name == "xmlns:w" && uri == W_NS))
                .collect::<BTreeMap<_, _>>();
            output.push_str(&format!("{bindings:?}"));
            output.push_str(&lexical);
            cursor = span.end;
        }
        output.push_str(&wrapped[cursor..wrapped.len() - "</result>".len()]);
        Ok(output)
    }
    let scope = actual_root_bindings(source, inherited)?;
    if !actual_field_is_complex(source, inherited)? {
        let mut attributes = element(source, &scope, "fldSimple", &["instr", "dirty", "fldLock"])?;
        let instruction = attributes
            .remove("instr")
            .ok_or_else(|| Error::Other("simple conversion source lacks instruction".into()))?
            .1;
        let expanded = append_word_child(source, "fldSimple", "")?;
        let inner = element_inner_range_any_prefix(&expanded, "fldSimple")?;
        let data = direct_word_element_spans_in_scope(&expanded, "fldData", &scope)?;
        if data.len() > 1
            || data
                .first()
                .is_some_and(|span| !expanded[inner.start..span.start].trim().is_empty())
        {
            return Err(Error::Other(
                "simple conversion source has ambiguous data".into(),
            ));
        }
        let start = data.first().map_or(inner.start, |span| span.end);
        return Ok((
            instruction,
            attributes
                .into_iter()
                .map(|(name, (raw, _))| (name, raw))
                .collect(),
            data.first()
                .map_or(String::new(), |span| expanded[span.clone()].to_owned()),
            result_facts(&expanded[start..inner.end], &scope)?,
        ));
    }
    let wrapped = format!("<carrier>{source}</carrier>");
    let spans = direct_element_spans(&wrapped)?;
    if spans.len() < 4 {
        return Err(Error::Other(
            "converted complex source lacks pure carrier runs".into(),
        ));
    }
    let (flags, data) = marker(&wrapped[spans[0].clone()], &scope, "begin")?;
    let instruction = run_child(&wrapped[spans[1].clone()], &scope)?;
    let attributes = element(instruction, &scope, "instrText", &["xml:space"])?;
    if attributes.get("xml:space").map(|(_, value)| value.as_str()) != Some("preserve") {
        return Err(Error::Other(
            "converted instruction lost its whitespace contract".into(),
        ));
    }
    let inner = element_inner_range_any_prefix(instruction, "instrText")?;
    if !direct_element_spans(instruction)?.is_empty() {
        return Err(Error::Other(
            "converted instruction has unconsumed child source".into(),
        ));
    }
    let instruction = quick_xml::escape::unescape(&instruction[inner])
        .map_err(|error| Error::Other(error.to_string()))?
        .into_owned();
    marker(&wrapped[spans[2].clone()], &scope, "separate")?;
    let last = spans.last().unwrap();
    marker(&wrapped[last.clone()], &scope, "end")?;
    if !wrapped["<carrier>".len()..spans[0].start].trim().is_empty()
        || !wrapped[last.end..wrapped.len() - "</carrier>".len()]
            .trim()
            .is_empty()
        || !wrapped[spans[0].end..spans[1].start].trim().is_empty()
        || !wrapped[spans[1].end..spans[2].start].trim().is_empty()
    {
        return Err(Error::Other(
            "converted carrier has unconsumed outside source".into(),
        ));
    }
    Ok((
        instruction,
        flags,
        data,
        result_facts(&wrapped[spans[2].end..last.start], &scope)?,
    ))
}

/// Map only the qualified clean source roles. This is an encoding change, not
/// Field re-authoring: the XML-normalized attribute value supplies instrText.
fn clean_simple_complex_source(
    xml: &str,
    inherited: &NamespaceBindings,
    field: &oxoffice_docx_oxml::text::Field,
) -> Result<String> {
    let closed = actual_field_source_fragment(xml, inherited)?;
    let scope = actual_root_bindings(xml, inherited)?;
    let mut reader = NsReader::from_str(&closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
        return Err(Error::Other(
            "unconsumed simple-field source root namespace".into(),
        ));
    }
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Err(Error::Other(
            "simple-field conversion has no actual source root".into(),
        ));
    };
    if root.local_name().as_ref() != b"fldSimple" {
        return Err(Error::Other(
            "simple-field conversion source is not fldSimple".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut instruction = None;
    let mut word_prefix = None;
    let mut flags = Vec::new();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let uri = match namespace {
            ResolveResult::Bound(uri) => uri.as_ref().to_vec(),
            ResolveResult::Unbound => Vec::new(),
            ResolveResult::Unknown(prefix) => {
                return Err(Error::Other(format!(
                    "unbound simple-field source attribute prefix {prefix:?}"
                )));
            }
        };
        if !seen.insert((uri.clone(), local.as_ref().to_vec())) {
            return Err(Error::Other(
                "duplicate expanded simple-field source attribute".into(),
            ));
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
            .map_err(|error| Error::Other(error.to_string()))?;
        if uri != W_NS.as_bytes() {
            return Err(Error::Other(format!(
                "unconsumed simple-field root attribute {}",
                String::from_utf8_lossy(attribute.key.as_ref())
            )));
        }
        match local.as_ref() {
            b"instr" => {
                let name = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
                word_prefix = name.split_once(':').map(|(prefix, _)| prefix.to_owned());
                instruction = Some(value.into_owned());
            }
            b"dirty" | b"fldLock" => {
                if !matches!(value.as_ref(), "0" | "1" | "false" | "true" | "off" | "on") {
                    return Err(Error::Other(format!(
                        "unmappable simple-field flag {}={value:?}",
                        String::from_utf8_lossy(local.as_ref())
                    )));
                }
                flags.push((
                    std::str::from_utf8(attribute.key.as_ref())
                        .map_err(utf8_error)?
                        .to_owned(),
                    std::str::from_utf8(attribute.value.as_ref())
                        .map_err(utf8_error)?
                        .to_owned(),
                ));
            }
            _ => {
                return Err(Error::Other(format!(
                    "unconsumed simple-field root attribute {}",
                    String::from_utf8_lossy(attribute.key.as_ref())
                )));
            }
        }
    }
    let prefix = word_prefix.ok_or_else(|| {
        Error::Other("simple-field instruction lacks a qualified actual Word prefix".into())
    })?;
    let instruction = instruction
        .ok_or_else(|| Error::Other("simple-field source instruction is missing".into()))?;
    let expanded = append_word_child(xml, "fldSimple", "")?;
    let inner = element_inner_range_any_prefix(&expanded, "fldSimple")?;
    let data = direct_word_element_spans_in_scope(&expanded, "fldData", &scope)?;
    if data.len() > 1
        || data
            .first()
            .is_some_and(|span| !expanded[inner.start..span.start].trim().is_empty())
    {
        return Err(Error::Other(
            "multiple or late simple-field fldData source owners".into(),
        ));
    }
    let result_start = data.first().map_or(inner.start, |span| span.end);
    let result = &expanded[result_start..inner.end];
    let result = qualified_complete_field_payload(result, &scope, false, false, true, &mut 0)?;
    let mut namespace_root = BytesStart::new(format!("{prefix}:r"));
    for (name, uri) in &scope {
        namespace_root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(namespace_root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let run_open = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    let run_close = format!("</{prefix}:r>");
    let mut begin = format!("<{prefix}:fldChar {prefix}:fldCharType=\"begin\"");
    for (name, value) in flags {
        begin.push_str(&format!(" {name}=\"{value}\""));
    }
    if let Some(span) = data.first() {
        begin.push('>');
        begin.push_str(&expanded[span.clone()]);
        begin.push_str(&format!("</{prefix}:fldChar>"));
    } else {
        begin.push_str("/>");
    }
    let output = format!(
        "{run_open}{begin}{run_close}{run_open}<{prefix}:instrText xml:space=\"preserve\">{}</{prefix}:instrText>{run_close}{run_open}<{prefix}:fldChar {prefix}:fldCharType=\"separate\"/>{run_close}{result}{run_open}<{prefix}:fldChar {prefix}:fldCharType=\"end\"/>{run_close}",
        quick_xml::escape::escape(&instruction)
    );
    // An independent read of emitted source must actually group a root Field.
    // No witness is allowed to fill a missing parsed owner or cached display.
    let root = format!("<w:p xmlns:w=\"{W_NS}\">{output}</w:p>");
    let model = story_document(&root, &Vec::new())?;
    let Some(BodyContent::Paragraph(paragraph)) = model.body.content.first() else {
        return Err(Error::Other(
            "converted source has no actual root paragraph".into(),
        ));
    };
    let fields = paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if fields.len() != 1
        || !fields[0].is_complex()
        || fields[0].effective_instruction().name != field.effective_instruction().name
        || fields[0].effective_instruction().arguments != field.effective_instruction().arguments
        || fields[0].effective_instruction().switches != field.effective_instruction().switches
        || fields[0].cached_result != field.cached_result
        || fields[0].dirty != field.dirty
        || fields[0].locked() != field.locked()
    {
        return Err(Error::Other(
            "converted source did not reconstruct its actual root field instruction/cache/flags"
                .into(),
        ));
    }
    Ok(output)
}

/// Qualify an ordinary whole-field run-revision payload at its actual owners.
/// Direct runs use the existing qualified child scanner. Opaque run payloads
/// and real nested control/wrapper custody retain their own source authority.
fn qualified_complete_field_payload(
    source: &str,
    scope: &NamespaceBindings,
    deleted: bool,
    paragraph_content: bool,
    close_roots: bool,
    field_depth: &mut usize,
) -> Result<String> {
    let initial_depth = *field_depth;
    let mut root = BytesStart::new("fieldPayload");
    for (name, uri) in scope {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let opening = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    let wrapped = format!("{opening}{source}</fieldPayload>");
    let mut output = String::new();
    let mut cursor = opening.len();
    for span in direct_element_spans(&wrapped)? {
        if !wrapped[cursor..span.start].trim().is_empty() {
            return Err(Error::Other(
                "unconsumed direct field-result source event".into(),
            ));
        }
        output.push_str(&wrapped[cursor..span.start]);
        let raw = &wrapped[span.clone()];
        let closed = actual_field_source_fragment(raw, scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let (Event::Start(element) | Event::Empty(element)) = event else {
            return Err(Error::Other("field-result source child is missing".into()));
        };
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
            return Err(Error::Other(format!(
                "unproved direct field-result owner {}",
                String::from_utf8_lossy(element.name().as_ref())
            )));
        }
        let local = element.local_name();
        if !(paragraph_content && matches!(local.as_ref(), b"fldSimple" | b"hyperlink"))
            && !matches!(
                local.as_ref(),
                b"r" | b"sdt"
                    | b"customXml"
                    | b"smartTag"
                    | b"dir"
                    | b"bdo"
                    | b"proofErr"
                    | b"permStart"
                    | b"permEnd"
                    | b"bookmarkStart"
                    | b"bookmarkEnd"
                    | b"commentRangeStart"
                    | b"commentRangeEnd"
                    | b"moveFromRangeStart"
                    | b"moveFromRangeEnd"
                    | b"moveToRangeStart"
                    | b"moveToRangeEnd"
                    | b"customXmlInsRangeStart"
                    | b"customXmlInsRangeEnd"
                    | b"customXmlDelRangeStart"
                    | b"customXmlDelRangeEnd"
                    | b"customXmlMoveFromRangeStart"
                    | b"customXmlMoveFromRangeEnd"
                    | b"customXmlMoveToRangeStart"
                    | b"customXmlMoveToRangeEnd"
                    | b"ins"
                    | b"del"
                    | b"moveFrom"
                    | b"moveTo"
            )
        {
            return Err(Error::Other(format!(
                "unproved whole-field run-revision child {}",
                String::from_utf8_lossy(element.name().as_ref())
            )));
        }
        let local = local.as_ref().to_vec();
        let child_scope = actual_root_bindings(raw, scope)?;
        let mut child = if close_roots { closed } else { raw.to_owned() };
        if local == b"sdt" {
            let children = direct_element_spans(&child)?;
            let mut previous = None;
            let mut seen = HashSet::new();
            for span in children.into_iter().rev() {
                let raw = &child[span.clone()];
                let scoped = actual_field_source_fragment(raw, &child_scope)?;
                let mut reader = NsReader::from_str(&scoped);
                let (namespace, event) = reader
                    .read_resolved_event()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let (Event::Start(element) | Event::Empty(element)) = event else {
                    return Err(Error::Other("control source child is missing".into()));
                };
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    return Err(Error::Other(
                        "unproved foreign direct inline-control child".into(),
                    ));
                }
                let role = match element.local_name().as_ref() {
                    b"sdtPr" => 0,
                    b"sdtEndPr" => 1,
                    b"sdtContent" => 2,
                    _ => {
                        return Err(Error::Other(
                            "unproved block/opaque inline-control child".into(),
                        ));
                    }
                };
                if !seen.insert(role) || previous.is_some_and(|next| role >= next) {
                    return Err(Error::Other(
                        "inline-control source order or multiplicity is unproved".into(),
                    ));
                }
                previous = Some(role);
                if role == 2 && !element_opening(raw)?.1 {
                    let content_scope = actual_root_bindings(raw, &child_scope)?;
                    let content = element_inner_range_any_prefix(raw, "sdtContent")?;
                    let replacement = qualified_complete_field_payload(
                        &raw[content.clone()],
                        &content_scope,
                        deleted,
                        true,
                        false,
                        field_depth,
                    )?;
                    let mut current = raw.to_owned();
                    current.replace_range(content, &replacement);
                    child.replace_range(span, &current);
                }
            }
        } else if matches!(
            local.as_slice(),
            b"customXml"
                | b"smartTag"
                | b"dir"
                | b"bdo"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"hyperlink"
                | b"fldSimple"
        ) && !element_opening(&child)?.1
        {
            let name = std::str::from_utf8(&local).map_err(utf8_error)?;
            let inner = element_inner_range_any_prefix(&child, name)?;
            let property = match local.as_slice() {
                b"customXml" => Some("customXmlPr"),
                b"smartTag" => Some("smartTagPr"),
                b"fldSimple" => Some("fldData"),
                _ => None,
            };
            let mut start = inner.start;
            if let Some(property) = property {
                let properties =
                    direct_word_element_spans_in_scope(&child, property, &child_scope)?;
                if properties.len() > 1
                    || properties
                        .first()
                        .is_some_and(|span| !child[start..span.start].trim().is_empty())
                {
                    return Err(Error::Other(format!(
                        "unproved {name} property order or multiplicity"
                    )));
                }
                if let Some(property) = properties.first() {
                    start = property.end;
                }
            }
            let paragraph = !matches!(local.as_slice(), b"ins" | b"del" | b"moveFrom" | b"moveTo");
            let replacement = qualified_complete_field_payload(
                &child[start..inner.end],
                &child_scope,
                deleted && local != b"fldSimple",
                paragraph,
                false,
                field_depth,
            )?;
            child.replace_range(start..inner.end, &replacement);
        }
        if local == b"r" {
            for forbidden in [
                "fldSimple",
                "hyperlink",
                "sdt",
                "p",
                "tbl",
                "ins",
                "del",
                "moveFrom",
                "moveTo",
            ] {
                if !direct_word_element_spans_in_scope(&child, forbidden, &child_scope)?.is_empty()
                {
                    return Err(Error::Other(format!(
                        "unproved {forbidden} inside an actual result run"
                    )));
                }
            }
        }
        if local == b"r" {
            let mut renames = Vec::new();
            for direct in direct_element_spans(&child)? {
                let raw = &child[direct.clone()];
                let scoped = actual_field_source_fragment(raw, &child_scope)?;
                let mut reader = NsReader::from_str(&scoped);
                let (namespace, event) = reader
                    .read_resolved_event()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let (Event::Start(element) | Event::Empty(element)) = event else {
                    continue;
                };
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    continue;
                }
                let local = element.local_name();
                if local.as_ref() == b"fldChar" {
                    let mut kind = None;
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                            && name.as_ref() == b"fldCharType"
                        {
                            kind = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?
                                    .into_owned(),
                            );
                        }
                    }
                    match kind.as_deref() {
                        Some("begin") => {
                            *field_depth = field_depth.checked_add(1).ok_or_else(|| {
                                Error::Other("field deletion depth overflow".into())
                            })?
                        }
                        Some("end") => {
                            *field_depth = field_depth.checked_sub(1).ok_or_else(|| {
                                Error::Other("field deletion has an unmatched end".into())
                            })?
                        }
                        Some("separate") if *field_depth > 0 => {}
                        _ => {
                            return Err(Error::Other(
                                "unproved field marker ownership during complete-span publication"
                                    .into(),
                            ));
                        }
                    }
                } else if deleted && *field_depth == 1 {
                    let target = match local.as_ref() {
                        b"t" => Some("delText"),
                        b"instrText" => Some("delInstrText"),
                        _ => None,
                    };
                    if let Some(target) = target {
                        renames.push((direct, target));
                    }
                }
            }
            for (text, target) in renames.into_iter().rev() {
                let raw = &child[text.clone()];
                let (_, empty, name) = element_opening(raw)?;
                let target = name
                    .rsplit_once(':')
                    .map_or(target.to_owned(), |(prefix, _)| {
                        format!("{prefix}:{target}")
                    });
                let mut renamed = raw.to_owned();
                if !empty {
                    let close = paragraph_close_start(raw)?;
                    renamed.replace_range(close + 2..close + 2 + name.len(), &target);
                }
                renamed.replace_range(1..1 + name.len(), &target);
                child.replace_range(text, &renamed);
            }
        }
        output.push_str(&child);
        cursor = span.end;
    }
    let end = wrapped.len() - "</fieldPayload>".len();
    if !wrapped[cursor..end].trim().is_empty() {
        return Err(Error::Other(
            "unconsumed trailing field-result source event".into(),
        ));
    }
    output.push_str(&wrapped[cursor..end]);
    if *field_depth != initial_depth {
        return Err(Error::Other(
            "field markers cross their actual enclosing source owner".into(),
        ));
    }
    Ok(output)
}

fn field_xml(field: &oxoffice_docx_oxml::text::Field) -> Result<String> {
    paragraph_owned_run_xml(&CT_R {
        properties: None,
        content: vec![RunContent::Field(field.clone())],
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        alt_drawings: Vec::new(),
    })
}

fn prepared_field_result_owner(
    field: &oxoffice_docx_oxml::text::Field,
    source: &str,
) -> Result<CT_P> {
    if let Some(owner) = field.cached_result_owner_projection()? {
        return Ok(owner);
    }
    if field.cached_result.is_empty() && source.as_bytes().iter().all(u8::is_ascii_whitespace) {
        return Ok(CT_P::default());
    }
    let runs = field.cached_result_physical_runs().ok_or_else(|| {
        Error::Other(
            "field result has neither a current ordered owner nor a proved contiguous run owner"
                .into(),
        )
    })?;
    Ok(CT_P {
        runs: runs.to_vec(),
        ..Default::default()
    })
}

fn field_result_source_paragraph(source: &str, scope: &NamespaceBindings) -> Result<String> {
    let prefix = scope
        .iter()
        .find(|(_, uri)| uri == W_NS)
        .map(|(name, _)| name.strip_prefix("xmlns:").unwrap_or(""))
        .ok_or_else(|| Error::Other("field result has no actual Word namespace binding".into()))?;
    let name = if prefix.is_empty() {
        "p".to_owned()
    } else {
        format!("{prefix}:p")
    };
    let mut root = BytesStart::new(name.as_str());
    for (name, uri) in scope {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    writer.get_mut().extend_from_slice(source.as_bytes());
    writer
        .write_event(Event::End(quick_xml::events::BytesEnd::new(name.as_str())))
        .map_err(|error| Error::Other(error.to_string()))?;
    String::from_utf8(writer.into_inner()).map_err(utf8_error)
}

fn tracked_field_result(
    fields: [&oxoffice_docx_oxml::text::Field; 2],
    original: &str,
    edited: &str,
    scopes: [&NamespaceBindings; 2],
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let left_model = prepared_field_result_owner(fields[0], original)?;
    let right_model = prepared_field_result_owner(fields[1], edited)?;
    let left = field_result_source_paragraph(original, scopes[0])?;
    let right = field_result_source_paragraph(edited, scopes[1])?;
    let outer = fields
        .iter()
        .map(|field| {
            metadata
                .source_frames
                .iter()
                .rev()
                .find_map(|frame| {
                    frame
                        .field_occurrences
                        .get(&(std::ptr::from_ref(*field) as usize))
                })
                .cloned()
                .ok_or_else(|| {
                    Error::Other("field result has no actual outer source occurrence".into())
                })
        })
        .collect::<Result<Vec<_>>>()?;
    if outer[0].input_role != 0 || outer[1].input_role != 1 || outer[0].story != outer[1].story {
        return Err(Error::Other(
            "field result outer occurrence association changed".into(),
        ));
    }
    let mut result_sources = SourceSnapshots {
        story: outer[0].story.clone(),
        ..Default::default()
    };
    for (role, (model, source)) in [(&left_model, &left), (&right_model, &right)]
        .into_iter()
        .enumerate()
    {
        result_sources.input_role = role;
        let key = std::ptr::from_ref(model) as usize;
        let mut path = outer[role].owner_path.clone();
        path.push((7, outer[role].field_slot));
        result_sources.owner_paths.insert(key, path);
        if !fields[role].cached_fields_in_source_order().is_empty() {
            let result = if role == 0 { original } else { edited };
            match actual_field_result_range(&outer[role].source, &outer[role].scope) {
                Ok(interval) if outer[role].source.get(interval.clone()) == Some(result) => {
                    let mut ancestors = outer[role].result_ancestors.clone();
                    ancestors.push(interval.clone());
                    result_sources.result_ancestors.insert(key, ancestors);
                    result_sources
                        .result_origins
                        .insert(key, (interval, element_opening(source)?.0));
                }
                _ => {
                    result_sources.unproved_result_owners.insert(key);
                }
            }
        }
        result_sources
            .roots
            .insert(key, (source.clone(), scopes[role].clone()));
        result_sources.paragraph(model, source)?;
    }
    let mut options = metadata.options.clone();
    // Cached-result properties are owned by the Field, before the ordinary
    // paragraph ignore-formatting policy. The actual legal child owners recurse
    // through the existing qualified paragraph/control/hyperlink publishers.
    options.ignore_formatting = false;
    let mut inner = Metadata {
        author: metadata.author,
        timestamp: metadata.timestamp,
        options: &options,
        ids: metadata.ids.clone(),
        existing_revisions: metadata.existing_revisions,
        source_frames: vec![result_sources],
        projected_textbox_noop: false,
        leaf_capture: false,
        paired_field_story: metadata
            .paired_field_story
            .as_ref()
            .filter(|story| outer[0].story == **story && outer[1].story == **story)
            .cloned(),
        leaf_publications: Vec::new(),
        field_conversions: Vec::new(),
    };
    let mut diagnostics = Vec::new();
    let result = compare_paragraph(
        &left_model,
        &right_model,
        Some(&left),
        &format!("{}/field[{}]/result", outer[0].story, outer[0].field_slot),
        &mut inner,
        &mut diagnostics,
    )?;
    metadata.ids = inner.ids;
    metadata.field_conversions.extend(inner.field_conversions);
    let inner_range = element_inner_range_any_prefix(&result, "p")?;
    Ok(result[inner_range].to_owned())
}

/// Locate an unchanged physical result interval without splitting producer runs.
/// Only complete pure marker geometry supplies a nested occurrence anchor.
fn actual_field_result_range(source: &str, inherited: &NamespaceBindings) -> Result<Range<usize>> {
    let scope = actual_root_bindings(source, inherited)?;
    if !actual_field_is_complex(source, inherited)? {
        if element_opening(source)?.1 {
            return Err(Error::Other(
                "empty field root has no physical result interval".into(),
            ));
        }
        let inner = element_inner_range_any_prefix(source, "fldSimple")?;
        let data = direct_word_element_spans_in_scope(source, "fldData", &scope)?;
        if data.len() > 1
            || data
                .first()
                .is_some_and(|span| !source[inner.start..span.start].trim().is_empty())
        {
            return Err(Error::Other(
                "field result data boundary is ambiguous".into(),
            ));
        }
        return Ok(data.first().map_or(inner.start, |span| span.end)..inner.end);
    }
    let opening = "<fieldSource>";
    let wrapped = format!("{opening}{source}</fieldSource>");
    let spans = direct_element_spans(&wrapped)?;
    let mut depth = 0usize;
    let mut start = None;
    let mut end = None;
    for (index, span) in spans.iter().enumerate() {
        let run = &wrapped[span.clone()];
        let run_scope = actual_root_bindings(run, &scope)?;
        let closed = actual_field_source_fragment(run, &scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            || !matches!(event, Event::Start(ref root) if root.local_name().as_ref() == b"r")
        {
            continue;
        }
        let children = direct_element_spans(run)?;
        let properties = direct_word_element_spans_in_scope(run, "rPr", &run_scope)?;
        let inner = element_inner_range_any_prefix(run, "r")?;
        let mut cursor = inner.start;
        let mut trivia = true;
        for child in &children {
            trivia &= run[cursor..child.start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
            cursor = child.end;
        }
        trivia &= run[cursor..inner.end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
        let pure = trivia
            && properties.len() <= 1
            && children.len() == 1 + properties.len()
            && properties
                .first()
                .is_none_or(|property| children.first() == Some(property));
        for child in &children {
            let raw = actual_field_source_fragment(&run[child.clone()], &run_scope)?;
            let mut reader = NsReader::from_str(&raw);
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|error| Error::Other(error.to_string()))?;
            let (Event::Start(root) | Event::Empty(root)) = event else {
                continue;
            };
            if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                || root.local_name().as_ref() != b"fldChar"
            {
                continue;
            }
            let mut kind = None;
            for attribute in root.attributes() {
                let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                    && name.as_ref() == b"fldCharType"
                {
                    if kind.is_some() {
                        return Err(Error::Other("duplicate physical field marker kind".into()));
                    }
                    kind = Some(
                        attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
                            .map_err(|error| Error::Other(error.to_string()))?
                            .into_owned(),
                    );
                }
            }
            match kind.as_deref() {
                Some("begin") => {
                    if depth == 0 && (index != 0 || !pure) {
                        return Err(Error::Other(
                            "nested result anchor has a shared or partial begin run".into(),
                        ));
                    }
                    depth = depth
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("physical field depth overflow".into()))?;
                }
                Some("separate") if depth == 1 => {
                    if start.is_some() || !pure {
                        return Err(Error::Other(
                            "nested result anchor has a shared or duplicate separate run".into(),
                        ));
                    }
                    start = Some(span.end - opening.len());
                }
                Some("separate") if depth > 1 => {}
                Some("end") if depth > 0 => {
                    if depth == 1 {
                        if end.is_some() || !pure || index + 1 != spans.len() {
                            return Err(Error::Other(
                                "nested result anchor has a shared or partial end run".into(),
                            ));
                        }
                        end = Some(span.start - opening.len());
                    }
                    depth -= 1;
                }
                _ => {
                    return Err(Error::Other(
                        "unbalanced qualified physical field marker".into(),
                    ));
                }
            }
        }
    }
    let range = start
        .zip(end)
        .filter(|(start, end)| start <= end && depth == 0)
        .map(|(start, end)| start..end)
        .ok_or_else(|| {
            Error::Other("field has no complete proved physical result interval".into())
        })?;
    if !source[..spans.first().unwrap().start - opening.len()]
        .trim()
        .is_empty()
        || !source[spans.last().unwrap().end - opening.len()..]
            .trim()
            .is_empty()
    {
        return Err(Error::Other(
            "field anchor has independent outside source".into(),
        ));
    }
    Ok(range)
}

fn complex_field_result(xml: &str) -> Result<(String, String, String)> {
    let separate = field_character(xml, 0, "separate")
        .ok_or_else(|| Error::Other("complex field source has no separate boundary".to_owned()))?;
    // A producer may pack a whole field in one run, as Google Docs writes page
    // fields. Ending a run after `separate` and starting one at `end` reads it
    // as the same field written one run per part.
    let xml = split_field_run(xml, separate, true)?;
    let (_, result_start) = containing_run(&xml, separate)?;
    let end_marker = field_character(&xml, result_start, "end")
        .ok_or_else(|| Error::Other("complex field source has no end boundary".to_owned()))?;
    let xml = split_field_run(&xml, end_marker, false)?;
    // The split may have moved the `end` character further along.
    let end_marker = field_character(&xml, result_start, "end")
        .ok_or_else(|| Error::Other("complex field source has no end boundary".to_owned()))?;
    let (result_end, _) = containing_run(&xml, end_marker)?;
    Ok((
        xml[..result_start].to_owned(),
        xml[result_start..result_end].to_owned(),
        xml[result_end..].to_owned(),
    ))
}

fn field_character(xml: &str, from: usize, kind: &str) -> Option<usize> {
    let tail = &xml[from..];
    tail.find(&format!("fldCharType=\"{kind}\""))
        .or_else(|| tail.find(&format!("fldCharType='{kind}'")))
        .map(|offset| from + offset)
}

/// Split the run holding the field character at `marker` so that the
/// character ends its run (`after`) or starts it.
///
/// Both runs repeat the original start tag and run properties. A run with no
/// content on that side of the character is returned unchanged.
fn split_field_run(xml: &str, marker: usize, after: bool) -> Result<String> {
    let (run_start, run_end) = containing_run(xml, marker)?;
    let run = &xml[run_start..run_end];
    let children = direct_element_spans(run)?;
    let character = children
        .iter()
        .position(|child| child.contains(&(marker - run_start)))
        .ok_or_else(|| Error::Other("complex field character is not a run child".to_owned()))?;
    let is_properties = |child: &Range<usize>| {
        let mut reader = Reader::from_reader(run[child.clone()].as_bytes());
        matches!(
            reader.read_event(),
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"rPr"
        )
    };
    let first_content = usize::from(children.first().is_some_and(is_properties));
    let split = if after { character + 1 } else { character };
    if split <= first_content || split >= children.len() {
        return Ok(xml.to_owned());
    }
    let head = &run[..children[first_content].start];
    let close = run
        .rfind("</")
        .map(|at| &run[at..])
        .ok_or_else(|| Error::Other("complex field run has no end tag".to_owned()))?;
    let at = run_start + children[split].start;
    Ok(format!("{}{close}{head}{}", &xml[..at], &xml[at..]))
}

fn containing_run(xml: &str, at: usize) -> Result<(usize, usize)> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(usize, bool)>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                stack.push((before, element.local_name().as_ref() == b"r"));
            }
            Event::End(_) => {
                let Some((start, run)) = stack.pop() else {
                    return Err(Error::Other("complex field XML is unbalanced".to_owned()));
                };
                if run && start <= at && at < after {
                    return Ok((start, after));
                }
            }
            Event::Empty(_) => {}
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Err(Error::Other(
        "complex field boundary has no owning run".to_owned(),
    ))
}

fn formatting_diagnostic(diagnostics: &mut Vec<ComparisonDiagnostic>, location: String) {
    diagnostics.push(ComparisonDiagnostic {
        location,
        message: "formatting differs and the original formatting was retained".to_owned(),
    });
}

fn body_location(location: &str, content: &BodyContent, index: usize) -> String {
    match content {
        BodyContent::Paragraph(_) => format!("{location}/paragraph[{index}]"),
        BodyContent::Table(_) => format!("{location}/table[{index}]"),
        BodyContent::ContentControl(_) => format!("{location}/content-control[{index}]"),
        BodyContent::RawXml(_) => format!("{location}/raw[{index}]"),
    }
}

/// The paragraphs, by index on each side, that a replaced run deletes or
/// inserts whole inside the complex fields that enclose their hyperlinks
/// and simple fields.
#[derive(Default)]
struct CarriedParagraphs {
    original: HashSet<usize>,
    edited: HashSet<usize>,
}

fn run_is_field(run: &CT_R) -> bool {
    matches!(run.content.as_slice(), [RunContent::Field(_)])
}

fn run_field(run: &CT_R) -> Option<&oxoffice_docx_oxml::text::Field> {
    match run.content.as_slice() {
        [RunContent::Field(field)] => Some(field),
        _ => None,
    }
}
