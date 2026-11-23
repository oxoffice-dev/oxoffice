//! The admission entry points: XML, sources, readers, field inputs and the classified admission.

use super::*;

/// Admit each independent input before recursive construction. Internal parser
/// routes consume only bytes derived from that admitted input, without ambient
/// thread state or a second whole-source scan at each nested owner.
pub(crate) fn admit_word_xml(
    xml: &[u8],
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
) -> Result<(usize, usize)> {
    Ok(admit_word_source(xml, prefixes, consumed_owner, None)?.work)
}

/// Admit one original operation and retain its constructor occurrence facts.
/// Prefixes are the actual inherited scope. Complete readers that learn their
/// scope from root declarations pass an empty slice, not a conventional alias.
pub(crate) fn admit_word_source(
    xml: &[u8],
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
) -> Result<AdmittedWordSource> {
    Ok(admit_word_input(
        WordXmlInput::Contiguous(xml),
        prefixes,
        consumed_owner,
        root_owner,
    )?)
}

/// Admit one concrete reader through qualification, capture and weighted
/// admission. Part dispatch keeps persistent wrapper bindings. The standalone
/// paragraph role also collects inert success-only selector occurrences.
pub(crate) fn admit_reader_source(
    xml: &[u8],
    reader: WordSourceReader,
) -> Result<AdmittedWordSource> {
    let mut pool = CaptureDeclarations::default();
    let (constructors, capture) = plan_capture_with_context(
        WordXmlInput::Contiguous(xml),
        &[],
        &[],
        None,
        None,
        &mut pool,
        Some(reader),
    )?;
    capture.admit(xml.len(), &pool)?;
    Ok(admit_word_input_classified(
        WordXmlInput::Contiguous(xml),
        &[],
        None,
        None,
        std::sync::Arc::new(constructors),
    )?)
}

/// Raw editing preserves ordinary input refusal, but must surface a resource
/// failure before constructing or publishing any part of its candidate.
pub(crate) fn admit_raw_word_source(
    xml: &[u8],
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
) -> Result<Option<AdmittedWordSource>> {
    match admit_word_input(WordXmlInput::Contiguous(xml), prefixes, None, root_owner) {
        Ok(admitted) => Ok(Some(admitted)),
        Err(WordSourceFailure::Input(_)) => Ok(None),
        Err(WordSourceFailure::Resource(error)) => Err(error),
    }
}

/// Preserve the actual source admission verdict across legacy parser adapters.
/// Ordinary input failures never mark this operation's resource receipt.
pub(crate) fn admit_word_field_input(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    admit_word_input(xml, prefixes, None, None)
        .map(|admitted| admitted.work)
        .map_err(|failure| record_word_source_failure(failure, registry))
}

/// The actual revision factory's root/header role. Its complete existing
/// capture and weighted admission use the same constructor facts before model
/// construction, including ordinary raw fallback. No XML wrapper is injected.
pub(crate) fn admit_revision_field_input(
    xml: &[u8],
    prefixes: &[String],
    require_author: bool,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    admit_actual_field_reader(
        xml,
        prefixes,
        WordSourceReader::OrderedRevision { require_author },
        registry,
    )
}

/// The same actual paragraph reader is used for ordered and nested-run caches.
/// Its real producer supplies the already-selected root opening and body bounds.
pub(crate) fn admit_paragraph_result_field_input(
    xml: &[u8],
    prefixes: &[String],
    opening_end: usize,
    result: std::ops::Range<usize>,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    if result.start > result.end || result.end > xml.len() || result.start < opening_end {
        // Unknown legacy range/EOF shapes retain their existing admission.
        return admit_word_field_input(WordXmlInput::Contiguous(xml), prefixes, registry);
    }
    admit_actual_field_reader(
        xml,
        prefixes,
        WordSourceReader::SimpleParagraph {
            opening_end,
            result_start: result.start,
            result_end: result.end,
        },
        registry,
    )
}

