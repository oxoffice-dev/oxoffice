//! Planning the records of an original capture.

use super::*;

impl OriginalCaptureWork {
    fn current_bytes(&self, pool: &CaptureDeclarations) -> Result<usize> {
        let mut current = self
            .attributes
            .iter()
            .try_fold(0usize, |sum, (_, bytes)| sum.checked_add(*bytes))
            .ok_or_else(|| OxmlError::InvalidValue("original capture attribute overflow".into()))?;
        for index in &self.namespaces {
            current = current
                .checked_add(2)
                .and_then(|bytes| bytes.checked_add(pool.prefix(*index).len()))
                .and_then(|bytes| bytes.checked_add(pool.namespace(*index).len()))
                .ok_or_else(|| OxmlError::InvalidValue("original capture scope overflow".into()))?;
        }
        Ok(current)
    }
    pub(super) fn admit(&self, source_bytes: usize, pool: &CaptureDeclarations) -> Result<()> {
        let allowance =
            crate::text::retained_owner_work(source_bytes, &[], self.current_bytes(pool)?)?;
        if self.needed > allowance {
            return Err(OxmlError::InvalidValue(
                "original source capture exceeds admitted source work".into(),
            ));
        }
        Ok(())
    }
}

// Install only existing inherited namespace facts for both real reader routes.
pub(super) fn inherited_capture_declarations(
    prefixes: &[String],
    pool: &mut CaptureDeclarations,
) -> WordSourceResult<Vec<u32>> {
    let reduced_inherited = reduce_inherited_bindings(prefixes);
    let mut inherited = Vec::with_capacity(reduced_inherited.len());
    for (prefix, namespace) in reduced_inherited {
        inherited.push(
            pool.intern(prefix, namespace)
                .map_err(WordSourceFailure::Resource)?,
        );
    }
    Ok(inherited)
}

/// Aggregate the retained namespace records a capture owner would create from
/// the ORIGINAL input, before any model construction. Physical declarations
/// are pooled once. Owners keep indices. Lengths are resolved from that pool,
/// so a shared URI is not copied once per owner. Duplicate stored declarations
/// still count. The allowance receives each original external fact once.
fn plan_original_capture_records(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
    pool: &mut CaptureDeclarations,
) -> WordSourceResult<(ConstructorFailures, OriginalCaptureWork)> {
    let inherited = inherited_capture_declarations(prefixes, pool)?;
    plan_capture_with_context(
        xml,
        &inherited,
        prefixes,
        root_owner,
        consumed_owner,
        pool,
        None,
    )
}

