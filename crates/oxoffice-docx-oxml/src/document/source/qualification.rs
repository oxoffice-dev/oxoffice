//! Constructor qualification: which constructors a source may use, and why one fails.

use super::*;

fn paragraph_constructor_kind(local: &[u8], empty: bool) -> ConstructorKind {
    use ConstructorKind as K;
    match local {
        b"r" => K::Run,
        b"hyperlink" => K::Hyperlink,
        b"fldSimple" => K::SimpleFieldUnknown,
        b"ruby" if !empty => K::Ruby,
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => K::Revision(false),
        b"pPr" if !empty => K::ParagraphProperties,
        b"sdt" if !empty => K::Control(SdtOwner::Inline),
        _ => K::Opaque,
    }
}

fn constructor_child_kind(
    mut parent: Option<&mut ConstructorFrame<'_>>,
    local: &[u8],
    word: bool,
    empty: bool,
    root_owner: Option<SdtOwner>,
) -> ConstructorKind {
    use ConstructorKind as K;
    if let Some(parent) = parent.as_deref_mut() {
        match parent.kind {
            K::ParagraphCollection => {
                return if word && local == b"p" {
                    K::Paragraph
                } else {
                    K::Unconstructed
                };
            }
            K::Notes => {
                return if word && matches!(local, b"footnote" | b"endnote") {
                    K::ParagraphCollection
                } else {
                    K::Unconstructed
                };
            }
            K::Comments => {
                return if word && local == b"comment" {
                    K::ParagraphCollection
                } else {
                    K::Unconstructed
                };
            }
            _ => {}
        }
        if matches!(parent.kind, K::Revision(_)) {
            if parent.failed {
                return K::Opaque;
            }
            if parent.revision_stopped {
                return K::Unconstructed;
            }
            match parent.revision_mode {
                Some(crate::revision::RevisionBodyMode::Ordered) => {
                    // This actual factory calls CT_P with RevisionSourceFacts.
                    // Its physical lane captures direct simple fields raw.
                    // Later complex ownership/projection remains unproved here.
                    return if word && local == b"fldSimple" {
                        K::Unconstructed
                    } else if word {
                        paragraph_constructor_kind(local, empty)
                    } else {
                        K::Opaque
                    };
                }
                Some(crate::revision::RevisionBodyMode::Legacy(kind)) => {
                    use crate::revision::RevisionContentAction as A;
                    return match A::for_names(Some(kind), local, word, empty) {
                        A::Run => K::Run,
                        A::NestedRevision => K::Revision(true),
                        A::Hyperlink => K::RevisionHyperlink,
                        A::PriorRunProperties => {
                            parent.revision_stopped = true;
                            K::RunProperties
                        }
                        A::PriorParagraphProperties => {
                            parent.revision_stopped = true;
                            K::ParagraphProperties
                        }
                        A::PriorTableProperties => {
                            parent.revision_stopped = true;
                            K::TableProperties
                        }
                        A::PriorSectionProperties => {
                            parent.revision_stopped = true;
                            K::SectionProperties
                        }
                        A::Skip => K::Unconstructed,
                    };
                }
                None => {
                    // Deletion/moveFrom still require their actual whole-source
                    // ordered-owner decision. Only this direct-run action is
                    // identical in both existing readers, never whole success.
                    return if word && !empty && local == b"r" {
                        K::Run
                    } else {
                        K::Opaque
                    };
                }
            }
        }
        if matches!(parent.kind, K::RevisionHyperlink) {
            use crate::revision::RevisionContentAction as A;
            return match A::for_names(None, local, word, empty) {
                A::Run => K::Run,
                A::NestedRevision => K::Revision(true),
                A::Hyperlink => K::RevisionHyperlink,
                _ => K::Unconstructed,
            };
        }
        if parent.ruby_none_at.is_some() {
            return K::Opaque;
        }
        if matches!(parent.kind, K::Ruby) {
            if !parent.known || parent.failed {
                return K::Opaque;
            }
            return match crate::ruby::RubyChildAction::for_names(local, word, empty) {
                crate::ruby::RubyChildAction::Properties if !empty => K::RubyProperties,
                crate::ruby::RubyChildAction::Properties => K::Opaque,
                crate::ruby::RubyChildAction::Text | crate::ruby::RubyChildAction::Base => {
                    K::RubyContent
                }
                _ => {
                    // Raw children are captured now. Their namespace completion
                    // happens only after the root reader reaches success.
                    parent.content_seen = true;
                    K::Opaque
                }
            };
        }
        if matches!(parent.kind, K::RunText) {
            parent.content_seen = true;
            parent.known = false;
        }
        if matches!(parent.kind, K::RubyContent) {
            return if !parent.content_seen
                && crate::ruby::RubyContentAction::for_names(local, word)
                    == crate::ruby::RubyContentAction::Run
            {
                K::Run
            } else {
                parent.content_seen = true;
                parent.known = false;
                K::Opaque
            };
        }
    }
    if let Some(parent) = parent.as_deref() {
        if matches!(parent.kind, K::Drawing) {
            return crate::drawing::DrawingContainerKind::for_name(local, empty)
                .map_or(K::Opaque, K::DrawingContainer);
        }
        if matches!(
            parent.kind,
            K::DrawingContainer(_) | K::DrawingWalk(_) | K::DrawingConsumed
        ) {
            return K::Opaque;
        }
    }
    if !word {
        return K::Opaque;
    }
    let named = || match local {
        b"p" => K::Paragraph,
        b"r" => K::Run,
        b"tbl" => K::Table,
        b"tr" => K::Row,
        b"tc" => K::Cell,
        _ => K::Opaque,
    };
    let Some(parent) = parent else {
        return match local {
            b"document" => K::Document,
            b"footnotes" | b"endnotes" => K::Notes,
            b"comments" => K::Comments,
            b"glossaryDocument" => K::Glossary,
            b"hdr" | b"ftr" => K::ParagraphCollection,
            b"body" | b"comment" | b"footnote" | b"endnote" | b"docPartBody" | b"txbxContent" => {
                K::Body
            }
            b"sdt" => root_owner.map_or(K::Opaque, K::Control),
            _ => named(),
        };
    };
    match parent.kind {
        K::Document if local == b"body" => K::Body,
        K::Glossary if local == b"docParts" => K::DocParts,
        K::DocParts if local == b"docPart" => K::DocPart,
        K::DocPart if local == b"docPartBody" => K::Body,
        K::Body | K::Cell => match local {
            b"tcPr" if !empty && matches!(parent.kind, K::Cell) => K::CellProperties,
            b"p" | b"tbl" => named(),
            b"sdt" if !empty => K::Control(if matches!(parent.kind, K::Cell) {
                SdtOwner::Cell
            } else {
                SdtOwner::Body
            }),
            _ => K::Opaque,
        },
        K::Table => match local {
            b"tblPr" if !empty => K::TableProperties,
            b"tblGrid" if !empty => K::TableGrid,
            b"tr" => K::Row,
            b"sdt" if !empty => K::Control(SdtOwner::Table),
            _ => K::Opaque,
        },
        K::Row => match local {
            b"trPr" if !empty => K::RowProperties,
            b"tc" => K::Cell,
            b"sdt" if !empty => K::Control(SdtOwner::Row),
            _ => K::Opaque,
        },
        K::Paragraph => paragraph_constructor_kind(local, empty),
        K::Hyperlink | K::SimpleFieldResult if local == b"r" => K::Run,
        K::Hyperlink if local == b"fldSimple" => K::SimpleFieldUnknown,
        K::Hyperlink if matches!(local, b"ins" | b"del" | b"moveFrom" | b"moveTo") => {
            K::Revision(true)
        }
        K::RunProperties if matches!(local, b"ins" | b"del" | b"rPrChange") => K::Revision(true),
        K::ParagraphProperties if local == b"pPrChange" => K::Revision(true),
        K::ParagraphProperties if local == b"numPr" && !empty => K::NumberingProperties,
        K::NumberingProperties if local == b"ins" => K::Revision(true),
        K::SectionProperties if local == b"sectPrChange" => K::Revision(true),
        K::TableProperties if local == b"tblPrChange" => K::Revision(true),
        K::RowProperties if matches!(local, b"ins" | b"del" | b"moveFrom") => K::Revision(true),
        K::ControlContent(_)
            if matches!(
                local,
                b"ins"
                    | b"del"
                    | b"moveFrom"
                    | b"moveTo"
                    | b"rPrChange"
                    | b"pPrChange"
                    | b"tblPrChange"
                    | b"sectPrChange"
            ) =>
        {
            K::Revision(true)
        }
        K::TableProperties if local == b"tblBorders" && !empty => K::TableBorders,
        K::CellProperties if local == b"tcBorders" && !empty => K::TableBorders,
        K::TableProperties if local == b"tblCellMar" && !empty => K::TableMargins,
        K::CellProperties if local == b"tcMar" && !empty => K::TableMargins,
        K::Run if !empty && matches!(local, b"t" | b"delText") => K::RunText,
        K::Run if local == b"drawing" && !empty => K::Drawing,
        K::Run | K::ParagraphProperties if local == b"rPr" && !empty => K::RunProperties,
        K::ParagraphProperties if local == b"sectPr" && !empty => K::SectionProperties,
        K::ParagraphProperties if local == b"pBdr" && !empty => K::ParagraphBorders,
        K::ParagraphProperties if local == b"tabs" && !empty => K::Tabs,
        K::Control(_) if local == b"sdtPr" && !empty && !parent.properties_seen => {
            parent.properties_seen = true;
            K::ControlProperties
        }
        K::Control(owner) if local == b"sdtContent" && !empty && !parent.content_seen => {
            parent.content_seen = true;
            K::ControlContent(owner)
        }
        K::ControlContent(SdtOwner::Inline) if local == b"fldSimple" => K::SimpleFieldUnknown,
        K::ControlContent(owner) if local == b"sdt" && !empty => K::Control(owner),
        K::ControlContent(owner) if owner.owns(local) => named(),
        _ => K::Opaque,
    }
}

