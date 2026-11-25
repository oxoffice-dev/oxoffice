//! Read-only content-control handles and atomic value binding.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use oxoffice_docx_oxml::CT_RPr;
use oxoffice_docx_oxml::content_control::{CT_DataBinding, CT_Sdt, SdtContent, SdtType};
use oxoffice_docx_oxml::date_format::{WordDateTime, check_word_date_format};
use oxoffice_docx_oxml::document::{BodyContent, CT_Body};
use oxoffice_docx_oxml::namespace::{W_NS, matches_local_name};
use oxoffice_docx_oxml::styles::{CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::{BreakClear, BreakType, CT_P, CT_R, CT_Text, RunContent};
use oxoffice_opc::OpcPackage;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::{NsReader, Reader};

use crate::{
    ContentFragment, ContentLocation, Document, Error, ParagraphRef, Result, StoryItemKind,
    StoryKind,
};

mod control_patch;
mod custom_xml;
mod data_binding;
mod display;
mod element_index;
mod form_in_place;
pub use form_in_place::FormEdit;
mod form_issues;
mod form_plan;
mod form_rules;
mod form_state;
mod in_place;
mod namespaces;
mod placement;
mod properties;
mod repeating;
mod shells;
mod structure;
mod type_change;
mod values;

use control_patch::{FullDateEdit, LastValueEdit, apply_control_patch};
pub(crate) use custom_xml::{
    custom_xml_store_item_count, normalize_store_item_id, parse_store_item_id,
    resolve_custom_xml_part, validate_store_item,
};
use custom_xml::{decode_one_xml_reference, is_xml_whitespace};
use data_binding::{
    apply_xml_edits, expand_empty_element, parse_prefix_mappings, parse_xpath,
    resolve_binding_edit, select_xml_element,
};
use display::{
    check_dropped_change, collect_sdt_text, drop_content_changes, orphaned_partners, proofing_mark,
    replace_document_controls, value_content,
};
use element_index::{
    ancestor_local, direct_child, direct_children, element_at, element_qname, expand_empty,
    index_elements, insert_into_element, is_w14, is_word, nth_direct_child, table_column_count,
};
use form_issues::{current_issues, date_format_issue, date_metadata_unsupported};
use form_plan::{
    LegacyWrite, LogicalPatch, apply_form_plan, plan_fill, plan_reset, prepare_control_text,
    preview_bindings,
};
use form_rules::{check_value, locate_binding, select_rules};
use form_state::{
    ControlKind, Owner, accepted_display_change, accepted_display_mark_change,
    accepted_display_paragraph, accepted_display_text, element_text, flushed_form_document,
    publish_form_document, read_form_state,
};
use namespaces::{
    AttrNs, in_scope_declarations, in_scope_xmlns, insert_into_start_tag, qualified_attributes,
    rewrite_qualified_attribute,
};
use placement::{
    ParentSlot, control_parent_is_block, destination_accepts_control, ensure_cell_paragraph,
    nested_destination, parent_can_hold, parent_slot, ranges_overlap,
    refuse_cell_without_paragraph, sdt_payload_matches, shift_after_removal,
    transparent_children_match, xml_space,
};
use properties::{
    PropertyEdit, edit_sdt_property, name_prefix, prefixed_val, property_rank, splice_absolute,
};
use repeating::{
    RepeatingContent, ancestor_repeating_section, control_matches_context,
    first_repeating_item_range, freshen_control, last_repeating_item, repeating_content,
    repeating_slot_allowed, sdt_content_elem, sdt_is_repeating_item, sdt_is_repeating_section,
    story_inside_repeating_section, unwrap_keeps_repeating_context, validate_repeating_type_change,
    wrap_repeating_item,
};
use shells::{
    cell_shell, control_shell, control_shell_with_body, parse_control, plain_or_rich,
    reject_control_text, row_shell, shell_contains_paragraph, xml_attr,
};
use structure::{
    ControlRewrite, control_fault, control_item, paragraph_parent_range, row_parent_range,
    splice_inside, table_parent_range,
};
use type_change::{
    content_embeds_relationship, is_type_child, part_owns_image, replace_control_type_markup,
    type_markup, validate_type_payload,
};
use values::{placeholder_style_ids, story_controls};

pub(crate) use in_place::declares_only_word_extensions;
pub use in_place::{ContentControlEdit, ControlBlock};

/// Read-only view of one structured document tag in document order.
///
/// A control of the body borrows the document's model. One of another story
/// (a header, a footer, a note, a comment or a text box) is read from its part
/// when listed, and the view holds that copy.
#[derive(Debug, Clone)]
pub struct ContentControlRef<'a> {
    pub(crate) inner: ControlSource<'a>,
}

/// Where a [`ContentControlRef`] reads its control.
#[derive(Debug, Clone)]
pub(crate) enum ControlSource<'a> {
    /// The document's model holds it.
    Borrowed(&'a CT_Sdt),
    /// A copy read from a story part.
    Owned(Box<CT_Sdt>),
}

impl std::ops::Deref for ControlSource<'_> {
    type Target = CT_Sdt;

    fn deref(&self) -> &CT_Sdt {
        match self {
            Self::Borrowed(control) => control,
            Self::Owned(control) => control,
        }
    }
}

