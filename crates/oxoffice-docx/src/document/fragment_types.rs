//! How a fragment's conflicts are settled, scoped reads and template fragment sources, and the methods of the content and document fragments defined in document.rs.

use super::*;

pub(crate) struct ScopedDocumentRead<'operation, 'source, 'frozen> {
    pub(crate) operation:
        &'operation mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
    pub(crate) source: &'source oxoffice_docx_oxml::document::ScopedWordSource,
    pub(crate) related:
        Option<&'source BTreeMap<String, Arc<oxoffice_docx_oxml::document::ScopedWordSource>>>,
}

pub(crate) struct TemplateFragmentSources {
    pub(crate) main: Arc<oxoffice_docx_oxml::document::ScopedWordSource>,
    pub(crate) related: BTreeMap<String, Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
}

pub(crate) enum TemplateFragmentSelection {
    Source(Box<oxoffice_docx_oxml::document::ScopedWordSource>),
    Bytes(Vec<u8>),
}

/// Conflict choices for one cross-document fragment import.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentConflictPolicy {
    reuse_equivalent_styles: bool,
    reuse_equivalent_numbering: bool,
    reuse_equivalent_related_parts: bool,
    preserve_source_effective_defaults: bool,
    keep_destination_formatting: bool,
}

/// Whether [`FragmentConflictPolicy::for_insert_document`] without a policy,
/// which `Document::append`, `Document::append_with_break` and
/// `Document::insert_document` use, keeps the destination's formatting (see
/// [`FragmentConflictPolicy::with_destination_formatting`]).
const INSERTED_DOCUMENTS_KEEP_DESTINATION_FORMATTING: bool = true;

impl FragmentConflictPolicy {
    /// Reuse equivalent supported dependencies and rename non-equivalent ones.
    /// A default character, table or numbering style under an id the
    /// destination lacks (Word's localized `Policepardfaut`) takes the
    /// destination's default. A default paragraph style does when it is that
    /// default under another id; a different one is imported under its own
    /// id, no longer a default, as a different default under the
    /// destination's id is renamed. The styles based on it and the text that
    /// names it keep its look; text that names no style takes the
    /// destination's default paragraph style.
    pub const fn reuse_equivalent() -> Self {
        Self {
            reuse_equivalent_styles: true,
            reuse_equivalent_numbering: true,
            reuse_equivalent_related_parts: true,
            preserve_source_effective_defaults: false,
            keep_destination_formatting: false,
        }
    }

    /// Allocate a fresh destination identity for every supported dependency,
    /// except a default style, which takes the destination's default as
    /// [`Self::reuse_equivalent`] says (a type has one default).
    pub const fn rename_all() -> Self {
        Self {
            reuse_equivalent_styles: false,
            reuse_equivalent_numbering: false,
            reuse_equivalent_related_parts: false,
            preserve_source_effective_defaults: false,
            keep_destination_formatting: false,
        }
    }

    /// The policy of inserting a whole document. Without a policy, lists
    /// restart and the content keeps the destination's formatting, as Word's
    /// Insert > Text from File does (see [`Self::with_destination_formatting`]).
    /// A given policy keeps its choice, and preserves the source's effective
    /// fonts unless it keeps the destination's formatting.
    pub const fn for_insert_document(policy: Option<Self>) -> Self {
        let mut policy = match policy {
            Some(policy) => policy,
            None => Self::reuse_equivalent()
                .with_numbering_reuse(false)
                .with_destination_formatting(INSERTED_DOCUMENTS_KEEP_DESTINATION_FORMATTING),
        };
        policy.preserve_source_effective_defaults = true;
        policy
    }

    /// Select whether the imported content takes the destination's look, as
    /// Word's Insert > Text from File does: a style the destination defines
    /// under the same id and type keeps the destination's definition, and the source's
    /// document defaults are not carried into the content. Otherwise a style
    /// with the same id and a different definition is imported under a new id.
    pub const fn with_destination_formatting(mut self, keep: bool) -> Self {
        self.keep_destination_formatting = keep;
        self
    }

