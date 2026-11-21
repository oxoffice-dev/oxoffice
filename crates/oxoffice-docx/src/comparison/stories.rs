//! The stories of a package and the comparison of one story part.

use super::*;

pub(super) fn story_ignored(options: &ComparisonOptions, kind: ComparisonStoryKind) -> bool {
    options.ignored_stories.contains(&kind)
        || (options.ignore_comments && kind == ComparisonStoryKind::Comment)
}

pub(super) fn validate_comparison_options(options: &ComparisonOptions) -> Result<()> {
    let mut unique = HashSet::new();
    if options
        .ignored_stories
        .iter()
        .any(|kind| !unique.insert(*kind))
    {
        return Err(Error::Other(
            "comparison options contain a duplicate ignored story".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn story_parts(document: &Document) -> Result<Vec<StoryPart>> {
    story_parts_with_options(document, &ComparisonOptions::default())
}

pub(super) fn story_parts_with_options(
    document: &Document,
    options: &ComparisonOptions,
) -> Result<Vec<StoryPart>> {
    let relationships = document.package.get_part_rels(&document.doc_part_name);
    let mut stories = Vec::new();
    let mut seen = HashSet::new();
    let sections = document
        .document
        .body
        .content
        .iter()
        .filter_map(|content| match content {
            BodyContent::Paragraph(paragraph) => paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref()),
            BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => None,
        })
        .chain(document.document.body.sect_pr.iter());
    for section in sections {
        for (kind, references) in [
            (ComparisonStoryKind::Header, section.header_refs.as_slice()),
            (ComparisonStoryKind::Footer, section.footer_refs.as_slice()),
        ] {
            if story_ignored(options, kind) {
                continue;
            }
            for reference in references {
                let relationships = relationships.ok_or_else(|| {
                    Error::Other(format!(
                        "{} reference {} has no relationship set",
                        kind.label(),
                        reference.rel_id
                    ))
                })?;
                let matching = relationships
                    .items
                    .iter()
                    .filter(|relationship| relationship.id == reference.rel_id)
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    return Err(Error::Other(format!(
                        "{} reference {} has {} relationships",
                        kind.label(),
                        reference.rel_id,
                        matching.len()
                    )));
                }
                let relationship = matching[0];
                let expected_type = if kind == ComparisonStoryKind::Header {
                    rel_types::HEADER
                } else {
                    rel_types::FOOTER
                };
                if relationship.rel_type != expected_type
                    || !crate::document::relationship_is_internal(relationship)
                {
                    continue;
                }
                let part_name =
                    OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
                if document.package.get_part(&part_name).is_none() {
                    return Err(Error::Other(format!(
                        "{} relationship {} targets missing part {part_name}",
                        kind.label(),
                        reference.rel_id
                    )));
                }
                if seen.insert((kind, part_name.clone())) {
                    stories.push(StoryPart { kind, part_name });
                }
            }
        }
    }

    if let Some(relationships) = relationships {
        for (kind, relationship_type) in [
            (ComparisonStoryKind::Comment, rel_types::COMMENTS),
            (ComparisonStoryKind::Footnote, rel_types::FOOTNOTES),
            (ComparisonStoryKind::Endnote, rel_types::ENDNOTES),
        ] {
            if story_ignored(options, kind) {
                continue;
            }
            for relationship in &relationships.items {
                if relationship.rel_type != relationship_type {
                    continue;
                }
                if !crate::document::relationship_is_internal(relationship) {
                    continue;
                }
                let part_name =
                    OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
                if document.package.get_part(&part_name).is_none() {
                    return Err(Error::Other(format!(
                        "{} relationship {} targets missing part {part_name}",
                        kind.label(),
                        relationship.id
                    )));
                }
                if !seen.insert((kind, part_name.clone())) {
                    return Err(Error::Other(format!(
                        "{} story part {part_name} is referenced more than once",
                        kind.label()
                    )));
                }
                stories.push(StoryPart { kind, part_name });
            }
        }
    }
    Ok(stories)
}

pub(crate) fn revision_story_parts(document: &Document) -> Result<Vec<(StoryKind, String)>> {
    story_parts(document).map(|stories| {
        stories
            .into_iter()
            .map(|story| {
                let kind = match story.kind {
                    ComparisonStoryKind::Main => StoryKind::Body,
                    ComparisonStoryKind::Header => StoryKind::Header,
                    ComparisonStoryKind::Footer => StoryKind::Footer,
                    ComparisonStoryKind::Comment => StoryKind::Comment,
                    ComparisonStoryKind::TextBox => StoryKind::TextBox,
                    ComparisonStoryKind::Footnote => StoryKind::Footnote,
                    ComparisonStoryKind::Endnote => StoryKind::Endnote,
                };
                (kind, story.part_name)
            })
            .collect()
    })
}

pub(super) fn story_xml<'a>(document: &'a Document, story: &StoryPart) -> Result<&'a [u8]> {
    document.package.get_part(&story.part_name).ok_or_else(|| {
        Error::Other(format!(
            "missing {} story {}",
            story.kind.label(),
            story.part_name
        ))
    })
}