impl<'a> ContentControlRef<'a> {
    /// A view of a control the document's model holds.
    pub(crate) fn borrowed(inner: &'a CT_Sdt) -> Self {
        Self {
            inner: ControlSource::Borrowed(inner),
        }
    }
}

impl ContentControlRef<'_> {
    pub fn tag(&self) -> Option<&str> {
        self.inner
            .properties
            .as_ref()
            .and_then(|properties| properties.tag.as_deref())
    }

    pub fn alias(&self) -> Option<&str> {
        self.inner
            .properties
            .as_ref()
            .and_then(|properties| properties.alias.as_deref())
    }

    pub fn id(&self) -> Option<i32> {
        self.inner
            .properties
            .as_ref()
            .and_then(|properties| properties.id)
    }

    pub fn control_type(&self) -> Option<SdtType> {
        self.inner
            .properties
            .as_ref()
            .and_then(|properties| properties.control_type)
    }

    /// The control's text: one line per paragraph of the control and of the
    /// block controls nested in it, each read as the document reads a
    /// paragraph, its inline controls included. A table, a row or a cell
    /// reads as its cells' text run together, each cell's paragraphs on
    /// lines of their own.
    pub fn text(&self) -> String {
        let mut text = String::new();
        collect_sdt_text(&self.inner, &mut text);
        text
    }
}

#[derive(Clone)]
struct ControlValue {
    display: String,
    bound: String,
    checkbox: Option<(bool, String)>,
    full_date: Option<String>,
}

impl Document {
    /// Return every typed content control: those of the body in document
    /// order, nested controls included, then those of the other stories, as
    /// [`Self::fill_form`] finds them: the body's text boxes, then the
    /// headers, footers, notes and comments in story order, with their text
    /// boxes. When the other stories cannot be read, the body's controls
    /// alone are listed; the setters then return the error.
    pub fn content_controls(&self) -> Vec<ContentControlRef<'_>> {
        let mut controls: Vec<ContentControlRef<'_>> = self
            .document
            .body
            .content_controls()
            .into_iter()
            .map(ContentControlRef::borrowed)
            .collect();
        if let Ok(stories) = flushed_form_document(self).and_then(|staged| story_controls(&staged))
        {
            controls.extend(stories.into_iter().map(|story| ContentControlRef {
                inner: ControlSource::Owned(Box::new(story.control)),
            }));
        }
        controls
    }

    /// Return every content control whose tag equals `tag`.
    pub fn content_controls_by_tag(&self, tag: &str) -> Vec<ContentControlRef<'_>> {
        self.content_controls()
            .into_iter()
            .filter(|control| control.tag() == Some(tag))
            .collect()
    }

    /// Return every content control whose alias equals `alias`.
    pub fn content_controls_by_alias(&self, alias: &str) -> Vec<ContentControlRef<'_>> {
        self.content_controls()
            .into_iter()
            .filter(|control| control.alias() == Some(alias))
            .collect()
    }

    /// Replace the display and bound custom XML value of controls with `tag`,
    /// in every story [`Self::content_controls`] lists. Two selected controls
    /// of which one holds the other are refused, as one value would
    /// overwrite the other.
    ///
    /// The value replaces the control's content as an edit made with track
    /// changes off: the tracked insertions, deletions and moves in it go with
    /// it, its paragraph marks' included, and the control holds the value and
    /// no pending change. A proofing mark paired with one in such a change
    /// goes too. The changes of properties, the control's own included, and
    /// of a nested control stay. A change holding a comment or note mark or a
    /// content control, one end of a bookmark, of a tracked range or of a
    /// proofing mark, or part of a complex field, whose rest is outside it, is
    /// refused: accept or reject it first.
    pub fn set_content_control_value_by_tag(&mut self, tag: &str, value: &str) -> Result<usize> {
        self.apply_content_control_values(|control| {
            (control.tag() == Some(tag)).then(|| value.to_owned())
        })
    }

    /// Fill distinct tags atomically, refusing missing tags and invalid late owners.
    /// Aliases do not participate. Counts retain the caller's order.
    pub fn fill_content_control_tags(&mut self, pairs: &[(&str, &str)]) -> Result<Vec<usize>> {
        let mut seen = std::collections::HashSet::new();
        for &(tag, value) in pairs {
            if !seen.insert(tag) {
                return Err(Error::Other(format!(
                    "duplicate content-control tag {tag:?}"
                )));
            }
            oxoffice_core::xml::reject_non_xml_characters("content-control value", value)?;
        }
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let mut candidate = self.clone_for_staging();
        let mut counts = Vec::with_capacity(pairs.len());
        for &(tag, value) in pairs {
            let count = candidate.set_content_control_value_by_tag(tag, value)?;
            if count == 0 {
                return Err(Error::Other(format!("no content control has tag {tag:?}")));
            }
            counts.push(count);
        }
        self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        Ok(counts)
    }

    /// Replace the display and bound custom XML value of controls with `alias`,
    /// in every story, as [`Self::set_content_control_value_by_tag`] does.
    pub fn set_content_control_value_by_alias(
        &mut self,
        alias: &str,
        value: &str,
    ) -> Result<usize> {
        self.apply_content_control_values(|control| {
            (control.alias() == Some(alias)).then(|| value.to_owned())
        })
    }

    /// Apply values by tag first, then by alias when the tag has no map entry.
    pub fn bind_content_controls(&mut self, values: &HashMap<String, String>) -> Result<usize> {
        self.apply_content_control_values(|control| {
            control
                .tag()
                .and_then(|tag| values.get(tag))
                .or_else(|| control.alias().and_then(|alias| values.get(alias)))
                .cloned()
        })
    }
}