    /// Read when used, so `with_destination_formatting` may come before or
    /// after `for_insert_document`.
    pub(crate) const fn preserves_source_effective_defaults(self) -> bool {
        self.preserve_source_effective_defaults && !self.keep_destination_formatting
    }

    /// Select whether equivalent style definitions are reused.
    pub const fn with_style_reuse(mut self, reuse: bool) -> Self {
        self.reuse_equivalent_styles = reuse;
        self
    }

    /// Select whether equivalent numbering definitions are reused.
    pub const fn with_numbering_reuse(mut self, reuse: bool) -> Self {
        self.reuse_equivalent_numbering = reuse;
        self
    }

    /// Select whether equivalent relationship-reached leaf parts are reused.
    pub const fn with_related_part_reuse(mut self, reuse: bool) -> Self {
        self.reuse_equivalent_related_parts = reuse;
        self
    }

    pub(crate) const fn reuse_styles(self) -> bool {
        self.reuse_equivalent_styles
    }

    pub(crate) const fn reuse_numbering(self) -> bool {
        self.reuse_equivalent_numbering
    }

    pub(crate) const fn reuse_related_parts(self) -> bool {
        self.reuse_equivalent_related_parts
    }

    pub(crate) const fn keeps_destination_formatting(self) -> bool {
        self.keep_destination_formatting
    }
}

impl Default for FragmentConflictPolicy {
    fn default() -> Self {
        Self::reuse_equivalent()
    }
}