pub(super) fn contains_modeled_revisions(
    document: &Document,
    stories: &[StoryPart],
    options: &ComparisonOptions,
) -> Result<bool> {
    if !story_ignored(options, ComparisonStoryKind::Main)
        && modeled_revision_count_with_options(
            document
                .package
                .get_part(&document.doc_part_name)
                .ok_or_else(|| {
                    Error::Other(format!("missing main story {}", document.doc_part_name))
                })?,
            options,
        )? > 0
    {
        return Ok(true);
    }
    stories
        .iter()
        .map(|story| modeled_revision_count_with_options(story_xml(document, story)?, options))
        .try_fold(false, |found, count| count.map(|count| found || count > 0))
}

pub(super) fn reject_cross_story_moves(
    original: &Document,
    edited: &Document,
    stories: &[StoryPart],
    options: &ComparisonOptions,
) -> Result<()> {
    let mut original_by_story = Vec::new();
    let mut edited_by_story = Vec::new();
    if !story_ignored(options, ComparisonStoryKind::Main) {
        original_by_story.push((
            "document".to_owned(),
            normalized_body_with_options(&original.document, options),
        ));
        edited_by_story.push((
            "document".to_owned(),
            normalized_body_with_options(&edited.document, options),
        ));
    }
    for story in stories {
        let identity = format!("{}:{}", story.kind.label(), story.part_name);
        let original_xml = story_xml(original, story)?;
        let edited_xml = story_xml(edited, story)?;
        let marker = if story_ignored(options, ComparisonStoryKind::TextBox) {
            let original_source = std::str::from_utf8(original_xml).map_err(utf8_error)?;
            let edited_source = std::str::from_utf8(edited_xml).map_err(utf8_error)?;
            let prefix = root_prefix(original_source, story.kind.root_local())?;
            Some(text_box_marker_local(
                original_source,
                edited_source,
                &prefix,
            )?)
        } else {
            None
        };
        original_by_story.push((
            identity.clone(),
            normalized_story_part(
                original,
                &story.part_name,
                original_xml,
                story.kind,
                options,
                marker.as_deref(),
                normalized_body_with_options,
            )?,
        ));
        edited_by_story.push((
            identity,
            normalized_story_part(
                edited,
                &story.part_name,
                edited_xml,
                story.kind,
                options,
                marker.as_deref(),
                normalized_body_with_options,
            )?,
        ));
    }

    let mut removed = HashMap::<String, Vec<String>>::new();
    let mut inserted = HashMap::<String, Vec<String>>::new();
    for ((identity, before), (_, after)) in original_by_story.iter().zip(&edited_by_story) {
        let before = signature_counts(before);
        let after = signature_counts(after);
        for (signature, count) in &before {
            let delta = count.saturating_sub(*after.get(signature).unwrap_or(&0));
            removed
                .entry(signature.clone())
                .or_default()
                .extend(std::iter::repeat_n(identity.clone(), delta));
        }
        for (signature, count) in &after {
            let delta = count.saturating_sub(*before.get(signature).unwrap_or(&0));
            inserted
                .entry(signature.clone())
                .or_default()
                .extend(std::iter::repeat_n(identity.clone(), delta));
        }
    }
    for (signature, sources) in removed {
        let Some(destinations) = inserted.get(&signature) else {
            continue;
        };
        if sources
            .iter()
            .any(|source| destinations.iter().any(|destination| source != destination))
        {
            return Err(Error::Other(
                "comparison cannot represent a move between Word stories".to_owned(),
            ));
        }
    }
    Ok(())
}