/// A typed content control outside the body's model: in a text box of the
/// body, or in a header, a footer, a note or a comment, with its text boxes.
/// `start..end` is its span in `part`, in the package of the flushed
/// document it was read from.
struct StoryControl {
    part: String,
    start: usize,
    end: usize,
    control: CT_Sdt,
}

/// The values to write, by control index in document order, and the ids of
/// the Placeholder Text style, which marks a prompt's run.
struct ControlReplacements {
    values: HashMap<usize, ControlValue>,
    placeholder_styles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XmlEdit {
    start: usize,
    end: usize,
    replacement: Vec<u8>,
}

#[derive(Debug, Clone)]
struct XmlSelection {
    replace_start: usize,
    replace_end: usize,
    element_start: usize,
    element_end: usize,
    qualified_name: Vec<u8>,
    empty: bool,
}

const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15_NS: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

/// Where a new content control is inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentControlPlacement {
    Block,
    Row,
    Cell,
    Paragraph,
    Inline,
}

/// Lock stored on `w:lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentControlLock {
    Unlocked,
    SdtLocked,
    ContentLocked,
    SdtContentLocked,
}

impl ContentControlLock {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unlocked => "unlocked",
            Self::SdtLocked => "sdtLocked",
            Self::ContentLocked => "contentLocked",
            Self::SdtContentLocked => "sdtContentLocked",
        }
    }
}

/// Appearance stored on `w15:appearance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentControlAppearance {
    BoundingBox,
    Tags,
    Hidden,
}

impl ContentControlAppearance {
    fn as_str(self) -> &'static str {
        match self {
            Self::BoundingBox => "boundingBox",
            Self::Tags => "tags",
            Self::Hidden => "hidden",
        }
    }
}

/// Type payload written onto an existing rich or plain shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentControlTypePayload {
    RichText,
    PlainText,
    /// A date control showing its date in the Word date pattern `format`
    /// (`d MMMM yyyy`, `dd/MM/yyyy`, ...), with English names: the control is
    /// written without a language.
    Date {
        format: String,
    },
    CheckBox,
    ComboBox {
        items: Vec<(String, String)>,
    },
    DropDownList {
        items: Vec<(String, String)>,
    },
    Picture {
        relationship_id: String,
    },
    Group,
    RepeatingSection,
    RepeatingSectionItem,
}

/// Fields for one rich-text or plain-text shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentControlSpec {
    pub placement: ContentControlPlacement,
    pub kind: SdtType,
    pub tag: Option<String>,
    pub alias: Option<String>,
    pub lock: Option<ContentControlLock>,
    pub appearance: Option<ContentControlAppearance>,
    pub placeholder: Option<bool>,
}

impl Document {
    /// Insert one rich-text or plain-text shell at `destination`.
    pub fn insert_content_control(
        &mut self,
        destination: &ContentLocation,
        spec: ContentControlSpec,
    ) -> Result<()> {
        reject_control_text("content-control tag", spec.tag.as_deref())?;
        reject_control_text("content-control alias", spec.alias.as_deref())?;
        let kind_name = plain_or_rich(spec.kind)?;
        let mut candidate = self.clone_for_staging();
        // Reserved on the candidate, whose registry a success commits and a
        // refusal drops with it.
        let id = candidate.identifiers.reserve_content_control_id()?;
        let shell = control_shell(&spec, id, kind_name)?;
        candidate.flush_to_package()?;
        match spec.placement {
            ContentControlPlacement::Block | ContentControlPlacement::Paragraph => {
                let control = parse_control(&shell)?;
                candidate
                    .insert_content(destination, ContentFragment::content_control(control)?)
                    .map_err(control_fault)?;
                self.commit_staged_mutation(candidate);
            }
            ContentControlPlacement::Row => {
                let located = table_parent_range(&candidate, destination)?;
                let columns = table_column_count(&located.xml, &located.range)?;
                let shell =
                    control_shell_with_body(&spec, id, kind_name, &row_shell(columns, spec.kind)?)?;
                splice_inside(&mut candidate, located, &shell)?;
                self.publish_candidate(candidate)?;
            }
            ContentControlPlacement::Cell => {
                let located = row_parent_range(&candidate, destination)?;
                let shell = control_shell_with_body(&spec, id, kind_name, &cell_shell(spec.kind)?)?;
                splice_inside(&mut candidate, located, &shell)?;
                self.publish_candidate(candidate)?;
            }
            ContentControlPlacement::Inline => {
                let located = paragraph_parent_range(&candidate, destination)?;
                if shell_contains_paragraph(&shell)? {
                    return Err(Error::Other(
                        "inline content control cannot contain a paragraph".to_owned(),
                    ));
                }
                splice_inside(&mut candidate, located, &shell)?;
                self.publish_candidate(candidate)?;
            }
        }
        Ok(())
    }

