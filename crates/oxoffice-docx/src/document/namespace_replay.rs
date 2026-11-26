//! Replaying nested namespace declarations into serialized Word XML.

use super::*;

/// One scan of `document_xml` decides both a plain local edit and the
/// diagnostic known before a replay. `nested_namespace_owners` is that scan
/// for this exact part. A root or body shadow blocks and records the
/// diagnostic. No owners leaves the edit local and records no diagnostic.
/// Compatible owners block and leave the diagnostic uncomputed. An unsafe
/// nested owner blocks and records the diagnostic. A failed scan blocks and
/// records no diagnostic.
pub(super) fn plain_local_edit_facts(
    root_declarations: &[(String, String)],
    body_declarations: &[(String, String)],
    document_xml: &[u8],
    nested_namespace_owners: &OnceLock<Option<Vec<NestedNamespaceOwner>>>,
) -> (bool, OnceLock<Option<String>>) {
    if let Some(prefix) =
        unsafe_serializer_namespace_prefix(root_declarations, body_declarations, Some(document_xml))
    {
        return (
            true,
            OnceLock::from(Some(shadowed_namespace_diagnostic(&prefix))),
        );
    }
    match nested_namespace_owners.get_or_init(|| nested_modeled_namespace_owners(document_xml).ok())
    {
        Some(owners) if owners.is_empty() => (false, OnceLock::from(None)),
        Some(owners) => match unsafe_nested_namespace_prefix(owners) {
            Some(prefix) => (
                true,
                OnceLock::from(Some(shadowed_namespace_diagnostic(&prefix))),
            ),
            None => (true, OnceLock::new()),
        },
        None => (true, OnceLock::from(None)),
    }
}

pub(super) fn replay_unsavable_body_edit(
    document_xml: &[u8],
    document: &CT_Document,
) -> Option<String> {
    let owners = match nested_modeled_namespace_owners(document_xml) {
        Ok(owners) if !owners.is_empty() => owners,
        _ => return None,
    };
    if let Some(prefix) = unsafe_nested_namespace_prefix(&owners) {
        return Some(shadowed_namespace_diagnostic(&prefix));
    }
    document
        .to_xml()
        .map_err(Error::from)
        .and_then(|xml| replay_nested_namespace_declarations(&xml, &owners))
        .err()
        .map(|error| error.to_string())
}

pub(super) fn replay_nested_namespace_declarations(
    xml: &[u8],
    owners: &[NestedNamespaceOwner],
) -> Result<Vec<u8>> {
    replay_nested_namespace_declarations_with_spans(xml, owners, None)
}

/// Replays `owners` onto `xml`.
///
/// `spans` replaces the whole modeled-owner scan only when the caller proved
/// it is the full candidate list for these exact bytes. `None` is the
/// authoritative scan. An empty owner list returns `xml` and does not turn a
/// missing span list into an empty one. Marker and ambiguity checks stay here.
pub(super) fn replay_nested_namespace_declarations_with_spans(
    xml: &[u8],
    owners: &[NestedNamespaceOwner],
    spans: Option<&[ModeledOwnerSpan]>,
) -> Result<Vec<u8>> {
    replay_nested_namespace_declarations_in(xml, owners, spans, None).map(|(bytes, _)| bytes)
}

fn namespace_replay_copy(
    xml: &[u8],
    template: Option<&mut TemplateMainWriter<'_, '_>>,
) -> Result<Vec<u8>> {
    if let Some(template) = template
        && let Some(source) = template.sources.source.take()
    {
        // Retain one real, reserved output copy. Move the actual prior source
        // into the legacy output instead of cloning an unchecked payload.
        let retained = template.operation.repeat_source(&source)?;
        let bytes = template.operation.finish_source(retained)?;
        template.sources.source = Some(source);
        Ok(bytes)
    } else {
        Ok(xml.to_vec())
    }
}

// The actual event Writer selects copied images while they are borrowed from
// its live input or the just-produced replacement event. Only generated writer
// punctuation owns scratch bytes. No whole replay buffer is materialized here.
enum NamespaceReplayPiece {
    Source(usize, Range<usize>),
    Generated(Vec<u8>),
}

enum NamespaceReplayOutput {
    Bytes(Vec<u8>),
    Recipe {
        input: (usize, usize),
        active: Option<(usize, usize, usize)>,
        reference: Option<Range<usize>>,
        pieces: Vec<NamespaceReplayPiece>,
    },
}