fn signature_counts(signatures: &[String]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for signature in signatures {
        *counts.entry(signature.clone()).or_default() += 1;
    }
    counts
}

pub(super) fn compare_story_part(
    original: &[u8],
    edited: &[u8],
    story: &StoryPart,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<Vec<u8>> {
    metadata.ids.local_word_binding |=
        foreign_word_binding(original)? || foreign_word_binding(edited)?;
    let original = std::str::from_utf8(original).map_err(utf8_error)?;
    let edited = std::str::from_utf8(edited).map_err(utf8_error)?;
    let original_root = root_inner_range(original, story.kind.root_local())?;
    let edited_root = root_inner_range(edited, story.kind.root_local())?;
    let word_prefix = root_prefix(original, story.kind.root_local())?;
    let original_bindings = root_namespace_bindings(original, story.kind.root_local(), &[])?;
    let edited_bindings = root_namespace_bindings(edited, story.kind.root_local(), &[])?;
    let tracked = if let Some(owner_local) = story.kind.owner_local() {
        compare_owned_story(
            original,
            edited,
            original_root,
            edited_root,
            owner_local,
            story,
            &word_prefix,
            &original_bindings,
            &edited_bindings,
            metadata,
            diagnostics,
        )?
    } else {
        let tracked_inner = compare_story_inner(
            &original[original_root.clone()],
            &edited[edited_root],
            &format!("{}:{}", story.kind.label(), story.part_name),
            &word_prefix,
            &original_bindings,
            &edited_bindings,
            metadata,
            diagnostics,
        )?;
        let mut tracked = original.to_owned();
        tracked.replace_range(original_root, &tracked_inner);
        tracked
    };
    let mut tracked = tracked.into_bytes();
    let generated = required_external_bindings(
        &tracked,
        &BTreeMap::from([("w".to_owned(), W_NS.to_owned())]),
    )?;
    tracked = crate::document::close_content_fragment_namespaces(&tracked, &generated)?;
    declare_w14_on_part_root(&mut tracked)?;
    crate::revision::modeled_revision_count(&tracked)?;
    Ok(tracked)
}

#[allow(clippy::too_many_arguments)]
fn compare_owned_story(
    original: &str,
    edited: &str,
    original_root: Range<usize>,
    edited_root: Range<usize>,
    owner_local: &str,
    story: &StoryPart,
    word_prefix: &str,
    original_bindings: &[(String, String)],
    edited_bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let original_spans = direct_word_element_spans(original, owner_local)?;
    let edited_spans = direct_word_element_spans(edited, owner_local)?;
    if original_spans.len() != edited_spans.len() {
        return Err(Error::Other(format!(
            "{} story shell count changed in {}",
            story.kind.label(),
            story.part_name
        )));
    }
    let (original_skeleton, original_owners) = canonical_owned_story(original, owner_local)?;
    let (edited_skeleton, edited_owners) = canonical_owned_story(edited, owner_local)?;
    if original_owners.len() != original_spans.len() || edited_owners.len() != edited_spans.len() {
        return Err(Error::Other(format!(
            "comparison could not correlate {} story owners in {}",
            story.kind.label(),
            story.part_name
        )));
    }
    if original_skeleton != edited_skeleton {
        return Err(Error::Other(format!(
            "{} story root shell changed in {}",
            story.kind.label(),
            story.part_name
        )));
    }
    let mut replacements = Vec::with_capacity(original_spans.len());
    for (index, (left, right)) in original_spans.iter().zip(&edited_spans).enumerate() {
        let left_xml = &original[left.clone()];
        let right_xml = &edited[right.clone()];
        if original_owners[index].first() != edited_owners[index].first() {
            return Err(Error::Other(format!(
                "{} owner shell changed at {}[{index}]",
                story.kind.label(),
                story.part_name
            )));
        }
        if matches!(
            story.kind,
            ComparisonStoryKind::Footnote | ComparisonStoryKind::Endnote
        ) && !normal_note_owner(left_xml, original_bindings)?
        {
            if original_owners[index] != edited_owners[index]
                || (matches!(
                    raw_note_type_in_scope(left_xml, original_bindings)?,
                    RawNoteType::Empty | RawNoteType::Unknown
                ) && left_xml != right_xml)
            {
                return Err(Error::Other(format!(
                    "{} separator shell changed at {}[{index}]",
                    story.kind.label(),
                    story.part_name
                )));
            }
            replacements.push(left_xml.to_owned());
            continue;
        }
        let left_inner = element_inner_range_any_prefix(left_xml, owner_local)?;
        let right_inner = element_inner_range_any_prefix(right_xml, owner_local)?;
        let left_bindings = root_namespace_bindings(left_xml, owner_local, original_bindings)?;
        let right_bindings = root_namespace_bindings(right_xml, owner_local, edited_bindings)?;
        let tracked_inner = compare_story_inner(
            &left_xml[left_inner.clone()],
            &right_xml[right_inner],
            &format!(
                "{}:{}/{}[{index}]",
                story.kind.label(),
                story.part_name,
                owner_local
            ),
            word_prefix,
            &left_bindings,
            &right_bindings,
            metadata,
            diagnostics,
        )?;
        let mut owner = left_xml.to_owned();
        owner.replace_range(left_inner, &tracked_inner);
        replacements.push(owner);
    }
    let mut tracked = original.to_owned();
    for (span, replacement) in original_spans.into_iter().zip(replacements).rev() {
        tracked.replace_range(span, &replacement);
    }
    if root_inner_range(&tracked, story.kind.root_local())?.start != original_root.start
        || edited_root.start == usize::MAX
    {
        return Err(Error::Other(
            "comparison story root moved unexpectedly".to_owned(),
        ));
    }
    Ok(tracked)
}

pub(super) fn compare_story_inner(
    original: &str,
    edited: &str,
    location: &str,
    word_prefix: &str,
    original_bindings: &[(String, String)],
    edited_bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    // Formatting policy uses the same qualified paired host publisher as the
    // main story. Ignored text boxes keep the existing mask-and-restore scan.
    let scan_text_boxes = !metadata.options.ignore_formatting
        || story_ignored(metadata.options, ComparisonStoryKind::TextBox);
    compare_story_inner_impl(
        original,
        edited,
        location,
        word_prefix,
        original_bindings,
        edited_bindings,
        metadata,
        diagnostics,
        scan_text_boxes,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compare_story_inner_impl(
    original: &str,
    edited: &str,
    location: &str,
    word_prefix: &str,
    original_bindings: &[(String, String)],
    edited_bindings: &[(String, String)],
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
    scan_text_boxes: bool,
) -> Result<String> {
    let original_text_boxes = if scan_text_boxes {
        text_box_spans(original, word_prefix)?
    } else {
        Vec::new()
    };
    let edited_text_boxes = if scan_text_boxes {
        text_box_spans(edited, word_prefix)?
    } else {
        Vec::new()
    };
    if !original_text_boxes.is_empty() || !edited_text_boxes.is_empty() {
        if text_box_host_skeletons(original, &original_text_boxes)?
            != text_box_host_skeletons(edited, &edited_text_boxes)?
        {
            return Err(Error::Other(format!(
                "comparison cannot change nested text-box host shells at {location}"
            )));
        }
        if story_ignored(metadata.options, ComparisonStoryKind::TextBox) {
            let marker = text_box_marker_local(original, edited, word_prefix)?;
            let (masked_original, preserved) =
                mask_text_box_subtrees(original, word_prefix, &marker)?;
            let (masked_edited, _) = mask_text_box_subtrees(edited, word_prefix, &marker)?;
            let mut tracked = compare_story_inner_impl(
                &masked_original,
                &masked_edited,
                location,
                word_prefix,
                original_bindings,
                edited_bindings,
                metadata,
                diagnostics,
                false,
            )?;
            restore_text_box_subtrees(&mut tracked, &preserved, word_prefix, &marker, location)?;
            return Ok(tracked);
        }
        if original_text_boxes.len() != edited_text_boxes.len() {
            return Err(Error::Other(format!(
                "comparison cannot change nested text-box hosts at {location}"
            )));
        }
        let text_box_coordinates = text_box_coordinates(original, &original_text_boxes)?;
        let mut tracked_boxes = Vec::with_capacity(original_text_boxes.len());
        for ((run_ordinal, box_ordinal), (left, right)) in text_box_coordinates
            .into_iter()
            .zip(original_text_boxes.iter().zip(&edited_text_boxes))
        {
            let left_box = &original[left.clone()];
            let right_box = &edited[right.clone()];
            let left_inner = element_inner_range_any_prefix(left_box, "txbxContent")?;
            let right_inner = element_inner_range_any_prefix(right_box, "txbxContent")?;
            let left_bindings =
                root_namespace_bindings(left_box, "txbxContent", original_bindings)?;
            let right_bindings =
                root_namespace_bindings(right_box, "txbxContent", edited_bindings)?;
            let tracked_inner = compare_story_inner(
                &left_box[left_inner.clone()],
                &right_box[right_inner],
                &format!("{location}/text-box[{run_ordinal}:{box_ordinal}]"),
                word_prefix,
                &left_bindings,
                &right_bindings,
                metadata,
                diagnostics,
            )?;
            let mut tracked_box = left_box.to_owned();
            tracked_box.replace_range(left_inner, &tracked_inner);
            tracked_boxes.push(tracked_box);
        }
        let tracked_hosts = replace_text_boxes_in_hosts(
            original,
            &original_text_boxes,
            &tracked_boxes,
            word_prefix,
        )?;
        let marker = text_box_marker_local(original, edited, word_prefix)?;
        let (masked_original, _) = mask_text_box_subtrees(original, word_prefix, &marker)?;
        let (masked_edited, _) = mask_text_box_subtrees(edited, word_prefix, &marker)?;
        let mut tracked = compare_story_inner_impl(
            &masked_original,
            &masked_edited,
            location,
            word_prefix,
            original_bindings,
            edited_bindings,
            metadata,
            diagnostics,
            false,
        )?;
        restore_text_box_subtrees(&mut tracked, &tracked_hosts, word_prefix, &marker, location)?;
        return Ok(tracked);
    }
    let original_spans = story_content_spans(original, original_bindings)?;
    let edited_spans = story_content_spans(edited, edited_bindings)?;
    let original_document = story_document(original, original_bindings)?;
    let edited_document = story_document(edited, edited_bindings)?;
    if original_spans.len() != original_document.body.content.len()
        || edited_spans.len() != edited_document.body.content.len()
    {
        return Err(Error::Other(format!(
            "comparison could not correlate related-story owners at {location}"
        )));
    }
    let held_original = HeldRevisions::of(
        original,
        &original_spans,
        original_bindings,
        metadata.existing_revisions,
    )?;
    let held_edited = HeldRevisions::of(
        edited,
        &edited_spans,
        edited_bindings,
        metadata.existing_revisions,
    )?;
    compare_body(
        [&original_document, &edited_document],
        location,
        Some((original, &original_spans, original_bindings)),
        (edited, &edited_spans, edited_bindings),
        [&held_original, &held_edited],
        metadata,
        diagnostics,
    )
    .map_err(|error| Error::Other(format!("comparison failed in {location}: {error}")))
}

pub(super) fn story_content_spans(
    xml: &str,
    bindings: &[(String, String)],
) -> Result<Vec<Range<usize>>> {
    let (wrapped, offset) = story_wrapper(xml, bindings)?;
    let mut reader = NsReader::from_reader(wrapped.as_slice());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut open_elements = Vec::new();
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_section = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()
        );
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let is_section = is_section && element.local_name().as_ref() == b"sectPr";
                open_elements.push((depth == 2 && !is_section).then_some(before));
                depth += 1;
            }
            Event::Empty(element)
                if depth == 2 && !(is_section && element.local_name().as_ref() == b"sectPr") =>
            {
                spans.push(before.saturating_sub(offset)..after.saturating_sub(offset));
            }
            Event::Empty(_) => {}
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if let Some(Some(start)) = open_elements.pop() {
                    spans.push(start.saturating_sub(offset)..after.saturating_sub(offset));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(spans)
}

pub(super) fn story_document(
    inner: &str,
    namespace_bindings: &[(String, String)],
) -> Result<CT_Document> {
    let (xml, _) = story_wrapper(inner, namespace_bindings)?;
    // Full occurrence capture consumes this exact physical representation.
    // Descendant closure before capture would change opaque pict/MCE bytes.
    CT_Document::from_xml(&xml).map_err(Into::into)
}

impl HeldRevisions {
    /// Find the owners of story content `inner`, split by `spans`, that hold
    /// a modeled revision. A revision outside every owner is in the section
    /// properties, the only other modeled child of a story. All flags are
    /// clear when no input holds an existing revision.
    pub(super) fn of(
        inner: &str,
        spans: &[Range<usize>],
        bindings: &[(String, String)],
        existing_revisions: bool,
    ) -> Result<Self> {
        if !existing_revisions {
            return Ok(Self::default());
        }
        let (xml, offset) = story_wrapper(inner, bindings)?;
        let mut held = Self {
            owners: vec![false; spans.len()],
            source_metadata: vec![None; spans.len()],
            section: false,
            section_source: None,
        };
        for (range, _) in crate::revision::modeled_revision_spans(&xml)? {
            // The spans are in document order and do not overlap.
            let start = range.start.saturating_sub(offset);
            let index = spans.partition_point(|span| span.end <= start);
            match spans.get(index).filter(|span| span.contains(&start)) {
                Some(_) => held.owners[index] = true,
                None => held.section = true,
            }
        }
        for (index, span) in spans.iter().enumerate() {
            if held.owners[index] {
                let source = source_fragment(&inner[span.clone()], bindings)?;
                held.source_metadata[index] =
                    Some(oxoffice_docx_oxml::properties::protected_source_residual(
                        source.as_bytes(),
                        &[],
                    )?);
            }
        }
        if held.section {
            let source = source_fragment(&format!("<source>{inner}</source>"), bindings)?;
            if let Some(span) = direct_word_element_spans(&source, "sectPr")?.first() {
                held.section_source =
                    Some(oxoffice_docx_oxml::properties::protected_source_residual(
                        source_child(&source, span.clone())?.as_bytes(),
                        &[],
                    )?);
            }
        }
        Ok(held)
    }

    /// Both actual owning residuals must exist and agree.
    pub(super) fn same_owner_source(&self, index: usize, other: &Self, other_index: usize) -> bool {
        self.source_metadata
            .get(index)
            .and_then(Option::as_ref)
            .zip(
                other
                    .source_metadata
                    .get(other_index)
                    .and_then(Option::as_ref),
            )
            .is_some_and(|(left, right)| left == right)
    }

    pub(super) fn holds(&self, index: Option<usize>) -> bool {
        index.is_some_and(|index| self.owners.get(index).copied().unwrap_or(false))
    }
}

/// Wrap story content in a Word document and body that declare
/// `namespace_bindings`, and return the wrapper with the offset of `inner`.
pub(super) fn story_wrapper(
    inner: &str,
    namespace_bindings: &[(String, String)],
) -> Result<(Vec<u8>, usize)> {
    let mut writer = Writer::new(Vec::with_capacity(inner.len() + 256));
    let mut suffix = 0usize;
    let wrapper_prefix = loop {
        let candidate = if suffix == 0 {
            "rdocxcmp".to_owned()
        } else {
            format!("rdocxcmp{suffix}")
        };
        let binding_name = format!("xmlns:{candidate}");
        if namespace_bindings
            .iter()
            .all(|(name, _)| name != &binding_name)
        {
            break candidate;
        }
        suffix += 1;
    };
    let document_name = format!("{wrapper_prefix}:document");
    let body_name = format!("{wrapper_prefix}:body");
    let binding_name = format!("xmlns:{wrapper_prefix}");
    let mut document = BytesStart::new(document_name.as_str());
    document.push_attribute((binding_name.as_str(), W_NS));
    for (name, value) in namespace_bindings {
        document.push_attribute((name.as_str(), value.as_str()));
    }
    writer.write_event(Event::Start(document))?;
    writer.write_event(Event::Start(BytesStart::new(body_name.as_str())))?;
    let offset = writer.get_ref().len();
    writer.get_mut().extend_from_slice(inner.as_bytes());
    writer.write_event(Event::End(BytesEnd::new(body_name.as_str())))?;
    writer.write_event(Event::End(BytesEnd::new(document_name.as_str())))?;
    Ok((writer.into_inner(), offset))
}

pub(super) fn comparison_input(document: &Document) -> Result<Document> {
    let mut candidate = document.clone_for_staging();
    candidate.prepare_staged_package()?;
    let source = candidate
        .package
        .get_part(&candidate.doc_part_name)
        .ok_or_else(|| Error::Other(format!("missing main story {}", candidate.doc_part_name)))?;
    candidate.document = crate::document::DocumentModel::unproved(CT_Document::from_xml(source)?);
    Ok(candidate)
}