    /// Add one custom XML item and its properties part, in place.
    ///
    /// The returned value is the braced item id. An empty URI list writes an
    /// empty `ds:schemaRefs`. Invalid item bytes and an empty schema URI
    /// change nothing.
    pub fn add_custom_xml_store(
        &mut self,
        item_xml: &[u8],
        schema_uris: &[&str],
    ) -> Result<String> {
        validate_store_item(item_xml)?;
        for uri in schema_uris {
            if uri.is_empty() {
                return Err(Error::Other("custom XML schema URI is empty".to_owned()));
            }
            oxoffice_core::xml::reject_non_xml_characters("custom XML schema URI", uri)?;
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        self.identifiers.open_journal();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::bibliography::add_custom_xml_store_in_place(self, item_xml, schema_uris)
        }));
        let journal = self.identifiers.close_journal();
        let result = match result {
            Err(panic) => {
                // The writes come after every step that can fail.
                if !journal.changed_package_bytes() {
                    self.undo_reservations(journal);
                }
                std::panic::resume_unwind(panic)
            }
            Ok(Err(error)) => {
                self.undo_reservations(journal);
                Err(error)
            }
            Ok(Ok(item_id)) => {
                let signed = self.package.package_rels.items.iter().any(|relationship| {
                    relationship.rel_type
                        == oxoffice_opc::relationship::rel_types::DIGITAL_SIGNATURE_ORIGIN
                });
                self.package_signatures_invalidated |= signed && journal.changed_package();
                // A bound control shows the store's value; the store leaves
                // the main part alone.
                self.invalidate_layout_keeping_body_answer();
                Ok(item_id)
            }
        };
        #[cfg(debug_assertions)]
        self.assert_custom_xml_store_matches_staged(before, item_xml, schema_uris, &result);
        result
    }

    /// Debug proof that the in-place store addition wrote the parts,
    /// relationships and content types the staged route writes, with the same
    /// identity, or refused with its error and left the package as it was.
    #[cfg(debug_assertions)]
    fn assert_custom_xml_store_matches_staged(
        &self,
        before: Self,
        item_xml: &[u8],
        schema_uris: &[&str],
        result: &Result<String>,
    ) {
        let mut staged = before.clone_for_staging();
        let outcome = staged.prepare_staged_package().and_then(|_| {
            crate::bibliography::stage_custom_xml_store(&mut staged, item_xml, schema_uris)
        });
        match (result, outcome) {
            (Ok(direct), Ok(staged_store)) => {
                assert_eq!(*direct, staged_store.item_id, "another store identity");
                let part = staged_store.part_name;
                let rels = |document: &Self, owner: &str| {
                    document.package.get_part_rels(owner).map(|relationships| {
                        let mut items = relationships
                            .items
                            .iter()
                            .map(|item| {
                                (item.id.clone(), item.rel_type.clone(), item.target.clone())
                            })
                            .collect::<Vec<_>>();
                        items.sort();
                        items
                    })
                };
                let properties = rels(&staged, &part)
                    .and_then(|items| items.first().cloned())
                    .map(|(_, _, target)| {
                        oxoffice_opc::OpcPackage::resolve_rel_target(&part, &target)
                    })
                    .expect("the staged item names its properties");
                for name in [&part, &properties] {
                    assert_eq!(staged.package.get_part(name), self.package.get_part(name));
                    assert_eq!(
                        staged.package.content_types.overrides.get(name.as_str()),
                        self.package.content_types.overrides.get(name.as_str()),
                    );
                }
                assert_eq!(rels(&staged, &part), rels(self, &part));
                assert_eq!(
                    rels(&staged, &staged.doc_part_name),
                    rels(self, &self.doc_part_name)
                );
            }
            (Err(direct), Err(staged)) => {
                assert_eq!(direct.to_string(), staged.to_string());
                assert!(before.package.same_package_as(&self.package));
                assert_eq!(before.identifiers, self.identifiers);
            }
            // The staged route fails at the save of a document that could not
            // be saved before (a typed edit of a body that cannot be saved
            // after one); the in-place addition leaves that error to the save.
            (Ok(_), Err(staged))
                if before
                    .clone_for_staging()
                    .to_bytes()
                    .is_err_and(|error| error.to_string() == staged.to_string())
                    && self
                        .clone_for_staging()
                        .to_bytes()
                        .is_err_and(|error| error.to_string() == staged.to_string()) => {}
            (Ok(_), Err(staged)) => panic!("the in-place store addition hid {staged}"),
            (Err(direct), Ok(_)) => panic!("the in-place store addition refused with {direct}"),
        }
    }

    /// Attach `w:dataBinding` to one control after the store and path resolve.
    ///
    /// `None` and an empty mapping string omit `w:prefixMappings`. A failed
    /// check publishes nothing.
    pub fn set_content_control_binding(
        &mut self,
        location: &ContentLocation,
        store_item_id: &str,
        xpath: &str,
        prefix_mappings: Option<&str>,
    ) -> Result<()> {
        let mappings_text = prefix_mappings.filter(|value| !value.is_empty());
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let mappings = parse_prefix_mappings(mappings_text.unwrap_or(""))?;
        let steps = parse_xpath(xpath, &mappings)?;
        let part_name =
            resolve_custom_xml_part(&candidate.package, &candidate.doc_part_name, store_item_id)?;
        {
            let item = candidate
                .package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("custom XML part {part_name} is missing")))?;
            select_xml_element(item, &steps)?;
        }
        let (part, mut xml, range, _) = control_item(&candidate, location)?;
        let updated = edit_sdt_property(
            &xml,
            &range,
            PropertyEdit::DataBinding {
                prefix_mappings: mappings_text.map(str::to_owned),
                xpath: xpath.to_owned(),
                store_item_id: store_item_id.to_owned(),
            },
        )?;
        xml.splice(range, updated);
        candidate.write_story_part(&part, xml)?;
        self.publish_candidate(candidate)
    }

    /// Set or clear `w:tag` without rewriting `sdtContent`.
    pub fn set_content_control_tag(
        &mut self,
        location: &ContentLocation,
        tag: Option<&str>,
    ) -> Result<()> {
        reject_control_text("content-control tag", tag)?;
        self.edit_control_property(location, PropertyEdit::Tag(tag.map(str::to_owned)))
    }

    /// Set or clear `w:alias` without rewriting `sdtContent`.
    pub fn set_content_control_alias(
        &mut self,
        location: &ContentLocation,
        alias: Option<&str>,
    ) -> Result<()> {
        reject_control_text("content-control alias", alias)?;
        self.edit_control_property(location, PropertyEdit::Alias(alias.map(str::to_owned)))
    }

    /// Set or clear `w:lock` without rewriting `sdtContent`.
    pub fn set_content_control_lock(
        &mut self,
        location: &ContentLocation,
        lock: Option<ContentControlLock>,
    ) -> Result<()> {
        self.edit_control_property(location, PropertyEdit::Lock(lock))
    }

    /// Set or clear `w15:appearance` without rewriting `sdtContent`.
    pub fn set_content_control_appearance(
        &mut self,
        location: &ContentLocation,
        appearance: Option<ContentControlAppearance>,
    ) -> Result<()> {
        self.edit_control_property(location, PropertyEdit::Appearance(appearance))
    }

    /// Set or clear the qualified `w:showingPlcHdr` marker.
    ///
    /// A foreign-namespace lookalike stays in place.
    pub fn set_content_control_placeholder(
        &mut self,
        location: &ContentLocation,
        placeholder: Option<bool>,
    ) -> Result<()> {
        self.edit_control_property(location, PropertyEdit::Placeholder(placeholder))
    }

    /// Replace the type element on an existing shell.
    ///
    /// Tag, alias, id, lock, appearance, placeholder, and unmodelled property
    /// children stay. A refusal publishes nothing.
    pub fn set_content_control_type(
        &mut self,
        location: &ContentLocation,
        payload: ContentControlTypePayload,
    ) -> Result<()> {
        validate_type_payload(&payload)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (part, mut xml, mut range, _) = control_item(&candidate, location)?;
        validate_repeating_type_change(&xml, &range, &payload)?;
        match &payload {
            ContentControlTypePayload::Picture { relationship_id } => {
                if !part_owns_image(&candidate, &part, relationship_id) {
                    return Err(Error::Other(
                        "content control picture relationship is missing".to_owned(),
                    ));
                }
                if !content_embeds_relationship(&xml, &range, relationship_id)? {
                    return Err(Error::Other(
                        "content control picture content does not embed that relationship"
                            .to_owned(),
                    ));
                }
            }
            ContentControlTypePayload::RepeatingSection => match repeating_content(&xml, &range)? {
                RepeatingContent::Empty => {
                    return Err(Error::Other("repeating section has no item".to_owned()));
                }
                RepeatingContent::NotBlock => {
                    return Err(Error::Other(
                        "repeating section content is not a block sequence".to_owned(),
                    ));
                }
                RepeatingContent::AlreadyItem => {}
                RepeatingContent::Wrap => {
                    let id = candidate.identifiers.reserve_content_control_id()?;
                    (xml, range) = wrap_repeating_item(&xml, &range, id)?;
                }
            },
            _ => {}
        }
        let markup = type_markup(&payload)?;
        let updated = replace_control_type_markup(&xml, &range, markup.as_bytes())?;
        xml.splice(range, updated);
        candidate.write_story_part(&part, xml)?;
        self.publish_candidate(candidate)
    }

    /// Clone the first repeating item inside a repeating section and append it.
    pub fn repeat_content_control(&mut self, location: &ContentLocation) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (part, mut xml, range, _) = control_item(&candidate, location)?;
        let elems = index_elements(&xml)?;
        let section = element_at(&elems, &range)?;
        if !sdt_is_repeating_section(&elems, section) {
            return Err(Error::Other(
                "repeat_content_control addresses a repeating section".to_owned(),
            ));
        }
        let Some(item_range) = first_repeating_item_range(&elems, section) else {
            return Err(Error::Other(
                "repeating section has no item to clone".to_owned(),
            ));
        };
        let bytes = xml
            .get(item_range)
            .ok_or_else(|| Error::Other("content control bytes are missing".to_owned()))?
            .to_vec();
        let fresh = freshen_control(&mut candidate, &bytes)?;
        let elems = index_elements(&xml)?;
        let section = element_at(&elems, &range)?;
        let content = sdt_content_elem(&elems, section)
            .ok_or_else(|| Error::Other("repeating section has no item to clone".to_owned()))?;
        let content_range = content.range.clone();
        let replacement = insert_into_element(&xml, &content_range, &fresh)?;
        xml.splice(content_range, replacement);
        candidate.write_story_part(&part, xml)?;
        self.publish_candidate(candidate)
    }

    /// Delete one control, including the content inside it.
    /// The footnotes and endnotes only that content referenced go too, and
    /// so do the comment threads wholly inside it; a comment the control
    /// holds only part of makes the call fail, as in
    /// [`Document::try_remove_content`].
    pub fn remove_content_control(&mut self, location: &ContentLocation) -> Result<()> {
        let (_part, xml, range, direct) = control_item(self, location)?;
        if last_repeating_item(&xml, &range)? {
            return Err(Error::Other(
                "cannot remove the last repeating section item".to_owned(),
            ));
        }
        if direct && control_parent_is_block(&xml, &range)? {
            refuse_cell_without_paragraph(self, location.story(), &xml, &range, b"")?;
            // The removed content is dropped, not handed back: the notes only
            // it referenced go with it.
            let mut candidate = self.clone_for_staging();
            let before = candidate.main_part_snapshot()?;
            candidate
                .remove_content_at(location)
                .map_err(control_fault)?;
            let after = candidate.main_part_snapshot()?;
            candidate.remove_unreferenced_notes(&before, &after)?;
            candidate.reconcile_comment_removal(self)?;
            return self.publish_candidate(candidate);
        }
        self.rewrite_control(location, ControlRewrite::Remove)
    }

    /// Drop the shell and leave its content when the parent can hold it.
    pub fn unwrap_content_control(&mut self, location: &ContentLocation) -> Result<()> {
        self.rewrite_control(location, ControlRewrite::Unwrap)
    }

    /// Move one control inside the same story. The id stays.
    pub fn move_content_control(
        &mut self,
        source: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        if source.story() != destination.story() {
            return Err(Error::Other(
                "content control can move only within one story".to_owned(),
            ));
        }
        let (_part, xml, range, direct) = control_item(self, source)?;
        if last_repeating_item(&xml, &range)? {
            return Err(Error::Other(
                "cannot move the last repeating section item".to_owned(),
            ));
        }
        // A block control, direct or written in another block control's
        // content, moves as a block.
        if control_parent_is_block(&xml, &range)? && (direct || self.names_a_block(source)) {
            if !control_matches_context(
                ParentSlot::Block,
                story_inside_repeating_section(self, destination.story())?,
                &xml,
                &range,
            )? {
                return Err(Error::Other(
                    "content control placement does not match its parent".to_owned(),
                ));
            }
            return self
                .move_content(source, destination)
                .map_err(control_fault);
        }
        let bytes = xml
            .get(range.clone())
            .ok_or_else(|| Error::Other("content control bytes are missing".to_owned()))?
            .to_vec();
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (part, mut updated, source_range, _) = control_item(&candidate, source)?;
        let dest = nested_destination(&candidate, destination)?;
        if part != dest.part {
            return Err(Error::Other(
                "content control can move only within one story".to_owned(),
            ));
        }
        if ranges_overlap(&source_range, &dest.range) {
            return Err(Error::Other(
                "content control move destination is inside the source".to_owned(),
            ));
        }
        destination_accepts_control(&dest.xml, &dest.range, &updated, &source_range)?;
        let original_len = updated.len();
        updated.drain(source_range.clone());
        let dest_range = shift_after_removal(&dest.range, &source_range);
        let replacement = insert_into_element(&updated, &dest_range, &bytes)?;
        updated.splice(dest_range, replacement);
        ensure_cell_paragraph(&candidate, source.story(), &updated, original_len)?;
        candidate.write_story_part(&part, updated)?;
        self.publish_candidate(candidate)
    }

    /// Clone one control. The copy gets a new id and drops paragraph identity.
    pub fn clone_content_control(
        &mut self,
        source: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let (_part, xml, range, direct) = control_item(self, source)?;
        if control_parent_is_block(&xml, &range)? && (direct || self.names_a_block(source)) {
            if !control_matches_context(
                ParentSlot::Block,
                story_inside_repeating_section(self, destination.story())?,
                &xml,
                &range,
            )? {
                return Err(Error::Other(
                    "content control placement does not match its parent".to_owned(),
                ));
            }
            return self
                .clone_content(source, destination)
                .map_err(control_fault);
        }
        let bytes = xml
            .get(range)
            .ok_or_else(|| Error::Other("content control bytes are missing".to_owned()))?
            .to_vec();
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (_, source_xml, source_range, _) = control_item(&candidate, source)?;
        let dest = nested_destination(&candidate, destination)?;
        destination_accepts_control(&dest.xml, &dest.range, &source_xml, &source_range)?;
        let fresh = freshen_control(&mut candidate, &bytes)?;
        let mut dest_xml = dest.xml;
        let replacement = insert_into_element(&dest_xml, &dest.range, &fresh)?;
        dest_xml.splice(dest.range, replacement);
        candidate.write_story_part(&dest.part, dest_xml)?;
        self.publish_candidate(candidate)
    }
}