fn admit_actual_field_reader(
    xml: &[u8],
    prefixes: &[String],
    source_reader: WordSourceReader,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    let admitted = (|| -> WordSourceResult<AdmittedWordSource> {
        let mut pool = CaptureDeclarations::default();
        let inherited = inherited_capture_declarations(prefixes, &mut pool)?;
        let (constructors, capture) = plan_capture_with_context(
            WordXmlInput::Contiguous(xml),
            &inherited,
            prefixes,
            None,
            None,
            &mut pool,
            Some(source_reader),
        )?;
        capture
            .admit(xml.len(), &pool)
            .map_err(WordSourceFailure::Resource)?;
        admit_word_input_classified(
            WordXmlInput::Contiguous(xml),
            prefixes,
            None,
            None,
            std::sync::Arc::new(constructors),
        )
    })();
    admitted
        .map(|admitted| admitted.work)
        .map_err(|failure| record_word_source_failure(failure, registry))
}

pub(super) fn record_word_source_failure(
    failure: WordSourceFailure,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> OxmlError {
    match failure {
        WordSourceFailure::Input(error) => error,
        WordSourceFailure::Resource(error) => {
            if let Some(registry) = registry {
                registry
                    .lock()
                    .expect("field origin registry poisoned")
                    .record_source_resource_failure(&error);
            }
            error
        }
    }
}

pub(super) fn admit_word_input(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
) -> WordSourceResult<AdmittedWordSource> {
    let constructors = admit_original_capture_records(xml, prefixes, root_owner, consumed_owner)?;
    admit_word_input_classified(
        xml,
        prefixes,
        consumed_owner,
        root_owner,
        std::sync::Arc::new(constructors),
    )
}

pub(crate) fn admit_word_input_with_constructors(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
    constructors: ConstructorFailures,
) -> Result<AdmittedWordSource> {
    Ok(admit_word_input_classified(
        xml,
        prefixes,
        consumed_owner,
        root_owner,
        std::sync::Arc::new(constructors),
    )?)
}

fn admission_envelope(prefixes: &[String]) -> WordSourceResult<(Vec<u8>, usize)> {
    // The wrapper supplies only the inherited bindings the actual reader
    // already uses. Declarations in the input still shadow this scope.
    let context_bytes = prefixes
        .iter()
        .try_fold(0usize, |sum, prefix| sum.checked_add(prefix.len()))
        .ok_or_else(|| WordSourceFailure::resource("Word source context byte overflow".into()))?;
    let mut bindings: Vec<(&str, &str)> = prefixes
        .iter()
        .filter_map(|value| split_namespace_binding(value))
        .collect();
    // Context multiplicity belongs to revision identity, not XML attributes
    // on this synthetic envelope. Keep the last identical binding so repeated
    // metadata cannot move an earlier binding past a conflicting later one.
    // Conflicting pairs and the stored input context remain untouched.
    let mut seen = std::collections::HashSet::new();
    bindings.reverse();
    bindings.retain(|binding| seen.insert(*binding));
    bindings.reverse();
    for prefix in prefixes.iter().filter(|prefix| !prefix.starts_with('\0')) {
        if !bindings.iter().any(|(name, _)| *name == prefix.as_str()) {
            bindings.push((prefix.as_str(), W_NS));
        }
    }
    let effective_context_bytes = bindings
        .iter()
        .try_fold(0usize, |sum, (prefix, namespace)| {
            let marker = if *namespace == W_NS { prefix.len() } else { 0 };
            sum.checked_add(prefix.len())
                .and_then(|sum| sum.checked_add(namespace.len()))
                .and_then(|sum| sum.checked_add(2))
                .and_then(|sum| sum.checked_add(marker))
        })
        .ok_or_else(|| {
            WordSourceFailure::resource("Word effective context byte overflow".into())
        })?;
    let duplicate_context_bytes = context_bytes.saturating_sub(effective_context_bytes);
    let mut source = b"<rdocxAdmission".to_vec();
    for (prefix, namespace) in bindings {
        source.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            source.push(b':');
            source.extend_from_slice(prefix.as_bytes());
        }
        source.extend_from_slice(b"=\"");
        source.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
        source.push(b'"');
    }
    source.push(b'>');
    Ok((source, duplicate_context_bytes))
}