/// Replay only a completed child's physical opening through the same header
/// reader and scalar decoder. The outer result retains reader/context errors,
/// while the inner result is the actual property constructor outcome.
fn ruby_property_after_capture(
    (input, length): (WordXmlInput<'_>, usize),
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Result<crate::ruby::RubyPropertyChild>> {
    let mut reader = Reader::from_reader(std::io::Read::take(input, length as u64));
    let mut buffer = Vec::new();
    match reader.read_event_into(&mut buffer)? {
        Event::Start(start) => {
            let aliases = constructor_attribute_prefixes(&start, namespace_state, pool)?;
            Ok(crate::ruby::RubyPropertyChild::read(&start, &aliases))
        }
        _ => Err(OxmlError::InvalidValue(
            "Missing captured Ruby property start".into(),
        )),
    }
}

/// Negative facts only. Unknown constructor families do not become valid here.
/// Each source event is visited once, with namespace facts pooled once and no
/// recursively owned model or expanded capture record constructed.
pub(super) fn qualify_constructor_failures(
    xml: WordXmlInput<'_>,
    inherited: &[u32],
    pool: &mut CaptureDeclarations,
    root_owner: Option<SdtOwner>,
    word_aliases: &[String],
    consumed_owner: Option<&[u8]>,
    source_reader: Option<WordSourceReader>,
) -> WordSourceResult<ConstructorFailures> {
    let part_reader = match source_reader {
        Some(WordSourceReader::Part(reader)) => Some(reader),
        _ => None,
    };
    let bare: std::collections::HashSet<&str> = word_aliases
        .iter()
        .filter(|alias| !alias.starts_with('\0'))
        .map(String::as_str)
        .collect();
    let mut first_bindings = std::collections::HashMap::new();
    let primitive_context_stable = word_aliases
        .iter()
        .filter_map(|alias| alias.strip_prefix('\0')?.split_once('\0'))
        .all(|(prefix, uri)| {
            let same = first_bindings
                .insert(prefix, uri)
                .is_none_or(|old| old == uri);
            same && (uri != W_NS || bare.contains(prefix))
        });
    let namespace_state = &mut CaptureNamespaceState::new(inherited, pool, word_aliases);
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = Vec::<OriginalCaptureScope>::new();
    let mut frames = Vec::<ConstructorFrame<'_>>::new();
    if let Some(owner) = consumed_owner {
        frames.push(ConstructorFrame {
            kind: if owner == b"p" {
                ConstructorKind::Paragraph
            } else {
                ConstructorKind::Run
            },
            start: 0,
            failed: false,
            content_seen: false,
            properties_seen: false,
            control_occurrence: None,
            root_attribute_failed: false,
            known: false,
            ruby_none_at: None,
            ruby_property_opening: None,
            revision_mode: None,
            revision_stopped: false,
            lexical_revision: false,
            revision_height: 0,
            drawing_stopped: false,
            drawing_has_id: false,
            drawing_metadata_opening: None,
            drawing_owner: None,
            source_depth: 0,
            drawing_prefix_unknown: false,
        });
    }
    let mut failures = ConstructorFailures {
        raw_subtrees: Vec::new(),
        root_raw: false,
        controls: Vec::new(),
        discarded: Vec::new(),
        simple_headers: Vec::new(),
        part_events: Vec::new(),
        paragraph_result_root: None,
        ordered_revision_root: None,
        selector_paragraph_root: None,
        simple_selectors: Vec::new(),
    };
    let collect_selectors = matches!(source_reader, Some(WordSourceReader::StandaloneParagraph))
        && word_aliases.is_empty()
        && inherited.is_empty()
        && consumed_owner.is_none()
        && root_owner.is_none();
    let mut selector: Option<DirectSimpleSelectorFold> = None;
    // The actual detached Field reader starts with this decoder. An outer
    // declaration or a later declaration cannot silently supply another role.
    let selector_decoder = Reader::from_reader(&b""[..]).decoder();
    let mut buffer = Vec::new();
    loop {
        let start_offset = reader.buffer_position() as usize;
        let opening_input = *reader.get_ref();
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        if let Some(active) = &mut selector {
            active.reusable &= reader.decoder() == selector_decoder;
        }
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                let mut declarations = Vec::new();
                let mut literal_namespaces = Vec::new();
                let mut raw_namespaces = start.attributes().filter_map(|attribute| {
                    let attribute = attribute.ok()?;
                    let name = attribute.key.as_ref();
                    (name == b"xmlns" || name.starts_with(b"xmlns:")).then_some(attribute.value)
                });
                for (prefix, namespace) in crate::text::raw_namespace_declarations(start)? {
                    let raw = raw_namespaces
                        .next()
                        .expect("same validated namespace attributes");
                    literal_namespaces.push(raw.as_ref() == namespace.as_bytes());
                    declarations.push(
                        pool.push(prefix, namespace)
                            .map_err(WordSourceFailure::Resource)?,
                    );
                }
                let global_reader = part_reader.filter(|_| {
                    frames
                        .last()
                        .is_none_or(|frame| matches!(frame.kind, ConstructorKind::PartWrapper))
                });
                namespace_state.enter(&declarations, scopes.len(), pool, Some(&literal_namespaces));
                let local = start.local_name();
                let word =
                    original_capture_word_name(start.name().as_ref(), false, namespace_state, pool);
                if collect_selectors {
                    if let Some(active) = &mut selector {
                        active.element(
                            local.as_ref(),
                            word,
                            empty,
                            scopes.len() + 1,
                            reader.buffer_position() as usize,
                        );
                    } else if !empty && scopes.is_empty() && word && local.as_ref() == b"p" {
                        failures.selector_paragraph_root =
                            Some(start_offset..reader.buffer_position() as usize);
                    } else if !empty
                        && scopes.len() == 1
                        && failures.selector_paragraph_root.is_some()
                        && word
                        && local.as_ref() == b"fldSimple"
                    {
                        selector = Some(DirectSimpleSelectorFold {
                            opening: start_offset..reader.buffer_position() as usize,
                            result_start: reader.buffer_position() as usize,
                            ordered: false,
                            result_seen: false,
                            traversed: 1,
                            consumed: None,
                            reusable: reader.decoder() == selector_decoder,
                        });
                    }
                }
                let part_action =
                    global_reader.map(|mode| mode.action(local.as_ref(), word, empty));
                if let Some(kind) = part_action {
                    failures.part_events.push(PartReaderEvent {
                        start: start_offset,
                        kind,
                        updates: if matches!(kind, ConstructorKind::PartWrapper) {
                            declarations.clone()
                        } else {
                            Vec::new()
                        },
                    });
                    if matches!(kind, ConstructorKind::PartWrapper) {
                        namespace_state.retain_part_declarations(&declarations, pool);
                        declarations.clear();
                    }
                }
                let word_name = |name: &[u8], attribute: bool| {
                    original_capture_word_name(name, attribute, namespace_state, pool)
                };
                let drawing_owner = frames.last().and_then(|parent| {
                    if matches!(parent.kind, ConstructorKind::DrawingContainer(_)) {
                        Some(frames.len() - 1)
                    } else {
                        parent.drawing_owner
                    }
                });
                let physical_since =
                    drawing_owner.map_or(usize::MAX, |index| frames[index].source_depth);
                let doc_pr = constructor_namespace_name(
                    start.name().as_ref(),
                    b"docPr",
                    crate::drawing::drawing_ns::WP,
                    namespace_state,
                    pool,
                    physical_since,
                );
                // The actual independent metadata postscan sees the complete
                // captured container, including consumed or late payloads.
                if primitive_context_stable && doc_pr {
                    let mut owner = drawing_owner;
                    while let Some(index) = owner {
                        if frames[index].drawing_metadata_opening.is_some() {
                            break;
                        }
                        frames[index].drawing_metadata_opening = Some((
                            opening_input,
                            reader.buffer_position() as usize - start_offset,
                        ));
                        // Each ancestor is initialized once. A previously seen
                        // inner owner already initialized its entire chain.
                        owner = frames[index].drawing_owner;
                    }
                }
                let drawing_kind = frames.last().and_then(|parent| match parent.kind {
                    ConstructorKind::DrawingContainer(kind)
                    | ConstructorKind::DrawingWalk(kind) => Some(kind),
                    _ => None,
                });
                let mut drawing_child = None;
                let decorative_parent = frames.last().and_then(|parent| match parent.kind {
                    ConstructorKind::DrawingDocPr => Some((0, true)),
                    ConstructorKind::DrawingDecorative(depth, admitted) => {
                        Some((depth + 1, admitted))
                    }
                    _ => None,
                });
                if primitive_context_stable && let Some((depth, admitted)) = decorative_parent {
                    if frames.last().is_some_and(|parent| parent.failed) {
                        drawing_child = Some(ConstructorKind::Unconstructed);
                    } else {
                        let outcome = crate::drawing::DecorativeChildPlan::read(
                            start,
                            depth,
                            admitted,
                            constructor_namespace_name(
                                start.name().as_ref(),
                                start.local_name().as_ref(),
                                crate::drawing::drawing_ns::A,
                                namespace_state,
                                pool,
                                physical_since,
                            ),
                            constructor_namespace_name(
                                start.name().as_ref(),
                                start.local_name().as_ref(),
                                crate::drawing::DECORATIVE_NAMESPACE,
                                namespace_state,
                                pool,
                                physical_since,
                            ),
                            start.decoder(),
                        );
                        drawing_child = Some(match outcome {
                            Ok(plan) if !empty => {
                                ConstructorKind::DrawingDecorative(depth, plan.admitted)
                            }
                            Ok(_) => ConstructorKind::Opaque,
                            Err(_) => {
                                frames.last_mut().expect("decorative owner").failed = true;
                                ConstructorKind::Unconstructed
                            }
                        });
                    }
                }
                if primitive_context_stable && let Some(kind) = drawing_kind {
                    use crate::drawing::DrawingChildAction as A;
                    let action = crate::drawing::DrawingChildAction::for_name(
                        kind,
                        start.name().as_ref(),
                        empty,
                        doc_pr,
                        constructor_namespace_name(
                            start.name().as_ref(),
                            b"txbxContent",
                            W_NS,
                            namespace_state,
                            pool,
                            physical_since,
                        ),
                    );
                    let parent = frames.last_mut().expect("drawing lane");
                    if parent.drawing_stopped || parent.failed {
                        drawing_child = Some(ConstructorKind::Unconstructed);
                    } else {
                        if action == A::TextBox {
                            parent.drawing_prefix_unknown = true;
                        }
                        if !parent.drawing_prefix_unknown {
                            match action {
                                A::Extent => {
                                    parent.failed |= crate::drawing::read_extent(
                                        start,
                                        &mut crate::units::Emu(0),
                                        &mut crate::units::Emu(0),
                                    )
                                    .is_err()
                                }
                                A::SourceRect => {
                                    parent.failed |=
                                        crate::drawing::parse_source_rect(start).is_err()
                                }
                                A::PositionH | A::PositionV => {
                                    parent.failed |=
                                        crate::drawing::DrawingPositionReference::read(
                                            start,
                                            action == A::PositionH,
                                        )
                                        .is_err();
                                }
                                A::DocPr => {
                                    for attribute in start.attributes() {
                                        let outcome = attribute.map_err(OxmlError::from).and_then(
                                            |attribute| {
                                                Ok(matches!(
                                                    crate::drawing::DrawingDocPrAttribute::read(
                                                        &attribute,
                                                        start.decoder()
                                                    )?,
                                                    crate::drawing::DrawingDocPrAttribute::Id(_)
                                                ))
                                            },
                                        );
                                        match outcome {
                                            Ok(id) => parent.drawing_has_id |= id,
                                            Err(_) => {
                                                parent.failed = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        drawing_child = Some(if parent.failed {
                            ConstructorKind::Unconstructed
                        } else if empty {
                            ConstructorKind::Opaque
                        } else {
                            match action {
                                A::Walk => ConstructorKind::DrawingWalk(kind),
                                A::DocPr if !parent.drawing_prefix_unknown => {
                                    ConstructorKind::DrawingDocPr
                                }
                                _ => ConstructorKind::DrawingConsumed,
                            }
                        });
                    }
                }
                if let Some(frame) = frames.last_mut()
                    && matches!(frame.kind, ConstructorKind::RubyContent)
                    && frame.known
                    && !frame.failed
                    && crate::ruby::RubyContentAction::for_names(local.as_ref(), word)
                        == crate::ruby::RubyContentAction::Reject
                {
                    frame.ruby_none_at = Some(start_offset);
                }
                if frames.last().is_some_and(|frame| {
                    matches!(frame.kind, ConstructorKind::Ruby) && frame.known && !frame.failed
                }) && word
                    && empty
                    && matches!(local.as_ref(), b"rt" | b"rubyBase")
                {
                    frames.last_mut().expect("ruby").ruby_none_at = Some(start_offset);
                }
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::Run))
                {
                    let action =
                        crate::text::RunChildAction::for_names(local.as_ref(), empty, word, false);
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let parent = frames.last_mut().expect("run parent");
                    match crate::text::RunChildPlan::read(start, &aliases, action) {
                        Err(_) => parent.failed = true,
                        Ok(plan) => {
                            let simple = plan.leaf.is_some()
                                || matches!(
                                    action,
                                    crate::text::RunChildAction::IgnoreEmptyProperties
                                        | crate::text::RunChildAction::Properties
                                        | crate::text::RunChildAction::Text
                                        | crate::text::RunChildAction::DeletedText
                                );
                            let comment_raw = action
                                == crate::text::RunChildAction::CommentReference
                                && (crate::text::comment_reference_has_payload_attributes(
                                    start, &aliases,
                                ) || !empty);
                            // A started note reference reads as the empty one
                            // only once its content is known to be empty.
                            let started_note = !empty
                                && matches!(
                                    action,
                                    crate::text::RunChildAction::FootnoteReference
                                        | crate::text::RunChildAction::EndnoteReference
                                );
                            parent.known &= simple && !comment_raw && !started_note;
                        }
                    }
                }
                let mut ruby_property_opening = None;
                if frames.last().is_some_and(|frame| {
                    matches!(frame.kind, ConstructorKind::RubyProperties)
                        && frame.known
                        && frame.ruby_none_at.is_none()
                        && !frame.failed
                }) {
                    // Started children are captured before their scalar is read.
                    // Empty raw capture writes this exact event, but its namespace
                    // completion is deferred until the property reader succeeds.
                    if empty {
                        let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                        let outcome = crate::ruby::RubyPropertyChild::read(start, &aliases);
                        let parent = frames.last_mut().expect("ruby property owner");
                        parent.record_ruby_property(outcome);
                    } else {
                        ruby_property_opening = Some((
                            opening_input,
                            reader.buffer_position() as usize - start_offset,
                        ));
                    }
                }
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunProperties))
                {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let bold = if empty {
                        crate::run_properties::empty_bold_value(start, &aliases)
                    } else {
                        Ok(None)
                    };
                    let parent = frames.last_mut().expect("property owner");
                    match bold {
                        Ok(Some(_)) => {}
                        Ok(None) => parent.known = false,
                        Err(_) => {
                            parent.failed = true;
                            parent.known = false;
                        }
                    }
                }
                if empty
                    && word
                    && frames
                        .last()
                        .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunProperties))
                    && let Some(measurement) =
                        crate::run_properties::RunMeasurement::for_local(local.as_ref())
                {
                    let mut value = None;
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(OxmlError::from)?;
                        if matches_local_name(attribute.key.as_ref(), b"val")
                            && word_name(attribute.key.as_ref(), true)
                        {
                            value = Some(
                                std::str::from_utf8(&attribute.value)
                                    .map_err(OxmlError::from)?
                                    .to_owned(),
                            );
                            break;
                        }
                    }
                    if measurement
                        .decode(start.name().as_ref(), value.as_deref())
                        .is_err()
                    {
                        frames.last_mut().expect("property frame exists").failed = true;
                    }
                }
                if empty
                    && word
                    && frames.last().is_some_and(|frame| {
                        matches!(frame.kind, ConstructorKind::ParagraphProperties)
                    })
                {
                    let all_values = matches!(local.as_ref(), b"spacing" | b"ind" | b"framePr");
                    let first_value = matches!(local.as_ref(), b"divId" | b"outlineLvl");
                    if all_values || first_value {
                        let mut failed = false;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let key = attribute.key.as_ref();
                            let qualified = word_name(key, true);
                            if !all_values && !qualified {
                                continue;
                            }
                            let attribute_local = attribute.key.local_name();
                            let measurement = qualified
                                .then(|| {
                                    crate::paragraph_properties::ParagraphMeasurement::for_names(
                                        local.as_ref(),
                                        attribute_local.as_ref(),
                                    )
                                })
                                .flatten();
                            if !all_values && measurement.is_none() {
                                continue;
                            }
                            // Spacing/indent/frame construction decodes every attribute's
                            // UTF-8, while val-only leaves stop at their first value.
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if let Some(measurement) = measurement {
                                failed = measurement
                                    .decode(start.name().as_ref(), key, Some(value))
                                    .is_err();
                                if failed || first_value {
                                    break;
                                }
                            }
                        }
                        if failed {
                            frames.last_mut().expect("property frame exists").failed = true;
                        }
                    }
                }
                if word && let Some(parent) = frames.last() {
                    let compound = empty
                        && matches!(parent.kind, ConstructorKind::RunProperties)
                        && matches!(local.as_ref(), b"fitText" | b"eastAsianLayout");
                    let border = (empty
                        && matches!(parent.kind, ConstructorKind::RunProperties)
                        && local.as_ref() == b"bdr")
                        || (matches!(parent.kind, ConstructorKind::ParagraphBorders)
                            && matches!(
                                local.as_ref(),
                                b"top"
                                    | b"bottom"
                                    | b"left"
                                    | b"start"
                                    | b"right"
                                    | b"end"
                                    | b"between"
                                    | b"bar"
                            ));
                    let border = border
                        || (empty
                            && matches!(parent.kind, ConstructorKind::TableBorders)
                            && matches!(
                                local.as_ref(),
                                b"top"
                                    | b"bottom"
                                    | b"left"
                                    | b"start"
                                    | b"right"
                                    | b"end"
                                    | b"insideH"
                                    | b"insideV"
                            ));
                    let tab =
                        matches!(parent.kind, ConstructorKind::Tabs) && local.as_ref() == b"tab";
                    if compound || border || tab {
                        let mut failed = false;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if !word_name(attribute.key.as_ref(), true) {
                                continue;
                            }
                            let attribute_local = attribute.key.local_name();
                            if compound
                                && let Some(measurement) =
                                    crate::run_properties::RunCompoundMeasurement::for_names(
                                        local.as_ref(),
                                        attribute_local.as_ref(),
                                    )
                            {
                                failed = measurement
                                    .decode(start.name().as_ref(), attribute.key.as_ref(), value)
                                    .is_err();
                            } else if let Some(measurement) = if border {
                                crate::borders::BorderMeasurement::border_attribute(
                                    attribute_local.as_ref(),
                                )
                            } else if tab {
                                crate::borders::BorderMeasurement::tab_attribute(
                                    attribute_local.as_ref(),
                                )
                            } else {
                                None
                            } {
                                failed = measurement
                                    .decode(start.name().as_ref(), attribute.key.as_ref(), value)
                                    .is_err();
                            }
                            if failed {
                                break;
                            }
                        }
                        if failed {
                            frames.last_mut().expect("property parent exists").failed = true;
                        }
                    }
                }
                if word && let Some(parent) = frames.last() {
                    use ConstructorKind as K;
                    let table = match parent.kind {
                        K::TableProperties if empty => matches!(
                            local.as_ref(),
                            b"tblW"
                                | b"tblInd"
                                | b"tblCellSpacing"
                                | b"tblpPr"
                                | b"tblStyleRowBandSize"
                                | b"tblStyleColBandSize"
                        ),
                        K::RowProperties if empty => matches!(
                            local.as_ref(),
                            b"trHeight"
                                | b"gridBefore"
                                | b"gridAfter"
                                | b"wBefore"
                                | b"wAfter"
                                | b"tblCellSpacing"
                        ),
                        K::CellProperties => {
                            matches!(local.as_ref(), b"tcW" | b"gridSpan" | b"vMerge")
                        }
                        K::TableGrid => local.as_ref() == b"gridCol",
                        K::TableMargins if empty => matches!(
                            local.as_ref(),
                            b"top" | b"bottom" | b"left" | b"start" | b"right" | b"end"
                        ),
                        _ => false,
                    };
                    let section = empty
                        && matches!(parent.kind, K::SectionProperties)
                        && matches!(local.as_ref(), b"pgSz" | b"pgMar" | b"cols");
                    let control = matches!(parent.kind, K::ControlProperties)
                        .then(|| crate::content_control::ControlScalar::for_local(local.as_ref()))
                        .flatten();
                    let mut failed = false;
                    if let Some(scalar) = control {
                        let mut value = None;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            if matches_local_name(attribute.key.as_ref(), b"val")
                                && word_name(attribute.key.as_ref(), true)
                            {
                                value = Some(
                                    attribute
                                        .decoded_and_normalized_value(
                                            XmlVersion::Implicit1_0,
                                            start.decoder(),
                                        )
                                        .map_err(OxmlError::from)?
                                        .into_owned(),
                                );
                                break;
                            }
                        }
                        failed = scalar.decode(value).is_err();
                    } else if table || section {
                        let first_value = matches!(
                            local.as_ref(),
                            b"tblStyleRowBandSize"
                                | b"tblStyleColBandSize"
                                | b"gridBefore"
                                | b"gridAfter"
                                | b"gridSpan"
                                | b"vMerge"
                        );
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let key = attribute.key.as_ref();
                            let qualified = word_name(key, true);
                            let attr_local = attribute.key.local_name();
                            let margin = matches!(parent.kind, K::TableMargins);
                            let table_scalar = if margin && qualified && attr_local.as_ref() == b"w"
                            {
                                Some(crate::table::TableScalar::MarginWidth)
                            } else {
                                (table && qualified)
                                    .then(|| {
                                        crate::table::TableScalar::for_names(
                                            local.as_ref(),
                                            attr_local.as_ref(),
                                        )
                                    })
                                    .flatten()
                            };
                            let section_scalar = (section && qualified)
                                .then(|| {
                                    SectionScalar::for_names(local.as_ref(), attr_local.as_ref())
                                })
                                .flatten();
                            // Grid and margin readers decode only selected numeric attributes.
                            // Val-only readers stop at the first qualified val.
                            let selected_only = margin
                                || first_value
                                || matches!(local.as_ref(), b"gridCol" | b"pgMar");
                            if selected_only && table_scalar.is_none() && section_scalar.is_none() {
                                continue;
                            }
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if let Some(scalar) = table_scalar {
                                failed = scalar.decode(start.name().as_ref(), key, value).is_err();
                            } else if let Some(scalar) = section_scalar {
                                failed = scalar.decode(start.name().as_ref(), key, value).is_err();
                            }
                            if failed || first_value || margin {
                                break;
                            }
                        }
                    }
                    if failed {
                        frames.last_mut().expect("scalar owner exists").failed = true;
                    }
                }
                let root_revision = match source_reader {
                    Some(WordSourceReader::OrderedRevision { require_author })
                        if frames.is_empty()
                            && word
                            && crate::revision::revision_kind_for_local(local.as_ref())
                                .is_some() =>
                    {
                        Some(ConstructorKind::Revision(require_author))
                    }
                    _ => None,
                };
                let paragraph_result_root = matches!(source_reader, Some(WordSourceReader::SimpleParagraph { opening_end, .. })
                    if primitive_context_stable && frames.is_empty() && word && local.as_ref() == b"fldSimple" && reader.buffer_position() as usize == opening_end);
                if paragraph_result_root {
                    failures.paragraph_result_root = Some(start_offset);
                }
                let excluded_result_child = matches!(source_reader, Some(WordSourceReader::SimpleParagraph { result_start, result_end, .. })
                    if frames.len() == 1 && failures.paragraph_result_root == frames.first().map(|frame| frame.start)
                        && !(result_start..result_end).contains(&start_offset));
                let failed_result_child = frames.last().is_some_and(|parent| {
                    parent.failed && failures.paragraph_result_root == Some(parent.start)
                });
                let mut kind = (excluded_result_child || failed_result_child)
                    .then_some(ConstructorKind::Unconstructed)
                    .or(drawing_child)
                    .or(root_revision)
                    .or(paragraph_result_root.then_some(ConstructorKind::Paragraph))
                    .or(part_action)
                    .unwrap_or_else(|| {
                        constructor_child_kind(
                            frames.last_mut(),
                            local.as_ref(),
                            word,
                            empty,
                            root_owner,
                        )
                    });
                let lexical_revision =
                    word && crate::revision::revision_kind_for_local(local.as_ref()).is_some();
                let mut revision_mode = None;
                let failed = if matches!(kind, ConstructorKind::SimpleFieldUnknown) {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let header = crate::text::SimpleFieldHeader::read(start, &aliases, empty);
                    if simple_field_header_projection_stable(start, namespace_state, pool)? {
                        let raw = header.is_raw();
                        if !empty && primitive_context_stable {
                            let opening = start_offset..reader.buffer_position() as usize;
                            failures.simple_headers.push(SimpleHeaderOccurrence {
                                original_part: 0,
                                original_opening: opening.clone(),
                                opening,
                                header,
                            });
                        }
                        if raw {
                            kind = ConstructorKind::SimpleFieldRaw;
                            true
                        } else {
                            kind = ConstructorKind::SimpleFieldResult;
                            false
                        }
                    } else {
                        false
                    }
                } else if let ConstructorKind::Revision(require_author) = kind {
                    let prefixes = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    match crate::revision::RevisionHeader::read(start, &prefixes, require_author) {
                        Ok(header) => {
                            revision_mode = if root_revision.is_some() && primitive_context_stable {
                                failures.ordered_revision_root = Some(start_offset);
                                Some(crate::revision::RevisionBodyMode::Ordered)
                            } else {
                                crate::revision::RevisionBodyMode::without_source_probe(header.kind)
                            };
                            false
                        }
                        Err(_) => true,
                    }
                } else if primitive_context_stable
                    && matches!(
                        kind,
                        ConstructorKind::DrawingContainer(
                            crate::drawing::DrawingContainerKind::Anchor
                        )
                    )
                {
                    crate::drawing::AnchorAttributes::read(start).is_err()
                } else {
                    false
                };
                if empty
                    && primitive_context_stable
                    && matches!(kind, ConstructorKind::Run)
                    && root_attribute_plan_failed(start, namespace_state, pool)?
                    && let Some(parent) = frames.last_mut()
                {
                    parent.failed = true;
                }
                if empty && matches!(kind, ConstructorKind::Unconstructed) {
                    failures
                        .discarded
                        .push(start_offset..reader.buffer_position() as usize);
                }
                if empty && failed {
                    failures.raw_subtrees.push(RawConstructorSpan {
                        span: start_offset..reader.buffer_position() as usize,
                        control: false,
                    });
                }
                if empty && let Some(parent) = frames.last_mut() {
                    parent.revision_height =
                        parent.revision_height.max(usize::from(lexical_revision));
                }
                if !empty {
                    let control_occurrence = if let ConstructorKind::Control(owner) = kind {
                        let index = failures.controls.len();
                        failures.controls.push(ControlOccurrence {
                            original_part: 0,
                            owner,
                            source_span: start_offset..0,
                            after: 0,
                            raw: false,
                        });
                        Some(index)
                    } else {
                        None
                    };
                    let root_attribute_failed = primitive_context_stable
                        && matches!(kind, ConstructorKind::Run)
                        && root_attribute_plan_failed(start, namespace_state, pool)?;
                    frames.push(ConstructorFrame {
                        kind,
                        start: start_offset,
                        failed,
                        content_seen: false,
                        properties_seen: false,
                        control_occurrence,
                        root_attribute_failed,
                        revision_mode,
                        revision_stopped: false,
                        lexical_revision,
                        revision_height: 0,
                        drawing_stopped: false,
                        drawing_has_id: false,
                        drawing_metadata_opening: None,
                        drawing_owner,
                        source_depth: scopes.len(),
                        drawing_prefix_unknown: frames.last().is_some_and(|parent| {
                            matches!(
                                parent.kind,
                                ConstructorKind::DrawingContainer(_)
                                    | ConstructorKind::DrawingWalk(_)
                            ) && parent.drawing_prefix_unknown
                        }),
                        known: primitive_context_stable
                            && matches!(
                                kind,
                                ConstructorKind::Run
                                    | ConstructorKind::RunText
                                    | ConstructorKind::RunProperties
                                    | ConstructorKind::Ruby
                                    | ConstructorKind::RubyContent
                                    | ConstructorKind::RubyProperties
                            ),
                        ruby_property_opening,
                        ruby_none_at: if matches!(
                            kind,
                            ConstructorKind::Ruby | ConstructorKind::RubyContent
                        ) && crate::ruby::has_foreign_attributes(start)?
                        {
                            Some(start_offset)
                        } else {
                            None
                        },
                    });
                    scopes.push(OriginalCaptureScope {
                        declarations,
                        raw: false,
                        typed_drawing: false,
                        inline_drawing: false,
                        text_span: false,
                        drawing_root: false,
                        dropped: false,
                        raw_copy: false,
                        constructs_runs: false,
                        constructs_hyperlinks: false,
                        paragraph: false,
                        ruby_opaque: false,
                        unconstructed: false,
                        comment_root: false,
                    });
                } else {
                    namespace_state.leave(&declarations, pool);
                }
            }
            Event::End(ref end) => {
                if let Some(active) = &mut selector
                    && active.end(scopes.len(), reader.buffer_position() as usize)
                {
                    let active = selector.take().expect("completed selector owner");
                    let owner_start = active.opening.start;
                    if active.reusable {
                        failures.simple_selectors.push(SimpleSelectorOccurrence {
                            opening: active.opening,
                            whole: owner_start..reader.buffer_position() as usize,
                            ordered: active.ordered,
                            result: active.result_start - owner_start..start_offset - owner_start,
                        });
                    }
                }
                if let Some(opening) = frames.last().and_then(|frame| frame.ruby_property_opening) {
                    // Local declarations are still active here. No nested child
                    // scope survives this successful matching capture End.
                    let outcome = ruby_property_after_capture(opening, namespace_state, pool)?;
                    let parent_index = frames.len() - 2;
                    let parent = &mut frames[parent_index];
                    parent.record_ruby_property(outcome);
                }
                if let Some(scope) = scopes.pop() {
                    namespace_state.leave(&scope.declarations, pool);
                }
                if let Some(mut frame) = frames.pop() {
                    if let ConstructorKind::DrawingContainer(kind)
                    | ConstructorKind::DrawingWalk(kind) = frame.kind
                    {
                        frame.drawing_stopped |= kind.matches_end(end.name().as_ref());
                    }
                    if matches!(frame.kind, ConstructorKind::DrawingContainer(_))
                        && !frame.drawing_prefix_unknown
                    {
                        frame.failed |= !frame.drawing_has_id;
                        if !frame.failed {
                            frame.failed |= frame.drawing_metadata_opening.is_none_or(|opening| {
                                drawing_metadata_after_capture(opening).is_err()
                            });
                        }
                    }
                    let revision_height = crate::revision::revision_depth_height(
                        frame.lexical_revision,
                        frame.revision_height,
                    );
                    // The actual revision reader checks this entire lexical
                    // nesting before even its header/body. Other opaque frames
                    // carry height only, not typed constructor authority.
                    if primitive_context_stable
                        && matches!(frame.kind, ConstructorKind::Revision(_))
                        && crate::revision::revision_depth_exceeds(revision_height)
                    {
                        frame.failed = true;
                    }
                    if matches!(frame.kind, ConstructorKind::Unconstructed) {
                        failures
                            .discarded
                            .push(frame.start..reader.buffer_position() as usize);
                    }
                    if matches!(
                        frame.kind,
                        ConstructorKind::RubyProperties | ConstructorKind::Ruby
                    ) && frame.content_seen
                        && frame.ruby_none_at.is_none()
                    {
                        // Successful raw-property completion is a separate action.
                        // A direct None never reaches it in the actual reader.
                        frame.known = false;
                    }
                    frame.failed |= frame.root_attribute_failed;
                    if matches!(frame.kind, ConstructorKind::Ruby)
                        && !frame.failed
                        && let Some(start) = frame.ruby_none_at
                    {
                        failures
                            .discarded
                            .push(start..reader.buffer_position() as usize);
                    }
                    if let Some(parent) = frames.last_mut() {
                        parent.revision_height = parent.revision_height.max(revision_height);
                        if matches!(
                            parent.kind,
                            ConstructorKind::DrawingContainer(_) | ConstructorKind::DrawingWalk(_)
                        ) && matches!(frame.kind, ConstructorKind::DrawingWalk(_))
                        {
                            parent.drawing_stopped |= frame.drawing_stopped;
                            parent.drawing_has_id |= frame.drawing_has_id;
                            parent.drawing_prefix_unknown |= frame.drawing_prefix_unknown;
                        }
                        if matches!(
                            parent.kind,
                            ConstructorKind::RubyContent
                                | ConstructorKind::Ruby
                                | ConstructorKind::Run
                        ) && !(matches!(parent.kind, ConstructorKind::Ruby)
                            && matches!(frame.kind, ConstructorKind::Opaque))
                        {
                            parent.known &= frame.known && !frame.failed;
                        }
                        if matches!(
                            frame.kind,
                            ConstructorKind::RubyProperties | ConstructorKind::RubyContent
                        ) && !frame.failed
                            && let Some(start) = frame.ruby_none_at
                        {
                            parent.ruby_none_at = Some(start);
                        }
                    }
                    if let Some(index) = frame.control_occurrence {
                        let after = failures.controls.len();
                        let occurrence = &mut failures.controls[index];
                        occurrence.source_span.end = reader.buffer_position() as usize;
                        occurrence.after = after;
                        occurrence.raw = frame.failed;
                    }
                    if frame.failed && matches!(frame.kind, ConstructorKind::Control(_)) {
                        failures.raw_subtrees.push(RawConstructorSpan {
                            span: frame.start..reader.buffer_position() as usize,
                            control: true,
                        });
                        failures.root_raw |= frames.is_empty();
                        // A failed nested control is retained raw by its parent.
                    } else if frame.failed
                        && matches!(
                            frame.kind,
                            ConstructorKind::Revision(_) | ConstructorKind::SimpleFieldRaw
                        )
                    {
                        failures.raw_subtrees.push(RawConstructorSpan {
                            span: frame.start..reader.buffer_position() as usize,
                            control: false,
                        });
                        // Revisions swallow constructor errors. Simple fields swallow
                        // only an early header None, not errors from parsed children.
                    } else if frame.failed
                        && let Some(parent) = frames.last_mut()
                    {
                        parent.failed = true;
                    }
                }
            }
            Event::Text(ref text)
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunText)) =>
            {
                let frame = frames.last_mut().expect("text span");
                frame.failed |= crate::text::decode_run_text_span(text).is_err();
                frame.known &= !frame.content_seen;
                frame.content_seen = true;
            }
            event
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunText))
                    && !matches!(event, Event::Eof) =>
            {
                frames.last_mut().expect("text span").known = false;
            }
            event if crate::ruby::rejects_direct_characters(&event) => {
                if let Some(frame) = frames.last_mut()
                    && matches!(
                        frame.kind,
                        ConstructorKind::Ruby
                            | ConstructorKind::RubyProperties
                            | ConstructorKind::RubyContent
                    )
                    && frame.known
                    && !frame.failed
                {
                    frame.ruby_none_at = Some(start_offset);
                }
            }
            Event::Comment(_) | Event::PI(_)
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::Run)) =>
            {
                frames.last_mut().expect("run").known = false;
            }
            Event::Eof => {
                // capture_element deliberately accepts EOF. Discharge the same
                // pending scalar there, restoring its opening scope if an
                // unfinished descendant introduced additional declarations.
                if let Some(index) = frames
                    .iter()
                    .rposition(|frame| frame.ruby_property_opening.is_some())
                {
                    let remaining_scopes = scopes.len() - (frames.len() - index - 1);
                    while scopes.len() > remaining_scopes {
                        let scope = scopes.pop().expect("nested capture scope");
                        namespace_state.leave(&scope.declarations, pool);
                    }
                    let opening = frames[index]
                        .ruby_property_opening
                        .expect("pending opening");
                    let outcome = ruby_property_after_capture(opening, namespace_state, pool)?;
                    frames[index - 1].record_ruby_property(outcome);
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    failures
        .raw_subtrees
        .sort_unstable_by_key(|fact| fact.span.start);
    failures.discarded.sort_unstable_by_key(|span| span.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for span in failures.discarded {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    failures.discarded = merged;
    Ok(failures)
}

pub(crate) fn control_has_property_measurement_failure_in(
    xml: &[u8],
    prefixes: &[String],
    owner: SdtOwner,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<bool> {
    let mut pool = CaptureDeclarations::default();
    let mut inherited = Vec::new();
    for (prefix, namespace) in reduce_inherited_bindings(prefixes) {
        inherited.push(pool.intern(prefix, namespace).map_err(|error| {
            record_word_source_failure(WordSourceFailure::Resource(error), registry)
        })?);
    }
    let verdict = (|| -> WordSourceResult<bool> {
        Ok(qualify_constructor_failures(
            WordXmlInput::Contiguous(xml),
            &inherited,
            &mut pool,
            Some(owner),
            prefixes,
            None,
            None,
        )?
        .root_raw)
    })();
    verdict.map_err(|failure| record_word_source_failure(failure, registry))
}

pub(super) fn original_comment_reference_has_payload(input: WordXmlInput<'_>) -> Result<bool> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().allow_unmatched_ends = true;
    reader.config_mut().check_end_names = false;
    let mut buffer = Vec::new();
    Ok(!matches!(
        reader.read_event_into(&mut buffer)?,
        Event::End(_)
    ))
}