struct PartRange {
    part: String,
    xml: Vec<u8>,
    range: Range<usize>,
}

struct XmlElem {
    id: usize,
    parent: Option<usize>,
    local: Vec<u8>,
    word: bool,
    w14: bool,
    w15: bool,
    range: Range<usize>,
    open_end: usize,
    inner_end: Option<usize>,
    val: Option<String>,
}

/// One form field selected by a content-control tag or a legacy field identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldTarget {
    /// Case-sensitive content-control tag.
    ContentControlTag(String),
    /// Legacy form field addressed by its pre-batch source part and ordinal.
    Legacy { source_part: String, ordinal: usize },
}

/// A typed value written into a modern or legacy form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldValue {
    /// Rich, plain, date, combo, or modern drop-down text.
    Text(String),
    /// Checkbox state. `false` is a present value.
    Checked(bool),
    /// Legacy drop-down index. Modern lists do not accept this.
    SelectedIndex(usize),
}

/// The value a reset rule writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormResetValue {
    /// Typed value, clearing placeholder state.
    Value(FormFieldValue),
    /// Prompt display for a modern text, date, or list control.
    Placeholder { text: String },
}

/// One transient validation and reset rule. It is not stored on the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormFieldRule {
    pub target: FormFieldTarget,
    pub required: bool,
    pub reset: FormResetValue,
}