pub(super) fn admit_word_input_classified(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
    constructors: std::sync::Arc<ConstructorFailures>,
) -> WordSourceResult<AdmittedWordSource> {
    debug_assert!(consumed_owner.is_none_or(|owner| matches!(owner, b"p" | b"r")));
    let (source, duplicate_context_bytes) = admission_envelope(prefixes)?;
    let source_start = source.len();
    let mut source_chunks = vec![source.as_slice()];
    source_chunks.extend(xml.chunks());
    source_chunks.push(b"</rdocxAdmission>");
    let mut reader = Reader::from_reader(WordXmlInput::pieces(&source_chunks));
    let mut stack: Vec<WordAdmissionFrame> = Vec::new();
    let mut discarded_index = 0usize;
    let mut part_event_index = 0usize;
    let mut controls = 0usize;
    let mut path_cost = 0usize;
    let mut copy_nodes = 0usize;
    let mut copy_bytes = 0usize;
    let mut namespace_depths: std::collections::HashMap<Vec<u8>, WordNamespaceBinding> =
        std::collections::HashMap::new();
    let mut copy_scope_weight: Option<usize> = None;
    let mut buffer = Vec::new();
    loop {
        let fact_start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name();
                let parent_admitted = stack.last().is_none_or(|frame| frame.admitted);
                let parent_owner = stack.last().and_then(|frame| frame.owner);
                let in_control_content = stack
                    .last()
                    .is_some_and(|frame| frame.local.as_ref() == b"sdtContent");
                let part_event = constructors
                    .part_events
                    .get(part_event_index)
                    .filter(|fact| fact_start.checked_sub(source_start) == Some(fact.start));
                if part_event.is_some() {
                    part_event_index += 1;
                }
                let part_wrapper = part_event
                    .is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper));
                let mut namespace_undo = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(OxmlError::from)?;
                    let key = attribute.key.as_ref();
                    let prefix = if key == b"xmlns" {
                        Some(b"".as_slice())
                    } else {
                        key.strip_prefix(b"xmlns:")
                    };
                    if let Some(prefix) = prefix {
                        let prefix = prefix.to_vec();
                        let uri = attribute.value.as_ref();
                        const XML_URI: &[u8] = b"http://www.w3.org/XML/1998/namespace";
                        if prefix == b"xmlns"
                            || prefix == b"xml" && uri != XML_URI
                            || prefix != b"xml" && uri == XML_URI
                            || uri == b"http://www.w3.org/2000/xmlns/"
                        {
                            return Err(WordSourceFailure::Input(OxmlError::InvalidValue(
                                "reserved XML namespace binding".into(),
                            )));
                        }
                        copy_scope_weight = None;
                        let previous = namespace_depths.insert(
                            prefix.clone(),
                            WordNamespaceBinding {
                                uri: uri.to_vec(),
                                depth: if part_wrapper { 0 } else { stack.len() },
                            },
                        );
                        namespace_undo.push((prefix, previous));
                    }
                }
                if part_event.is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper))
                {
                    namespace_undo.clear();
                }
                let name = element.name();
                let name = name.as_ref();
                let prefix = name
                    .iter()
                    .position(|byte| *byte == b':')
                    .map_or(b"".as_slice(), |separator| &name[..separator]);
                let binding = namespace_depths.get(prefix);
                let namespace_uri =
                    binding.map_or(b"".as_slice(), |binding| binding.uri.as_slice());
                let virtual_owner = stack.is_empty() && consumed_owner.is_some();
                let fallback_run = consumed_owner == Some(b"p".as_slice())
                    && binding.is_none()
                    && local.as_ref() == b"r"
                    && stack.last().is_some_and(|parent| {
                        parent.admitted && parent.word && parent.local.as_ref() == b"p"
                    });
                let legacy_run_prefix = if fallback_run {
                    Some(prefix.to_vec())
                } else {
                    stack
                        .last()
                        .and_then(|parent| parent.legacy_run_prefix.clone())
                };
                let legacy_word = binding.is_none() && legacy_run_prefix.as_deref() == Some(prefix);
                let word = virtual_owner
                    || legacy_word
                    || !stack.is_empty() && decoded_namespace_is(namespace_uri, W_NS);
                // Only modeled owner containers admit recursive construction.
                // A qualified-looking descendant inside an opaque extension or
                // foreign rebinding remains opaque, including its control names.
                let owned_control_child = !in_control_content
                    || !word
                    || local.as_ref() == b"sdt"
                    || parent_owner.is_some_and(|owner: SdtOwner| owner.owns(local.as_ref()));
                let qualified_control_owner =
                    !word || local.as_ref() != b"sdt" || parent_owner.is_some() || stack.len() == 1;
                let mut admitted = parent_admitted && owned_control_child && qualified_control_owner
                    && ((stack.is_empty() && local.as_ref() == b"rdocxAdmission")
                        || (word && matches!(local.as_ref(),
                            b"document" | b"body" | b"p" | b"r" | b"tbl" | b"tr" | b"tc"
                            | b"sdt" | b"sdtContent" | b"hdr" | b"ftr" | b"comments" | b"comment"
                            | b"footnotes" | b"footnote" | b"endnotes" | b"endnote"
                            | b"glossaryDocument" | b"docParts" | b"docPart" | b"docPartBody"
                            | b"hyperlink" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                            | b"drawing" | b"pict" | b"object" | b"txbxContent" | b"ruby" | b"rt" | b"rubyBase"))
                        || (decoded_namespace_is(namespace_uri, crate::namespace::MC_NS)
                            && matches!(local.as_ref(), b"AlternateContent" | b"Choice" | b"Fallback"))
                        || (namespace_uri == b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                            // Inline shape text is opaque. Only the anchor
                            // parser constructs a recursive Word text body.
                            && local.as_ref() == b"anchor")
                        || (namespace_uri == b"http://schemas.openxmlformats.org/drawingml/2006/main"
                            && matches!(local.as_ref(), b"graphic" | b"graphicData"))
                        || (namespace_uri == b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
                            && matches!(local.as_ref(), b"wsp" | b"txbx"))
                        || (namespace_uri == b"urn:schemas-microsoft-com:vml"
                            && matches!(local.as_ref(), b"shape" | b"group" | b"textbox" | b"rect" | b"roundrect" | b"oval" | b"line" | b"polyline" | b"curve" | b"arc")));
                if let Some(fact) = part_event {
                    admitted = !matches!(fact.kind, ConstructorKind::Unconstructed);
                } else if word
                    && let Some(parent) = stack.last()
                    && parent.word
                {
                    let parent_name = if parent.paragraph_reader_root {
                        b"p".as_slice()
                    } else {
                        parent.local.as_ref()
                    };
                    admitted &= admitted_word_child(parent_name, local.as_ref())
                        && !(parent.ordered_revision_root && local.as_ref() == b"fldSimple");
                }
                // A qualified drawing owner is projected only from an actual
                // run drawing, or as an independently admitted drawing root.
                if namespace_uri
                    == b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                {
                    admitted &= stack.len() == 1
                        || stack.last().is_some_and(|parent| {
                            parent.word && parent.local.as_ref() == b"drawing"
                        });
                }
                let mc = !stack.is_empty()
                    && decoded_namespace_is(namespace_uri, crate::namespace::MC_NS);
                let mut mc_capture_depth = stack.last().and_then(|frame| frame.mc_capture_depth);
                let mut qualified_drawing_depth = None;
                let vml = !stack.is_empty() && namespace_uri == b"urn:schemas-microsoft-com:vml";
                if mc && let Some(parent) = stack.last() {
                    admitted &= if local.as_ref() == b"AlternateContent" {
                        (stack.len() == 1 || parent.word && parent.local.as_ref() == b"r")
                            && captured_namespace_is(binding, stack.len(), crate::namespace::MC_NS)
                    } else {
                        parent.mc
                            && parent.local.as_ref() == b"AlternateContent"
                            && mc_capture_depth.is_some_and(|depth| {
                                captured_namespace_is(binding, depth, crate::namespace::MC_NS)
                            })
                    };
                    if local.as_ref() == b"AlternateContent" {
                        mc_capture_depth = admitted.then_some(stack.len());
                    }
                }
                if word
                    && let Some(parent) = stack.last()
                    && parent.mc
                {
                    admitted &= match (parent.local.as_ref(), local.as_ref()) {
                        (b"Choice", b"drawing") => mc_capture_depth.is_some_and(|depth| {
                            captured_namespace_is(binding, depth, W_NS) || legacy_word
                        }),
                        (b"Choice" | b"Fallback", b"pict") => true,
                        _ => false,
                    };
                    if admitted && local.as_ref() == b"drawing" {
                        qualified_drawing_depth = Some(stack.len());
                    }
                }
                if vml && let Some(parent) = stack.last() {
                    let shape = matches!(
                        local.as_ref(),
                        b"shape"
                            | b"group"
                            | b"rect"
                            | b"roundrect"
                            | b"oval"
                            | b"line"
                            | b"polyline"
                            | b"curve"
                            | b"arc"
                    );
                    admitted &= parent.word
                        && matches!(parent.local.as_ref(), b"pict" | b"object")
                        && shape
                        || parent.vml && parent.local.as_ref() == b"group" && shape
                        || parent.vml
                            && matches!(
                                parent.local.as_ref(),
                                b"shape"
                                    | b"rect"
                                    | b"roundrect"
                                    | b"oval"
                                    | b"line"
                                    | b"polyline"
                                    | b"curve"
                                    | b"arc"
                            )
                            && local.as_ref() == b"textbox";
                }
                if word && matches!(local.as_ref(), b"ruby" | b"rt" | b"rubyBase") {
                    // The native ruby parser preserves the entire owner as
                    // opaque when one of these attribute-free owners has
                    // any non-namespace attribute.
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(OxmlError::from)?;
                        let key = attribute.key.as_ref();
                        if key != b"xmlns" && !key.starts_with(b"xmlns:") {
                            admitted = false;
                        }
                    }
                }
                let parent_anchor = stack.last().and_then(|frame| frame.anchor_depth);
                if parent_anchor.is_none()
                    && matches!(
                        namespace_uri,
                        b"http://schemas.openxmlformats.org/drawingml/2006/main"
                            | b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
                    )
                {
                    admitted = false;
                }
                let direct_drawing = stack.last().is_some_and(|frame| {
                    frame.word && frame.local.as_ref() == b"drawing" && frame.admitted
                });
                // Direct CT_Drawing entries choose anchor by local name after
                // their qualified Word drawing owner has been established.
                // AlternateContent has its separate qualified owner admission.
                if direct_drawing && local.as_ref() == b"anchor" {
                    admitted = stack.last().unwrap().qualified_drawing_depth.is_none_or(|depth|
                        captured_namespace_is(binding, depth, "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"));
                }
                let mut anchor_depth =
                    (admitted && local.as_ref() == b"anchor").then_some(stack.len());
                let typed_anchor_body = parent_anchor.is_some_and(|depth| {
                    local.as_ref() == b"txbxContent"
                        && (captured_namespace_is(binding, depth, W_NS) || legacy_word)
                });
                if let Some(parent_anchor) = parent_anchor {
                    // The actual anchor reader traverses unknown wrappers and
                    // any graphicData URI. These captured branches are the
                    // exceptions it skips without constructing a Word body.
                    admitted = !matches!(
                        local.as_ref(),
                        b"srcRect"
                            | b"positionH"
                            | b"positionV"
                            | b"spPr"
                            | b"blip"
                            | b"wrapNone"
                            | b"wrapSquare"
                            | b"wrapTight"
                            | b"wrapThrough"
                            | b"wrapTopAndBottom"
                    ) && !(captured_namespace_is(
                        binding,
                        parent_anchor,
                        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
                    ) && local.as_ref() == b"docPr");
                    anchor_depth = (admitted && !typed_anchor_body).then_some(parent_anchor);
                }
                if fact_start.checked_sub(source_start).is_some_and(|start| {
                    while constructors
                        .discarded
                        .get(discarded_index)
                        .is_some_and(|range| range.end <= start)
                    {
                        discarded_index += 1;
                    }
                    constructors
                        .discarded
                        .get(discarded_index)
                        .is_some_and(|range| range.contains(&start))
                        || constructors
                            .raw_subtrees
                            .binary_search_by_key(&start, |fact| fact.span.start)
                            .is_ok()
                }) {
                    admitted = false;
                    anchor_depth = None;
                }
                let control =
                    admitted && anchor_depth.is_none() && word && local.as_ref() == b"sdt";
                let typed_vml_body = admitted
                    && word
                    && local.as_ref() == b"txbxContent"
                    && stack
                        .last()
                        .is_some_and(|frame| frame.local.as_ref() == b"textbox");
                let textbox = matches!(event, Event::Start(_))
                    && admitted
                    && (typed_anchor_body || typed_vml_body);
                // A field gains an ordered owner only when an admitted result
                // revision/control is encountered. Pure old field chains keep
                // their previous admission limit. Stop at a real run/body slot.
                if admitted
                    && anchor_depth.is_none()
                    && word
                    && matches!(
                        local.as_ref(),
                        b"sdt" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                    )
                {
                    for frame in stack.iter_mut().rev() {
                        if !frame.admitted
                            || !frame.word
                            || matches!(frame.local.as_ref(), b"r" | b"p" | b"body" | b"tc")
                        {
                            break;
                        }
                        if frame.local.as_ref() == b"fldSimple" && !frame.ordered_field_charged {
                            path_cost = word_owner_cost(path_cost, 1)
                                .map_err(WordSourceFailure::Resource)?;
                            frame.cost += 1;
                            frame.ordered_field_charged = true;
                        }
                    }
                }
                let cost = usize::from(control) + 16 * usize::from(textbox);
                if control && controls == 256 {
                    return Err(WordSourceFailure::resource(
                        "content-control nesting exceeds inclusive limit 256".into(),
                    ));
                }
                let next_cost =
                    word_owner_cost(path_cost, cost).map_err(WordSourceFailure::Resource)?;
                // Keep effective modeled table depth across controls and
                // captured typed text bodies, whose recursive parser entries
                // otherwise begin with a fresh local table counter.
                let table_depth = stack.last().map_or(0, |frame| frame.table_depth);
                let table_depth =
                    if admitted && anchor_depth.is_none() && word && local.as_ref() == b"tbl" {
                        if table_depth >= crate::table::MAX_RECOGNIZED_TABLE_NESTING {
                            return Err(WordSourceFailure::resource(
                                "recognized model nesting exceeds table limit".into(),
                            ));
                        }
                        table_depth + 1
                    } else {
                        table_depth
                    };
                let copy_depth = stack.last().map_or(0usize, |frame| frame.copy_depth);
                let copy_owner = admitted
                    && word
                    && matches!(
                        local.as_ref(),
                        b"p" | b"r"
                            | b"fldSimple"
                            | b"sdt"
                            | b"ins"
                            | b"del"
                            | b"moveFrom"
                            | b"moveTo"
                            | b"tbl"
                            | b"tr"
                            | b"tc"
                    );
                // A retained opaque child is one physical fact. Its descendants
                // do not become separate modeled-copy owners through this budget.
                let copy_fact =
                    copy_owner || stack.last().is_some_and(|frame| frame.admitted) && !admitted;
                let copy_scope_bytes = if let Some(bytes) = copy_scope_weight {
                    bytes
                } else {
                    let bytes = namespace_depths
                        .iter()
                        .try_fold(0usize, |sum, (prefix, binding)| {
                            let marker = if decoded_namespace_is(&binding.uri, W_NS) {
                                prefix.len()
                            } else {
                                0
                            };
                            sum.checked_add(prefix.len())
                                .and_then(|sum| sum.checked_add(binding.uri.len()))
                                .and_then(|sum| sum.checked_add(2))
                                .and_then(|sum| sum.checked_add(marker))
                        })
                        .ok_or_else(|| {
                            WordSourceFailure::resource("Word local copy context overflow".into())
                        })?;
                    copy_scope_weight = Some(bytes);
                    bytes
                };
                let next_copy_depth =
                    copy_depth
                        .checked_add(usize::from(copy_owner))
                        .ok_or_else(|| {
                            WordSourceFailure::resource(
                                "Word source-copy ownership overflow".into(),
                            )
                        })?;
                if copy_fact && matches!(event, Event::Empty(_)) {
                    let visits = copy_depth.checked_add(1).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy visit overflow".into())
                    })?;
                    copy_nodes = copy_nodes.checked_add(visits).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy visit overflow".into())
                    })?;
                    let bytes = (reader.buffer_position() as usize - fact_start)
                        .checked_add(copy_scope_bytes)
                        .and_then(|bytes| bytes.checked_mul(visits))
                        .ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy byte overflow".into())
                        })?;
                    copy_bytes = copy_bytes.checked_add(bytes).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy byte overflow".into())
                    })?;
                }
                if matches!(event, Event::Start(_)) {
                    let owner = if !admitted || anchor_depth.is_some() {
                        None
                    } else if word {
                        match consumed_owner
                            .filter(|_| virtual_owner)
                            .unwrap_or(local.as_ref())
                        {
                            b"p" | b"r" | b"hyperlink" | b"fldSimple" => Some(SdtOwner::Inline),
                            b"body" | b"hdr" | b"ftr" | b"comment" | b"footnote" | b"endnote"
                            | b"docPartBody" | b"txbxContent" => Some(SdtOwner::Body),
                            b"tbl" => Some(SdtOwner::Table),
                            b"tr" => Some(SdtOwner::Row),
                            b"tc" => Some(SdtOwner::Cell),
                            b"sdt" => Some(
                                parent_owner
                                    .or(root_owner.filter(|_| stack.len() == 1))
                                    .unwrap_or(SdtOwner::Standalone),
                            ),
                            b"sdtContent" => parent_owner,
                            b"ins" | b"del" | b"moveFrom" | b"moveTo" => parent_owner
                                .or_else(|| (stack.len() == 1).then_some(SdtOwner::Inline)),
                            _ => None,
                        }
                    } else if namespace_uri == crate::namespace::MC_NS.as_bytes() {
                        parent_owner
                    } else {
                        None
                    };
                    stack.push(WordAdmissionFrame {
                        fact_start,
                        paragraph_reader_root: fact_start.checked_sub(source_start).is_some_and(
                            |start| {
                                constructors.paragraph_result_root == Some(start)
                                    || constructors.ordered_revision_root == Some(start)
                            },
                        ),
                        ordered_revision_root: fact_start
                            .checked_sub(source_start)
                            .is_some_and(|start| constructors.ordered_revision_root == Some(start)),
                        copy_fact,
                        copy_depth: next_copy_depth,
                        copy_scope_bytes,
                        control,
                        ordered_field_charged: false,
                        admitted,
                        word,
                        legacy_run_prefix: admitted.then_some(legacy_run_prefix).flatten(),
                        mc,
                        vml,
                        owner,
                        anchor_depth,
                        namespace_undo,
                        mc_capture_depth,
                        qualified_drawing_depth,
                        cost,
                        table_depth,
                        local: {
                            let value = consumed_owner
                                .filter(|_| virtual_owner)
                                .unwrap_or(local.as_ref());
                            match value {
                                b"p" => std::borrow::Cow::Borrowed(b"p".as_slice()),
                                b"r" => std::borrow::Cow::Borrowed(b"r".as_slice()),
                                b"rPr" => std::borrow::Cow::Borrowed(b"rPr".as_slice()),
                                b"t" => std::borrow::Cow::Borrowed(b"t".as_slice()),
                                _ => std::borrow::Cow::Owned(value.to_vec()),
                            }
                        },
                    });
                    controls += usize::from(control);
                    path_cost = next_cost;
                } else {
                    restore_namespace_depths(
                        &mut namespace_depths,
                        namespace_undo,
                        &mut copy_scope_weight,
                    );
                }
            }
            Event::End(_) => {
                if let Some(frame) = stack.pop() {
                    if frame.copy_fact {
                        let visits = stack
                            .last()
                            .map_or(0usize, |parent| parent.copy_depth)
                            .checked_add(1)
                            .ok_or_else(|| {
                                WordSourceFailure::resource(
                                    "Word source-copy visit overflow".into(),
                                )
                            })?;
                        copy_nodes = copy_nodes.checked_add(visits).ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy visit overflow".into())
                        })?;
                        let bytes = (reader.buffer_position() as usize - frame.fact_start)
                            .checked_add(frame.copy_scope_bytes)
                            .and_then(|bytes| bytes.checked_mul(visits))
                            .ok_or_else(|| {
                                WordSourceFailure::resource("Word source-copy byte overflow".into())
                            })?;
                        copy_bytes = copy_bytes.checked_add(bytes).ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy byte overflow".into())
                        })?;
                    }
                    controls -= usize::from(frame.control);
                    path_cost -= frame.cost;
                    restore_namespace_depths(
                        &mut namespace_depths,
                        frame.namespace_undo,
                        &mut copy_scope_weight,
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let context_work = copy_nodes
        .checked_mul(duplicate_context_bytes)
        .ok_or_else(|| WordSourceFailure::resource("Word source-copy context overflow".into()))?;
    copy_bytes = copy_bytes
        .checked_add(context_work)
        .ok_or_else(|| WordSourceFailure::resource("Word source-copy context overflow".into()))?;
    // Namespace closure may activate an inherited encoded-only Word alias.
    // Such caller-supplied contexts retain the existing constructor scan until
    // their projected alias actions are represented. Closed source declarations
    // already activate aliases in the original qualifier's scope.
    let projected_aliases = prefixes
        .iter()
        .filter_map(|value| split_namespace_binding(value))
        .all(|(prefix, namespace)| {
            namespace != W_NS || prefixes.iter().any(|alias| alias.as_str() == prefix)
        });
    Ok(AdmittedWordSource {
        work: (copy_nodes, copy_bytes),
        constructors,
        projected_aliases,
        source_map: std::sync::Arc::new(WordSourceMap::original(
            0..xml.len().map_err(WordSourceFailure::Resource)?,
        )),
    })
}