impl NamespaceReplayOutput {
    fn input_range(input: &[u8], bytes: &[u8]) -> Option<Range<usize>> {
        let start = (bytes.as_ptr() as usize).checked_sub(input.as_ptr() as usize)?;
        let end = start.checked_add(bytes.len())?;
        (end <= input.len()).then_some(start..end)
    }

    fn active_event(&mut self, source: Option<(usize, &[u8])>) {
        if let Self::Recipe { active, .. } = self {
            *active = source.map(|(index, bytes)| (index, bytes.as_ptr() as usize, bytes.len()));
        }
    }
}

impl std::io::Write for NamespaceReplayOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Self::Recipe {
            input,
            active,
            reference,
            pieces,
        } = self
        else {
            let Self::Bytes(output) = self else {
                unreachable!()
            };
            output.extend_from_slice(bytes);
            return Ok(bytes.len());
        };
        if bytes.is_empty() {
            return Ok(0);
        }
        // GeneralRef's three Writer writes belong to the actual Reader event,
        // including its grammar-owned delimiters. Preserve that complete
        // encoded payload rather than labelling static delimiters generated.
        let reference_piece = reference.as_mut().map(|range| {
            let start = range.start;
            range.start += bytes.len();
            (0, start..range.start)
        });
        let selected = reference_piece.or_else(|| {
            active
                .iter()
                .copied()
                .chain(std::iter::once((0, input.0, input.1)))
                .find_map(|(index, base, length)| {
                    let start = (bytes.as_ptr() as usize).checked_sub(base)?;
                    let end = start.checked_add(bytes.len())?;
                    (end <= length).then_some((index, start..end))
                })
        });
        if let Some((index, range)) = selected {
            if let Some(NamespaceReplayPiece::Source(previous, span)) = pieces.last_mut()
                && *previous == index
                && span.end == range.start
            {
                span.end = range.end;
            } else {
                pieces.push(NamespaceReplayPiece::Source(index, range));
            }
        } else if let Some(NamespaceReplayPiece::Generated(output)) = pieces.last_mut() {
            output.extend_from_slice(bytes);
        } else {
            pieces.push(NamespaceReplayPiece::Generated(bytes.to_vec()));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn replay_nested_namespace_declarations_in(
    xml: &[u8],
    owners: &[NestedNamespaceOwner],
    spans: Option<&[ModeledOwnerSpan]>,
    mut template: Option<&mut TemplateMainWriter<'_, '_>>,
) -> Result<(Vec<u8>, bool)> {
    if owners.is_empty() {
        return namespace_replay_copy(xml, template).map(|bytes| (bytes, false));
    }
    let scanned;
    let spans = match spans {
        Some(spans) => spans,
        None => {
            scanned = modeled_owner_spans(xml)?;
            &scanned
        }
    };
    let mut targets: Vec<(usize, &NestedNamespaceOwner)> = Vec::new();
    let mut output_has_retained_namespace_owner = false;
    for owner in owners {
        let mut candidates = Vec::new();
        for span in spans
            .iter()
            .filter(|span| span.local_name == owner.local_name)
        {
            let snapshot = logical_owner_snapshot(
                &xml[span.start..span.end],
                &span.bindings,
                &owner.declarations,
            )?;
            if snapshot.structure != owner.snapshot.structure {
                continue;
            }
            let markers = candidate_namespace_owner_markers(
                &xml[span.start..span.end],
                &owner.declarations,
                &span.bindings,
                &owner.markers,
            )?;
            if marker_multiset_matches(&owner.markers, &markers, false) {
                candidates.push((span, snapshot, markers));
            }
        }
        let namespace_candidates = candidates
            .iter()
            .filter(|(_, _, markers)| marker_multiset_matches(&owner.markers, markers, true))
            .collect::<Vec<_>>();
        let target = match namespace_candidates.as_slice() {
            [candidate] if !owner.same_namespace_structural_alternate => candidate.0,
            [candidate]
                if !owner.ambiguous_without_namespace
                    && candidate.1.semantic == owner.snapshot.semantic =>
            {
                candidate.0
            }
            [] if !owner.ambiguous_without_namespace => {
                let semantic_candidates = candidates
                    .iter()
                    .filter(|(_, snapshot, _)| snapshot.semantic == owner.snapshot.semantic)
                    .collect::<Vec<_>>();
                match semantic_candidates.as_slice() {
                    [candidate] => candidate.0,
                    [] if candidates.len() == 1 && !owner.same_marker_structural_alternate => {
                        candidates[0].0
                    }
                    _ => {
                        return Err(Error::Other(format!(
                            "cannot identify retained `{}` nested namespace owner after mutation",
                            owner.local_name
                        )));
                    }
                }
            }
            candidates => {
                let semantic_candidates = candidates
                    .iter()
                    .filter(|candidate| candidate.1.semantic == owner.snapshot.semantic)
                    .collect::<Vec<_>>();
                if let ([candidate], false) = (
                    semantic_candidates.as_slice(),
                    owner.ambiguous_without_namespace && owner.same_namespace_structural_alternate,
                ) {
                    candidate.0
                } else if let Some(peer) = spans
                    .iter()
                    .filter(|span| span.local_name == owner.local_name)
                    .nth(owner.ordinal)
                    .filter(|_| {
                        spans
                            .iter()
                            .filter(|span| span.local_name == owner.local_name)
                            .count()
                            == owner.peers
                    })
                    && let Some(candidate) = semantic_candidates
                        .iter()
                        .find(|candidate| candidate.0.start == peer.start)
                    && owner.declarations.iter().all(|(name, namespace)| {
                        inherited_namespace(&peer.bindings, &namespace_prefix(name))
                            == Some(namespace.as_str())
                    })
                {
                    // Owners written again alike, only their declarations
                    // told them apart, and those declarations say what the
                    // scope around the element says already (an edited
                    // paragraph's own `w` in a part that names Word
                    // otherwise): where they go changes no name, and an
                    // edit keeps the elements of the part in their order,
                    // so they go back to the element at the owner's place
                    // among its peers. Declarations that bind a name
                    // otherwise tell the owners apart: never guessed.
                    candidate.0
                } else {
                    return Err(Error::Other(format!(
                        "cannot identify retained `{}` nested namespace owner after mutation",
                        owner.local_name
                    )));
                }
            }
        };
        // This is the selected live serialized span from the candidate scan
        // above. Its marker facts include the prospective owner declarations,
        // while nested shadows still take precedence. The fact is published
        // only if the full replay/copy succeeds, so it proves a nonempty output
        // owner, never a complete empty output inventory.
        let selected = candidates.iter().find(|(span, _, _)| {
            span.start == target.start
                && span.end == target.end
                && span.local_name == target.local_name
        });
        output_has_retained_namespace_owner |= selected.is_some_and(|(_, _, markers)| {
            owner.declarations.iter().any(|(name, value)| {
                !value.is_empty()
                    && markers.iter().any(|marker| {
                        marker.namespace_facts.iter().any(|(prefix, namespace)| {
                            *prefix == namespace_prefix(name)
                                && namespace.as_deref() == Some(value.as_str())
                        })
                    })
            })
        });
        if owner.declarations.iter().all(|declaration| {
            target
                .declarations
                .iter()
                .any(|candidate| candidate == declaration)
        }) {
            continue;
        }
        targets.push((target.start, owner));
    }
    if targets.is_empty() {
        return namespace_replay_copy(xml, template)
            .map(|bytes| (bytes, output_has_retained_namespace_owner));
    }

    let source = template
        .as_mut()
        .and_then(|template| template.sources.source.take());
    let input = source.as_ref().map_or(xml, |source| source.as_bytes());
    let output = if source.is_some() {
        NamespaceReplayOutput::Recipe {
            input: (input.as_ptr() as usize, input.len()),
            active: None,
            reference: None,
            pieces: Vec::new(),
        }
    } else {
        NamespaceReplayOutput::Bytes(Vec::new())
    };
    let mut retained_events = Vec::new();
    let mut reader = NsReader::from_reader(input);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(output);
    let mut replayed = 0usize;
    loop {
        let start = reader.buffer_position() as usize;
        let (_namespace, event) = reader.read_resolved_event().map_err(|error| {
            Error::Other(format!("invalid serialized namespace scope: {error}"))
        })?;
        let starts_scope = matches!(&event, Event::Start(_));
        match event {
            Event::Start(mut element) | Event::Empty(mut element) => {
                if let Some((_, owner)) = targets.iter().find(|(target, _)| *target == start) {
                    let existing = namespace_declarations(&element)?;
                    let mut missing = Vec::new();
                    for (name, value) in &owner.declarations {
                        match existing.iter().find(|(candidate, _)| candidate == name) {
                            Some((_, existing_value)) if existing_value == value => {}
                            Some(_) => {
                                return Err(Error::Other(format!(
                                    "cannot replay conflicting `{}` namespace after mutation",
                                    namespace_prefix(name),
                                )));
                            }
                            None => missing.push((name, value)),
                        }
                    }
                    replayed += 1;
                    if let Some(source) = &source {
                        let range = NamespaceReplayOutput::input_range(input, element.as_ref())
                            .ok_or_else(|| {
                                Error::Other(
                                    "namespace replay event has no live source image".into(),
                                )
                            })?;
                        // Only the new attributes are generated. The actual
                        // quick_xml attribute producer retains its escaping.
                        let mut attributes = BytesStart::new("x");
                        for (name, value) in missing {
                            attributes.push_attribute((name.as_str(), value.as_str()));
                        }
                        let attribute_bytes: &[u8] = attributes.as_ref();
                        let emitted = template
                            .as_mut()
                            .expect("scoped replay writer")
                            .operation
                            .compose_sources(&[
                                (Some(source), range, &[]),
                                (None, 0..0, &attribute_bytes[1..]),
                            ])?;
                        let name_length = element.name().as_ref().len();
                        let record = BytesStart::from_content(
                            std::str::from_utf8(emitted.as_bytes()).map_err(|error| {
                                Error::Other(format!("invalid serialized namespace scope: {error}"))
                            })?,
                            name_length,
                        );
                        writer
                            .get_mut()
                            .active_event(Some((retained_events.len() + 1, emitted.as_bytes())));
                        writer.write_event(if starts_scope {
                            Event::Start(record)
                        } else {
                            Event::Empty(record)
                        })?;
                        writer.get_mut().active_event(None);
                        retained_events.push(emitted);
                        continue;
                    }
                    for (name, value) in missing {
                        element.push_attribute((name.as_str(), value.as_str()));
                    }
                }
                writer.write_event(if starts_scope {
                    Event::Start(element)
                } else {
                    Event::Empty(element)
                })?;
            }
            Event::End(element) => writer.write_event(Event::End(element))?,
            Event::GeneralRef(reference) => {
                if let NamespaceReplayOutput::Recipe {
                    reference: range, ..
                } = writer.get_mut()
                {
                    let payload = NamespaceReplayOutput::input_range(input, reference.as_ref())
                        .expect("borrowed reference from actual replay Reader");
                    *range = Some(payload.start - 1..payload.end + 1);
                }
                writer.write_event(Event::GeneralRef(reference))?;
                if let NamespaceReplayOutput::Recipe { reference, .. } = writer.get_mut() {
                    *reference = None;
                }
            }
            Event::Eof => {
                if replayed < targets.len() {
                    return Err(Error::Other(
                        "cannot align retained nested namespace declarations after mutation"
                            .to_owned(),
                    ));
                }
                return match writer.into_inner() {
                    NamespaceReplayOutput::Bytes(bytes) => {
                        Ok((bytes, output_has_retained_namespace_owner))
                    }
                    NamespaceReplayOutput::Recipe { pieces, .. } => {
                        let source = source.as_ref().expect("actual scoped replay input");
                        let chunks = pieces
                            .iter()
                            .map(|piece| match piece {
                                NamespaceReplayPiece::Source(0, range) => {
                                    (Some(source.as_ref()), range.clone(), &[][..])
                                }
                                NamespaceReplayPiece::Source(index, range) => {
                                    (Some(&retained_events[index - 1]), range.clone(), &[][..])
                                }
                                NamespaceReplayPiece::Generated(bytes) => {
                                    (None, 0..0, bytes.as_slice())
                                }
                            })
                            .collect::<Vec<_>>();
                        let template = template.as_mut().expect("scoped replay writer");
                        let emitted = template.operation.compose_sources(&chunks)?;
                        let legacy = template.operation.repeat_source(&emitted)?;
                        let bytes = template.operation.finish_source(legacy)?;
                        template.sources.source = Some(Arc::new(emitted));
                        Ok((bytes, output_has_retained_namespace_owner))
                    }
                };
            }
            event => writer.write_event(event)?,
        }
    }
}