/// One value to publish for a target that a rule already names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormFieldAssignment {
    pub target: FormFieldTarget,
    pub value: FormFieldValue,
}

/// One current-state problem for one physical control or legacy field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormValidationIssue {
    pub target: FormFieldTarget,
    pub source_part: String,
    pub ordinal: usize,
    pub message: String,
}

impl Document {
    /// Report one issue per selected physical field. An empty rule list discovers nothing.
    pub fn validate_form(&self, rules: &[FormFieldRule]) -> Result<Vec<FormValidationIssue>> {
        let staged = flushed_form_document(self)?;
        let mut state = read_form_state(&staged)?;
        let selections = select_rules(&staged, &mut state, rules)?;
        Ok(current_issues(&state, rules, &selections))
    }

    /// Publish assignment values atomically. Other required fields may stay empty.
    /// A control's value replaces its content and the tracked changes in it,
    /// as [`Self::set_content_control_value_by_tag`] writes one.
    pub fn fill_form(
        &mut self,
        rules: &[FormFieldRule],
        assignments: &[FormFieldAssignment],
    ) -> Result<Vec<usize>> {
        match self.fill_form_in_blocks(rules, assignments)? {
            Some(edit) => Ok(edit.counts),
            None => self.fill_form_staged(rules, assignments),
        }
    }