/// Public reader entrypoints begin after the owning start event. Reconstruct
/// exactly that remaining owner, without consuming the caller or its siblings.
pub(crate) fn admit_word_reader(
    input: &Reader<&[u8]>,
    prefixes: &[String],
    owner: &[u8],
    namespace: Option<&str>,
) -> Result<AdmittedWordSource> {
    // The clone retains the actual consumed QName stack. Validate its close
    // without moving the production cursor or inventing an owning binding.
    let mut reader = input.clone();
    reader.config_mut().check_end_names = true;
    let xml = *input.get_ref();
    let offset = input.buffer_position();
    let mut depth = 0usize;
    loop {
        let before = reader.buffer_position();
        match reader.read_event()? {
            Event::Start(_) => depth += 1,
            Event::End(_) if depth > 0 => depth -= 1,
            Event::End(end) => {
                let name = end.name();
                if end.local_name().as_ref() != owner {
                    return Err(OxmlError::InvalidValue(
                        "remaining XML closes a different owner".into(),
                    ));
                }
                if let Some(namespace) = namespace {
                    let qualified = if namespace == W_NS {
                        is_word_element(name.as_ref(), owner, prefixes)
                    } else {
                        let bindings = namespace_bindings(prefixes);
                        name.as_ref()
                            .iter()
                            .position(|byte| *byte == b':')
                            .is_some_and(|at| {
                                bindings.iter().any(|(prefix, uri)| {
                                    prefix.as_bytes() == &name.as_ref()[..at] && uri == namespace
                                })
                            })
                    };
                    if !qualified {
                        return Err(OxmlError::InvalidValue(
                            "remaining XML closes a different qualified owner".into(),
                        ));
                    }
                    let mut source = vec![b'<'];
                    source.extend_from_slice(name.as_ref());
                    source.push(b'>');
                    source.extend_from_slice(&xml[..(reader.buffer_position() - offset) as usize]);
                    return admit_word_source(&source, prefixes, None, None);
                }
                // Legacy CT_P/CT_R callers provide a typed operation but no
                // start event or namespace scope. Admit its actual children
                // under that concrete role, retaining their real bindings.
                return admit_word_source(
                    &xml[..(before - offset) as usize],
                    prefixes,
                    Some(owner),
                    None,
                );
            }
            Event::Eof => {
                return Err(OxmlError::InvalidValue(
                    "remaining XML has no owning close".into(),
                ));
            }
            _ => {}
        }
    }
}