impl DocumentFragment {
    pub(crate) fn from_part_content(
        document: &Document,
        source_part: &str,
        selected: Vec<u8>,
    ) -> Result<Self> {
        validate_fragment_block_content(&selected)?;
        let mut candidate = document.clone_for_staging();
        candidate.prepare_staged_package()?;
        candidate.project_fragment_part_content(source_part, selected)?;
        let mut output = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut output)?;
        Ok(Self {
            package: output.into_inner(),
            include_final_section_properties: false,
        })
    }

    pub(crate) fn from_part_source(
        document: &mut Document,
        source_part: &str,
        selected: oxoffice_docx_oxml::document::ScopedWordSource,
        evaluation: &mut crate::template::TemplateEvaluation<'_, '_>,
    ) -> Result<(Self, TemplateFragmentSources)> {
        validate_fragment_block_content(selected.as_bytes())?;
        document.prepare_staged_template_package(
            &mut evaluation.operation,
            &mut evaluation.lineage,
            &mut evaluation.sources,
        )?;
        let mut context = TemplateMainWriter {
            operation: &mut evaluation.operation,
            lineage: &evaluation.lineage,
            sources: &mut evaluation.sources,
        };
        let main = match &context.sources.source {
            Some(source) => Arc::clone(source),
            None => Arc::new(context.document_source(&document.document)?),
        };
        let mut candidate = document.clone_for_staging();
        let main = candidate.project_fragment_part_source(
            source_part,
            selected,
            &main,
            context.operation,
        )?;
        let mut related = BTreeMap::new();
        for part in document.related_story_parts() {
            if part == document.doc_part_name {
                continue;
            }
            let Some(bytes) = document.package.get_part_shared(&part) else {
                continue;
            };
            if let Some(source) = context.take_related_source(&part, bytes)? {
                related.insert(part.clone(), Arc::clone(&source));
                context.sources.put_related_source(&part, bytes, source);
            }
        }
        let mut output = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut output)?;
        Ok((
            Self {
                package: output.into_inner(),
                include_final_section_properties: false,
            },
            TemplateFragmentSources {
                main: Arc::new(main),
                related,
            },
        ))
    }

    pub(crate) fn import_content_staged(
        &self,
        candidate: &mut Document,
        destination_part: &str,
        policy: FragmentConflictPolicy,
        source_owned_numbering: bool,
        note_copy_sources: &mut Vec<(StoryKind, String, i32, String, i32)>,
    ) -> Result<Vec<u8>> {
        let (xml, _) = crate::field::import_document_fragment_content(
            candidate,
            &self.package,
            self.include_final_section_properties,
            policy,
            destination_part,
            source_owned_numbering,
            note_copy_sources,
            None,
        )?;
        package_authoritative_body_fragment(
            &xml,
            self.include_final_section_properties,
            &BTreeMap::new(),
        )
    }

    pub(crate) fn contains_section_properties(&self) -> bool {
        self.include_final_section_properties
    }

    /// Capture one non-empty, half-open range of blocks of one container:
    /// the story, or the content of one block content control, whose end is
    /// [`ContentLocation::control_end`]. A range holding only part of a
    /// complex field is refused.
    ///
    /// `start` is included and `end` is excluded, each with the bookmark
    /// starts written just before a block belonging to that block: the range
    /// ends before those of `end`, and starts before those of `start` when
    /// every one of them ends inside the range or in the bookmark ends
    /// written just after it, which it then takes, with those closing a
    /// bookmark started inside it (when a leading start ends further on, the
    /// range keeps its start and takes none of those ends, so an import of
    /// it leaves those bookmarks out). Section properties can be
    /// included only when `end` is the main body's final boundary.
    pub fn from_range(
        document: &Document,
        start: &ContentLocation,
        end: &ContentLocation,
        include_final_section_properties: bool,
    ) -> Result<Self> {
        let mut candidate = document.clone_for_staging();
        candidate.prepare_staged_package()?;
        if start.story != end.story {
            return Err(Error::Other(
                "document fragment ranges must share one checked story owner".to_owned(),
            ));
        }
        let (source, owner, _) = candidate.fragment_story_source_owner_and_owners(&start.story)?;
        let source_xml = source.xml.into_owned();
        let start_slot = resolve_block_location(&source_xml, &owner, start)?;
        start_slot.block()?;
        let mut start_index = start_slot.index;
        let item_count = start_slot.items.len();
        let (_, checked_owner, _) = candidate.fragment_story_source_owner_and_owners(&end.story)?;
        // The range ends before the bookmark starts that lead its end block.
        let end_slot = resolve_block_location(&source_xml, &checked_owner, end)?;
        let (_, mut end_index) = end_slot.leading_boundary(&source_xml)?;
        if start_slot.owner.full != end_slot.owner.full {
            let container = |slot: &BlockSlot| match slot.control {
                Some(control) => format!("the content of content control {control}"),
                None => "the story".to_owned(),
            };
            let end_hint = match start_slot.control {
                Some(control) => format!(
                    "; a range in that control's content ends at one of its blocks or at \
                     control_end(story, {control})"
                ),
                None => {
                    // The block of the story that holds the end.
                    let holder = scan_story_items(&source_xml, &owner, None)?
                        .iter()
                        .position(|item| {
                            item.direct_owner_child
                                && item.full.start <= end_slot.owner.full.start
                                && end_slot.owner.full.end <= item.full.end
                        });
                    match holder {
                        Some(holder) => format!(
                            "; a range of the story ends at a block of the story: at the control \
                             that holds that end ([{holder}]) to stop before it, or at the block \
                             after it"
                        ),
                        None => String::new(),
                    }
                }
            };
            return Err(Error::Other(format!(
                "document fragment range splits block ownership: it starts in {} and ends in {}, \
                 and a range stays in one container{end_hint}",
                container(&start_slot),
                container(&end_slot),
            )));
        }
        if start_index >= end_index {
            return Err(Error::Other(
                "document fragment range must contain at least one direct body item".to_owned(),
            ));
        }
        let nested = start_slot.control.is_some();
        let owner = start_slot.owner;
        if include_final_section_properties
            && (start.story.kind != StoryKind::Body || nested || end_index != item_count)
        {
            return Err(Error::Other(
                "section-inclusive document fragments must end at the main-body boundary"
                    .to_owned(),
            ));
        }
        let items = start_slot.items;
        // The range starts before the bookmark starts that lead its start
        // block when every one of them ends inside the range or just after
        // it, and then takes the bookmark ends written just after it that
        // close them or a bookmark started inside it.
        let leading = leading_bookmark_start_item(&source_xml, &items, start_index)?;
        let owned =
            owned_bookmark_marker_items(&source_xml, &items, start_index, start_index..end_index)?;
        let all_leading_owned = owned.before.len() == start_index - leading;
        if all_leading_owned {
            start_index = leading;
            if let Some(last) = owned.after.last() {
                end_index = end_index.max(last + 1);
            }
        }
        let content_start = items
            .first()
            .map(|item| item.full.start)
            .ok_or_else(|| Error::Other("document fragment source body is empty".to_owned()))?;
        for item in &items[start_index..end_index] {
            let scope = story_namespace_scope_at(&source_xml, item.full.start)?;
            let xml = close_content_fragment_namespaces(&source_xml[item.full.clone()], &scope)?;
            validate_fragment_block_item(&xml)?;
        }
        let selected = items[start_index..end_index]
            .iter()
            .map(|item| item.full.clone())
            .collect::<Vec<_>>();
        refuse_partial_complex_field(&source_xml, &selected, "the range")?;
        let content_end = story_owner_content_end(&source_xml, &owner)?;
        let selected_start = items[start_index].full.start;
        let selected_end = if end_index == item_count {
            content_end
        } else {
            items[end_index].full.start
        };
        if start.story.kind == StoryKind::Body
            && start.story.part_name == candidate.doc_part_name
            && !nested
        {
            let selected = source_xml[selected_start..selected_end].to_vec();
            let mut updated = source_xml;
            updated.splice(content_start..content_end, selected);
            candidate
                .package
                .set_part(&candidate.doc_part_name, updated);
        } else {
            let mut selected = Vec::new();
            for item in &items[start_index..end_index] {
                let scope = story_namespace_scope_at(&source_xml, item.full.start)?;
                selected.extend(close_content_fragment_namespaces(
                    &source_xml[item.full.clone()],
                    &scope,
                )?);
            }
            candidate.project_fragment_part_content(&source.part_name, selected)?;
        }
        let mut output = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut output)?;
        Ok(Self {
            package: output.into_inner(),
            include_final_section_properties,
        })
    }
}