    /// Restore every rule's reset value atomically. Required fields may become empty.
    /// A control's reset value replaces its content and the tracked changes
    /// in it, as a value does.
    pub fn reset_form(&mut self, rules: &[FormFieldRule]) -> Result<Vec<usize>> {
        match self.reset_form_in_blocks(rules)? {
            Some(edit) => Ok(edit.counts),
            None => self.reset_form_staged(rules),
        }
    }

    /// [`Self::fill_form`] on the body blocks that hold its targets, one
    /// `EditUnit::BodyBlocks` unit per changed block, and the header and
    /// footer parts that hold them and the custom XML items of its bound
    /// controls (`EditUnit::Parts`). `Ok(None)` changes
    /// nothing and leaves the call to the staged route: a legacy field
    /// target, a tag a note or a comment names, a body that cannot be saved after
    /// a typed edit, a block that does not read back as it writes. A refusal
    /// leaves the document as it was, with the staged route's error.
    #[doc(hidden)]
    pub fn fill_form_in_blocks(
        &mut self,
        rules: &[FormFieldRule],
        assignments: &[FormFieldAssignment],
    ) -> Result<Option<FormEdit>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.form_in_blocks(rules, form_in_place::FormPlanner::Fill(assignments));
        #[cfg(debug_assertions)]
        self.assert_form_in_blocks_matches_staged(before, &result, |staged| {
            staged.fill_form_staged(rules, assignments)
        });
        result
    }

    /// [`Self::reset_form`] on the body blocks that hold its targets, as
    /// [`Self::fill_form_in_blocks`].
    #[doc(hidden)]
    pub fn reset_form_in_blocks(&mut self, rules: &[FormFieldRule]) -> Result<Option<FormEdit>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.form_in_blocks(rules, form_in_place::FormPlanner::Reset);
        #[cfg(debug_assertions)]
        self.assert_form_in_blocks_matches_staged(before, &result, |staged| {
            staged.reset_form_staged(rules)
        });
        result
    }

    #[cfg(debug_assertions)]
    fn assert_form_in_blocks_matches_staged(
        &self,
        before: Document,
        result: &Result<Option<FormEdit>>,
        staged: impl FnOnce(&mut Document) -> Result<Vec<usize>>,
    ) {
        match result {
            Ok(None) => assert_eq!(
                self.clone_for_staging().to_bytes().ok(),
                before.clone_for_staging().to_bytes().ok(),
                "a form operation left to the staged route changed the document"
            ),
            Ok(Some(edit)) => {
                let counts = Ok(edit.counts.clone());
                self.assert_in_place_matches_staged_main_part_rewritten(before, &counts, staged);
            }
            Err(error) => {
                let error: Result<Vec<usize>> = Err(Error::Other(error.to_string()));
                self.assert_in_place_matches_staged_main_part_rewritten(before, &error, staged);
            }
        }
    }

    fn fill_form_staged(
        &mut self,
        rules: &[FormFieldRule],
        assignments: &[FormFieldAssignment],
    ) -> Result<Vec<usize>> {
        let mut candidate = flushed_form_document(self)?;
        let mut state = read_form_state(&candidate)?;
        let selections = select_rules(&candidate, &mut state, rules)?;
        let plan = plan_fill(&candidate, &state, assignments, &selections)?;
        apply_form_plan(&mut candidate, &state, &plan)?;
        publish_form_document(self, candidate)?;
        Ok(plan.counts)
    }

    fn reset_form_staged(&mut self, rules: &[FormFieldRule]) -> Result<Vec<usize>> {
        let mut candidate = flushed_form_document(self)?;
        let mut state = read_form_state(&candidate)?;
        let selections = select_rules(&candidate, &mut state, rules)?;
        let plan = plan_reset(&candidate, &state, rules, &selections)?;
        apply_form_plan(&mut candidate, &state, &plan)?;
        publish_form_document(self, candidate)?;
        Ok(plan.counts)
    }

    /// Publish forms-mode protection only when `validate_form` reports no issues.
    pub fn protect_form(
        &mut self,
        rules: &[FormFieldRule],
        protection: crate::DocumentProtection,
    ) -> Result<()> {
        if protection.mode != crate::ProtectionMode::Forms {
            return Err(form_fault("form protection requires forms mode"));
        }
        let issues = self.validate_form(rules)?;
        if !issues.is_empty() {
            return Err(form_fault("form protection requires a valid form"));
        }
        self.set_document_protection(protection)
    }
}