pub(super) fn plan_capture_with_context(
    xml: WordXmlInput<'_>,
    inherited: &[u32],
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
    pool: &mut CaptureDeclarations,
    source_reader: Option<WordSourceReader>,
) -> WordSourceResult<(ConstructorFailures, OriginalCaptureWork)> {
    let failures = qualify_constructor_failures(
        xml,
        inherited,
        pool,
        root_owner,
        prefixes,
        consumed_owner,
        source_reader,
    )?;
    let namespace_state = &mut CaptureNamespaceState::new(inherited, pool, prefixes);
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scope: Vec<OriginalCaptureScope> = Vec::new();
    let mut owners: Vec<OriginalCaptureOwner> = Vec::new();
    let mut discarded_index = 0usize;
    let mut part_event_index = 0usize;
    let mut total_needed = 0usize;
    let mut attribute_facts = Vec::new();
    let mut facts: Vec<u32> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let start_offset = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                let declared = crate::text::raw_namespace_declarations(start)?;
                let mut declarations = Vec::with_capacity(declared.len());
                for (prefix, namespace) in declared {
                    declarations.push(
                        pool.push(prefix, namespace)
                            .map_err(WordSourceFailure::Resource)?,
                    );
                }
                let part_event = failures
                    .part_events
                    .get(part_event_index)
                    .filter(|fact| fact.start == start_offset);
                if part_event.is_some() {
                    part_event_index += 1;
                }
                namespace_state.enter(&declarations, scope.len(), pool, None);
                if part_event.is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper))
                {
                    namespace_state
                        .retain_part_declarations(&part_event.expect("part wrapper").updates, pool);
                    declarations.clear();
                }
                let local = start.local_name();
                let name = start.name();
                let name = name.as_ref();
                let element_prefix = name
                    .iter()
                    .position(|byte| *byte == b':')
                    .map_or("", |separator| {
                        std::str::from_utf8(&name[..separator]).unwrap_or("")
                    });
                let word = original_capture_binding(element_prefix, namespace_state)
                    .is_some_and(|(index, _)| pool.namespace(index) == W_NS);
                let direct_run_child = owners
                    .last()
                    .is_some_and(|owner| owner.run && scope.len() == owner.depth + 1)
                    && !scope
                        .last()
                        .is_some_and(|parent| parent.raw || parent.dropped || parent.unconstructed);
                let action = crate::text::RunChildAction::for_names(
                    local.as_ref(),
                    empty,
                    original_capture_word_name(name, false, namespace_state, pool),
                    false,
                );
                let special_typed =
                    if direct_run_child && action == crate::text::RunChildAction::Special {
                        let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                        crate::text::RunChildPlan::read(start, &aliases, action)?
                            .leaf
                            .is_some()
                    } else {
                        false
                    };
                let comment_raw = if direct_run_child
                    && action == crate::text::RunChildAction::CommentReference
                {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    crate::text::comment_reference_has_payload_attributes(start, &aliases)
                        || (!empty && original_comment_reference_has_payload(*reader.get_ref())?)
                } else {
                    false
                };
                // A started note reference that holds anything stays raw, as
                // the run reader keeps it.
                let note_raw = direct_run_child
                    && !empty
                    && matches!(
                        action,
                        crate::text::RunChildAction::FootnoteReference
                            | crate::text::RunChildAction::EndnoteReference
                    )
                    && crate::text::started_reference_has_payload(*reader.get_ref())?;
                let modeled = direct_run_child
                    && !action.retains_raw(special_typed)
                    && !comment_raw
                    && !note_raw;
                let mut class = classify_original_capture(
                    empty,
                    word,
                    local.as_ref(),
                    modeled,
                    &scope,
                    &owners,
                );
                let raw_action = failures
                    .raw_subtrees
                    .binary_search_by_key(&start_offset, |fact| fact.span.start)
                    .ok()
                    .map(|index| &failures.raw_subtrees[index]);
                if raw_action.is_some_and(|fact| fact.control) {
                    class.raw = true;
                }
                while failures
                    .discarded
                    .get(discarded_index)
                    .is_some_and(|range| range.end <= start_offset)
                {
                    discarded_index += 1;
                }
                let discarded = failures
                    .discarded
                    .get(discarded_index)
                    .is_some_and(|range| range.contains(&start_offset));
                let unconstructed = scope.last().is_some_and(|parent| parent.unconstructed)
                    || raw_action.is_some_and(|fact| !fact.control)
                    || discarded;
                if discarded {
                    class.owner = false;
                }
                if class.owner {
                    let (record, attribute_bytes) =
                        original_capture_root_attributes(start, namespace_state, pool)?;
                    total_needed = total_needed.checked_add(record).ok_or_else(|| {
                        WordSourceFailure::resource("original capture work overflow".into())
                    })?;
                    if attribute_bytes > 0 {
                        attribute_facts.push((
                            start_offset..reader.buffer_position() as usize,
                            attribute_bytes,
                        ));
                    }
                    owners.push(OriginalCaptureOwner {
                        depth: scope.len(),
                        run: local.as_ref() == b"r",
                        shared: Vec::new(),
                        local: Vec::new(),
                        raw_copies: Vec::new(),
                        open_raw: Vec::new(),
                        open_comment: None,
                    });
                } else if let Some(owner) = owners.last_mut()
                    && !class.dropped
                    && (class.raw || class.inside_typed)
                {
                    if class.typed_root {
                        owner.open_raw.push(Vec::new());
                    }
                    if comment_raw {
                        owner.open_comment = Some((scope.len(), Vec::new()));
                    }
                    let used = original_capture_event_prefixes(start, namespace_state, pool)?;
                    for prefix in used {
                        let Some((index, depth)) =
                            original_capture_binding(&prefix, namespace_state)
                        else {
                            continue;
                        };
                        if let Some((root_depth, copy)) = &mut owner.open_comment {
                            if depth < *root_depth {
                                capture_index_push(copy, index);
                            }
                        } else {
                            charge_original_capture_binding(
                                owner,
                                index,
                                depth,
                                pool,
                                class.inside_typed,
                                namespace_state,
                            );
                        }
                    }
                    if empty
                        && comment_raw
                        && let Some((_, copy)) = owner.open_comment.take()
                    {
                        owner.raw_copies.push(copy);
                    }
                }
                if !empty {
                    let parent = scope.last();
                    let ruby_opaque = parent.is_some_and(|parent| parent.ruby_opaque)
                        || (word
                            && matches!(local.as_ref(), b"ruby" | b"rt" | b"rubyBase")
                            && capture_non_namespace_attribute(start));
                    let paragraph_result_root =
                        failures.paragraph_result_root == Some(start_offset);
                    let ordered_revision_root =
                        failures.ordered_revision_root == Some(start_offset);
                    let parent_paragraph = parent.is_some_and(|parent| parent.paragraph);
                    let revision =
                        word && matches!(local.as_ref(), b"ins" | b"del" | b"moveFrom" | b"moveTo");
                    let revision_runs = if ordered_revision_root {
                        true
                    } else if revision && !ruby_opaque {
                        capture_revision_opens_runs(start, parent_paragraph, namespace_state, pool)?
                    } else {
                        false
                    };
                    let parsed_hyperlink = local.as_ref() == b"hyperlink"
                        && (scope.is_empty()
                            || parent.is_some_and(|parent| parent.constructs_hyperlinks));
                    // A paragraph retains these bytes, but accepted reads also
                    // parse their contents as an ordinary synthetic paragraph.
                    // Keep that dispatch inside the original reservation. Raw
                    // wrappers inside a run or an opaque payload do not expand.
                    let wrapper_paragraph = word
                        && matches!(local.as_ref(), b"smartTag" | b"customXml")
                        && parent.is_some_and(|parent| {
                            parent.paragraph && !parent.raw && !parent.dropped
                        });
                    let inline_drawing = parent.is_some_and(|parent| parent.inline_drawing)
                        || (class.typed_root
                            && crate::drawing::DrawingContainerKind::for_name(
                                local.as_ref(),
                                empty,
                            ) == Some(crate::drawing::DrawingContainerKind::Inline));
                    let text_span = parent.is_some_and(|parent| parent.text_span)
                        || (direct_run_child
                            && matches!(
                                action,
                                crate::text::RunChildAction::Text
                                    | crate::text::RunChildAction::DeletedText
                            ));
                    let constructs_runs = !text_span
                        && !inline_drawing
                        && !ruby_opaque
                        && !unconstructed
                        && word
                        && (matches!(
                            local.as_ref(),
                            b"p" | b"rt" | b"rubyBase" | b"fldSimple" | b"sdtContent"
                        ) || wrapper_paragraph
                            || parsed_hyperlink
                            || revision_runs);
                    let constructs_hyperlinks = !text_span
                        && !inline_drawing
                        && !ruby_opaque
                        && !unconstructed
                        && word
                        && (local.as_ref() == b"p"
                            || wrapper_paragraph
                            || revision_runs
                            || paragraph_result_root);
                    scope.push(OriginalCaptureScope {
                        declarations,
                        raw: class.raw,
                        typed_drawing: class.inside_typed,
                        inline_drawing,
                        text_span,
                        drawing_root: class.drawing_root,
                        dropped: class.dropped,
                        raw_copy: class.typed_root,
                        constructs_runs,
                        constructs_hyperlinks,
                        paragraph: !text_span
                            && !inline_drawing
                            && word
                            && (local.as_ref() == b"p"
                                || wrapper_paragraph
                                || paragraph_result_root)
                            && !ruby_opaque
                            && !unconstructed,
                        ruby_opaque,
                        unconstructed,
                        comment_root: comment_raw,
                    });
                } else {
                    if let Some(owner) = owners.pop_if(|owner| owner.depth == scope.len()) {
                        finish_original_capture_owner(owner, &mut total_needed, &mut facts, pool)
                            .map_err(WordSourceFailure::Resource)?;
                    }
                    namespace_state.leave(&declarations, pool);
                }
            }
            Event::End(_) => {
                if scope.last().is_some_and(|frame| frame.comment_root)
                    && let Some(owner) = owners.last_mut()
                    && let Some((_, copy)) = owner.open_comment.take()
                {
                    owner.raw_copies.push(copy);
                }
                if scope.last().is_some_and(|frame| frame.raw_copy)
                    && let Some(owner) = owners.last_mut()
                    && let Some(copy) = owner.open_raw.pop()
                    && !copy.is_empty()
                {
                    owner.raw_copies.push(copy);
                }
                if let Some(scope) = scope.pop() {
                    namespace_state.leave(&scope.declarations, pool);
                }
                if let Some(owner) = owners.pop_if(|owner| owner.depth == scope.len()) {
                    finish_original_capture_owner(owner, &mut total_needed, &mut facts, pool)
                        .map_err(WordSourceFailure::Resource)?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((
        failures,
        OriginalCaptureWork {
            needed: total_needed,
            attributes: attribute_facts,
            namespaces: facts,
        },
    ))
}

pub(super) fn admit_original_capture_records(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
) -> WordSourceResult<ConstructorFailures> {
    let mut pool = CaptureDeclarations::default();
    let (constructors, work) =
        plan_original_capture_records(xml, prefixes, root_owner, consumed_owner, &mut pool)?;
    work.admit(xml.len().map_err(WordSourceFailure::Resource)?, &pool)
        .map_err(WordSourceFailure::Resource)?;
    Ok(constructors)
}