impl ContentFragment {
    /// Create a fixed-prefix paragraph fragment.
    pub fn paragraph(paragraph: CT_P) -> Result<Self> {
        Self::from_body_content(BodyContent::Paragraph(Box::new(paragraph)))
    }

    /// Create a fixed-prefix table fragment.
    pub fn table(table: CT_Tbl) -> Result<Self> {
        Self::from_body_content(BodyContent::Table(Box::new(table)))
    }

    /// Create a fixed-prefix block content-control fragment.
    pub fn content_control(control: CT_Sdt) -> Result<Self> {
        Self::from_body_content(BodyContent::ContentControl(Box::new(control)))
    }

    /// Return the direct story-item kind carried by this fragment.
    pub fn kind(&self) -> StoryItemKind {
        self.kind
    }

    fn from_body_content(content: BodyContent) -> Result<Self> {
        let kind = match &content {
            BodyContent::Paragraph(_) => StoryItemKind::Paragraph,
            BodyContent::Table(_) => StoryItemKind::Table,
            BodyContent::ContentControl(_) => StoryItemKind::ContentControl,
            BodyContent::RawXml(_) => StoryItemKind::PreservedNode,
        };
        let xml = serialize_content_fragment(content)?;
        if kind == StoryItemKind::ContentControl {
            validate_serialized_block_content_control(&xml)?;
        }
        Ok(Self {
            kind,
            xml,
            source_part_name: None,
            source_story_kind: None,
            source_owner_index: None,
            namespace_scope: BTreeMap::new(),
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        })
    }
}