#[derive(Clone)]
struct BindingRef {
    store_item_id: String,
    xpath: String,
    prefix_mappings: Option<String>,
}

#[derive(Clone)]
struct ListChoice {
    value: Option<String>,
    display: Option<String>,
}

#[derive(Clone)]
struct DateFacts {
    format: Option<String>,
    language: Option<String>,
    store: Option<String>,
    calendar: Option<String>,
    full_date: Option<String>,
    duplicate: bool,
}

struct PhysicalControl {
    part: String,
    start: usize,
    end: usize,
    ordinal: usize,
    tag: Option<String>,
    kind: ControlKind,
    placeholder: bool,
    placeholder_broken: bool,
    display: String,
    binding: Option<BindingRef>,
    bound_text: Option<String>,
    checked: Option<bool>,
    checked_broken: bool,
    glyph_broken: bool,
    checked_glyph: Option<String>,
    unchecked_glyph: Option<String>,
    checked_font: Option<String>,
    unchecked_font: Option<String>,
    checked_fault: Option<String>,
    unchecked_fault: Option<String>,
    date: Option<DateFacts>,
    list_items: Vec<ListChoice>,
    duplicate_tag: bool,
}

struct LegacyField {
    part: String,
    ordinal: usize,
    kind: crate::LegacyFormFieldKind,
    value: crate::LegacyFormFieldValue,
    default_value: Option<crate::LegacyFormFieldValue>,
    choices: Vec<String>,
}

struct LegacyAnchor {
    part: String,
    ordinal: usize,
    start: usize,
    end: usize,
}

struct FormState {
    doc_part: String,
    controls: Vec<PhysicalControl>,
    legacy: Vec<LegacyField>,
    anchors: Vec<LegacyAnchor>,
}

struct Selection {
    target: FormFieldTarget,
    owners: Vec<Owner>,
}

#[derive(Clone)]
struct ControlPatch {
    placeholder: Option<bool>,
    full_date: FullDateEdit,
    last_value: LastValueEdit,
    checked: Option<bool>,
    display: Option<String>,
    font: Option<String>,
    /// For a value: whether the control showed its placeholder, so that the
    /// value takes the control's run properties. `None` for a placeholder
    /// reset, whose text keeps the prompt's formatting.
    prompt: Option<bool>,
}

struct BindingWrite {
    binding: BindingRef,
    value: String,
}

struct FormPlan {
    counts: Vec<usize>,
    legacy: Vec<LegacyWrite>,
    patches: Vec<LogicalPatch>,
    bindings: Vec<BindingWrite>,
    watched_legacy: Vec<(String, usize)>,
}

fn form_fault(message: impl Into<String>) -> Error {
    Error::Other(message.into())
}
