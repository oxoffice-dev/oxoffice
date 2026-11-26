//! Text boxes: masking them in their hosts, comparing them, projecting paired ones.

use super::*;

pub(super) fn text_box_coordinates(
    xml: &str,
    boxes: &[Range<usize>],
) -> Result<Vec<(usize, usize)>> {
    let mut coordinates = Vec::with_capacity(boxes.len());
    let mut previous_run = None;
    let mut run_ordinal = 0usize;
    let mut box_ordinal = 0usize;
    for text_box in boxes {
        let run = containing_run(xml, text_box.start)?;
        if previous_run.as_ref() != Some(&run) {
            run_ordinal += usize::from(previous_run.is_some());
            box_ordinal = 0;
            previous_run = Some(run);
        }
        coordinates.push((run_ordinal, box_ordinal));
        box_ordinal += 1;
    }
    Ok(coordinates)
}

pub(super) fn text_box_marker_local(
    original: &str,
    edited: &str,
    word_prefix: &str,
) -> Result<String> {
    for ordinal in 0usize.. {
        let local = format!("textBoxHost{ordinal}");
        if !contains_private_text_box_marker(original, word_prefix, &local)?
            && !contains_private_text_box_marker(edited, word_prefix, &local)?
        {
            return Ok(local);
        }
    }
    unreachable!()
}

fn contains_private_text_box_marker(xml: &str, word_prefix: &str, local: &str) -> Result<bool> {
    let open = format!(
        r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:{word_prefix}="{W_NS}">"#
    );
    let wrapped = format!("{open}{xml}</rdocxcmp:root>");
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let private = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == b"urn:rdocx:comparison:private"
        );
        match event {
            Event::Start(element) | Event::Empty(element)
                if private && element.local_name().as_ref() == local.as_bytes() =>
            {
                return Ok(true);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

/// `xml` with each host of a text box (the run child holding it) written as
/// a private marker naming its frame: the host without the content of its
/// boxes. Two markers are alike when their hosts differ only inside their
/// boxes. Also the hosts, in order.
pub(super) fn mask_text_box_subtrees(
    xml: &str,
    word_prefix: &str,
    marker_local: &str,
) -> Result<(String, Vec<String>)> {
    let boxes = text_box_spans(xml, word_prefix)?;
    let spans = text_box_host_spans(xml, &boxes)?;
    let preserved = spans
        .iter()
        .map(|span| xml[span.clone()].to_owned())
        .collect::<Vec<_>>();
    let mut masked = xml.to_owned();
    for (span, host) in spans.into_iter().zip(&preserved).rev() {
        let frame = box_frame(host, &text_box_spans(host, word_prefix)?)?;
        let frame = Sha256::digest(frame.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        masked.replace_range(
            span,
            &format!(
                r#"<rdocxcmp:{marker_local} xmlns:rdocxcmp="urn:rdocx:comparison:private" rdocxcmp:frame="{frame}"/>"#
            ),
        );
    }
    Ok((masked, preserved))
}

/// Which host a marker the masked comparison wrote stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerRole {
    /// Kept: the host both inputs hold, as the comparison resolved it.
    Kept,
    /// Inside a deletion or a move source the comparison wrote: the
    /// original's host.
    Deleted,
    /// Inside an insertion or a move destination it wrote: the edited one's.
    Inserted,
}

/// The hosts that take the place of the markers in `xml`, a masked story the
/// comparison wrote: the `original` host for a marker it deleted (whose
/// boxes [`deleted_text_box_contents`] marks deleted), the `edited` host for
/// one it inserted, and for one it kept the `compared` host (one for each
/// original host, compared with the edited host of the same rank) or, with
/// none, the original host. The markers of each input come in its order.
/// `written` tells the ids of the revisions the comparison wrote from those
/// an input holds.
pub(super) fn restore_text_box_subtrees(
    xml: &mut String,
    [original, edited]: [&[String]; 2],
    compared: Option<&[String]>,
    written: impl Fn(i32) -> bool,
    word_prefix: &str,
    marker_local: &str,
    location: &str,
) -> Result<()> {
    let markers = private_text_box_host_roles(xml, word_prefix, marker_local, written)?;
    let lost = || Error::Other(format!("comparison lost a text-box host at {location}"));
    let (mut left, mut right) = (0usize, 0usize);
    let mut replacements = Vec::with_capacity(markers.len());
    for (span, role) in markers {
        let host = match role {
            MarkerRole::Kept => {
                left += 1;
                right += 1;
                match compared {
                    // A kept host compared box by box pairs the same rank.
                    Some(compared) if left == right => compared.get(left - 1),
                    Some(_) => None,
                    None => original.get(left - 1),
                }
            }
            MarkerRole::Deleted => {
                left += 1;
                original.get(left - 1)
            }
            MarkerRole::Inserted => {
                right += 1;
                edited.get(right - 1)
            }
        };
        replacements.push((span, host.ok_or_else(lost)?));
    }
    if left != original.len() || right != edited.len() {
        return Err(lost());
    }
    for (span, host) in replacements.into_iter().rev() {
        xml.replace_range(span, host);
    }
    Ok(())
}

/// The markers of `xml` in order, each with the role [`MarkerRole`] the
/// revision around it gives it.
fn private_text_box_host_roles(
    xml: &str,
    word_prefix: &str,
    marker_local: &str,
    written: impl Fn(i32) -> bool,
) -> Result<Vec<(Range<usize>, MarkerRole)>> {
    let open = format!(
        r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:{word_prefix}="{W_NS}">"#
    );
    let wrapped = format!("{open}{xml}</rdocxcmp:root>");
    let offset = open.len();
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut markers = Vec::new();
    // The role of each open element, and where it starts if it is a marker.
    let mut open_elements = Vec::<(MarkerRole, Option<usize>)>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let private = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == b"urn:rdocx:comparison:private"
        );
        let after = reader.buffer_position() as usize;
        let role = open_elements.last().map_or(MarkerRole::Kept, |open| open.0);
        let span = |start: usize| start.saturating_sub(offset)..after.saturating_sub(offset);
        match event {
            Event::Start(element) | Event::Empty(element)
                if private && element.local_name().as_ref() == marker_local.as_bytes() =>
            {
                if wrapped[..after].ends_with("/>") {
                    markers.push((span(before), role));
                } else {
                    open_elements.push((role, Some(before)));
                }
            }
            Event::Start(element) => {
                let written_id = element
                    .attributes()
                    .flatten()
                    .find(|attribute| {
                        attribute.key.local_name().as_ref() == b"id"
                            && attribute.key.prefix().is_some()
                    })
                    .and_then(|attribute| std::str::from_utf8(&attribute.value).ok()?.parse().ok())
                    .is_some_and(&written);
                let role = match element.local_name().as_ref() {
                    b"del" | b"moveFrom" if written_id => MarkerRole::Deleted,
                    b"ins" | b"moveTo" if written_id => MarkerRole::Inserted,
                    _ => role,
                };
                open_elements.push((role, None));
            }
            Event::End(_) => {
                if let Some((role, Some(start))) = open_elements.pop() {
                    markers.push((span(start), role));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(markers)
}

pub(super) fn text_box_host_spans(xml: &str, boxes: &[Range<usize>]) -> Result<Vec<Range<usize>>> {
    let mut hosts = Vec::new();
    for text_box in boxes {
        let run = containing_run(xml, text_box.start)?;
        let relative = text_box.start - run.0;
        let children = direct_element_spans(&xml[run.0..run.1])?;
        let child = children
            .into_iter()
            .find(|child| child.start <= relative && relative < child.end)
            .ok_or_else(|| Error::Other("text-box subtree has no run-child host".to_owned()))?;
        let host = run.0 + child.start..run.0 + child.end;
        if hosts.last() != Some(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

/// The runs holding the outermost boxes of `xml`, each without the content
/// of its boxes and its drawing ids (see [`box_frame`]): alike in two
/// versions when the boxes changed only inside.
pub(super) fn text_box_run_skeletons(xml: &str, boxes: &[Range<usize>]) -> Result<Vec<String>> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for text_box in boxes {
        if boxes.iter().any(|outer| {
            outer != text_box && outer.start <= text_box.start && text_box.end <= outer.end
        }) {
            continue;
        }
        let run = containing_run(xml, text_box.start)?;
        if runs.last() != Some(&run) {
            runs.push(run);
        }
    }
    runs.into_iter()
        .map(|(start, end)| {
            let run = &xml[start..end];
            box_frame(run, &text_box_spans(run, "w")?)
        })
        .collect()
}

/// `host` without the content of its `boxes` and without its drawing ids, in
/// a canonical writing: what a box's frame says. A drawing id is no part of
/// it: a box replaced whole keeps its id on both sides, and the redline
/// renumbers one of them.
fn box_frame(host: &str, boxes: &[Range<usize>]) -> Result<String> {
    let skeleton = story_skeleton(host, boxes);
    let skeleton =
        String::from_utf8(without_drawing_ids(skeleton.as_bytes())).map_err(utf8_error)?;
    Ok(oxoffice_core::xml_element::canonical_fragment(&skeleton).unwrap_or(skeleton))
}

pub(super) fn replace_text_boxes_in_hosts(
    xml: &str,
    boxes: &[Range<usize>],
    replacements: &[String],
    word_prefix: &str,
) -> Result<Vec<String>> {
    let mut replacement_index = 0usize;
    let mut hosts = Vec::new();
    for span in text_box_host_spans(xml, boxes)? {
        let mut host = xml[span].to_owned();
        let host_boxes = text_box_spans(&host, word_prefix)?;
        let count = host_boxes.len();
        for (box_span, replacement) in host_boxes
            .into_iter()
            .zip(&replacements[replacement_index..replacement_index + count])
            .rev()
        {
            host.replace_range(box_span, replacement);
        }
        replacement_index += count;
        hosts.push(host);
    }
    if replacement_index != replacements.len() {
        return Err(Error::Other(
            "comparison could not correlate nested text-box owners".to_owned(),
        ));
    }
    Ok(hosts)
}

pub(super) fn private_text_box_host_spans(
    xml: &str,
    word_prefix: &str,
    marker_local: &str,
) -> Result<Vec<Range<usize>>> {
    let open = format!(
        r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:{word_prefix}="{W_NS}">"#
    );
    let wrapped = format!("{open}{xml}</rdocxcmp:root>");
    let offset = open.len();
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut stack = Vec::<Option<usize>>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_marker = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == b"urn:rdocx:comparison:private"
        );
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => stack.push(
                (is_marker && element.local_name().as_ref() == marker_local.as_bytes())
                    .then_some(before),
            ),
            Event::Empty(element)
                if is_marker && element.local_name().as_ref() == marker_local.as_bytes() =>
            {
                spans.push(before.saturating_sub(offset)..after.saturating_sub(offset));
            }
            Event::Empty(_) => {}
            Event::End(_) => {
                if let Some(Some(start)) = stack.pop() {
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

pub(super) fn without_text_box_subtrees(xml: &[u8]) -> Result<Vec<u8>> {
    let source = std::str::from_utf8(xml).map_err(utf8_error)?;
    let boxes = text_box_spans(source, "w")?;
    if boxes.is_empty() {
        return Ok(xml.to_vec());
    }
    let mut output = source.to_owned();
    for span in boxes.into_iter().rev() {
        output.replace_range(span, "");
    }
    Ok(output.into_bytes())
}

pub(super) fn modeled_revision_count_with_options(
    xml: &[u8],
    options: &ComparisonOptions,
) -> Result<usize> {
    if story_ignored(options, ComparisonStoryKind::TextBox) {
        crate::revision::modeled_revision_count(&without_text_box_subtrees(xml)?)
    } else {
        crate::revision::modeled_revision_count(xml)
    }
}

pub(super) fn word_ids_with_options(
    xml: &[u8],
    options: &ComparisonOptions,
) -> Result<HashSet<i32>> {
    if story_ignored(options, ComparisonStoryKind::TextBox) {
        word_ids(&without_text_box_subtrees(xml)?)
    } else {
        word_ids(xml)
    }
}

/// Whether the text boxes of a story changed only inside: the same boxes in
/// the same hosts, one at least holding other content. Such a story is
/// compared box by box, so that an edit is tracked inside its box, in each
/// copy an alternate content holds, rather than the box replaced whole.
pub(super) fn text_boxes_changed_inside(
    original: &str,
    edited: &str,
    word_prefix: &str,
) -> Result<bool> {
    let left = text_box_spans(original, word_prefix)?;
    let right = text_box_spans(edited, word_prefix)?;
    if left.is_empty()
        || left.len() != right.len()
        || text_box_run_skeletons(original, &left)? != text_box_run_skeletons(edited, &right)?
    {
        return Ok(false);
    }
    Ok(left
        .iter()
        .zip(&right)
        .any(|(left, right)| original[left.clone()] != edited[right.clone()]))
}

/// `xml`, a story part the comparison wrote, with the content of each text
/// box inside a deletion it wrote (`written` tells its ids from an input's)
/// marked deleted too: each run of the box, in each copy an alternate content
/// holds and in a box nested in it, in a deletion of its own with its text as
/// deleted text, as the deletion around the box needs. Accepting removes the
/// box with its run, rejecting gives back its text. A run an input already
/// holds deleted inside the box is left as it is.
pub(super) fn deleted_text_box_contents(
    xml: &[u8],
    written: impl Fn(i32) -> bool,
    metadata: &mut Metadata<'_>,
) -> Result<Vec<u8>> {
    #[derive(Clone, Copy, Default)]
    struct Frame {
        /// Inside a deletion this comparison wrote.
        deleted: bool,
        /// Inside the content of a box in such a deletion.
        boxed: bool,
        /// Inside a deletion or a move source an input holds in that box.
        held: bool,
        /// A run of that box, marked deleted.
        marked: bool,
    }
    if !xml
        .windows(b"txbxContent".len())
        .any(|window| window == b"txbxContent")
    {
        return Ok(xml.to_vec());
    }
    let source = std::str::from_utf8(xml).map_err(utf8_error)?;
    let author = oxoffice_core::xml::escape_attribute_value(metadata.author);
    let date = oxoffice_core::xml::escape_attribute_value(metadata.timestamp);
    let unbalanced = || Error::Other("comparison XML is unbalanced".into());
    let mut reader = NsReader::from_str(source);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<Frame>::new();
    // Text written in place of a byte range, in document order.
    let mut edits = Vec::<(Range<usize>, String)>::new();
    // The end tag of each marked run open, and the start and new name of
    // the text open in one.
    let mut closings = Vec::<String>::new();
    let mut text = None::<(usize, String)>;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let empty = matches!(event, Event::Empty(_));
                let parent = stack.last().copied().unwrap_or_default();
                let local = element.local_name();
                let local = local.as_ref();
                let name = std::str::from_utf8(element.name().as_ref())
                    .map_err(utf8_error)?
                    .to_owned();
                let prefix = name.rsplit_once(':').map(|(prefix, _)| prefix.to_owned());
                if word && parent.marked && matches!(local, b"t" | b"instrText") {
                    let target = if local == b"t" {
                        "delText"
                    } else {
                        "delInstrText"
                    };
                    let renamed = prefix
                        .as_ref()
                        .map_or(target.to_owned(), |prefix| format!("{prefix}:{target}"));
                    if empty {
                        let element = &source[before..after];
                        edits.push((
                            before..after,
                            oxoffice_core::xml_element::rename(element, &renamed)?,
                        ));
                    } else {
                        text = Some((before, renamed));
                    }
                }
                let written_deletion = word
                    && local == b"del"
                    && element
                        .attributes()
                        .flatten()
                        .find(|attribute| {
                            attribute.key.local_name().as_ref() == b"id"
                                && attribute.key.prefix().is_some()
                        })
                        .and_then(|attribute| {
                            std::str::from_utf8(&attribute.value).ok()?.parse().ok()
                        })
                        .is_some_and(&written);
                let mut frame = Frame {
                    deleted: parent.deleted || written_deletion,
                    boxed: parent.boxed || (parent.deleted && word && local == b"txbxContent"),
                    held: parent.held
                        || (parent.boxed && word && matches!(local, b"del" | b"moveFrom")),
                    marked: false,
                };
                if word && local == b"r" && frame.boxed && !frame.held {
                    frame.marked = true;
                    let id = metadata.ids.allocate()?;
                    let (opening, closing) = match &prefix {
                        Some(prefix) => (
                            format!(
                                r#"<{prefix}:del {prefix}:id="{id}" {prefix}:author="{author}" {prefix}:date="{date}">"#
                            ),
                            format!("</{prefix}:del>"),
                        ),
                        None => (
                            format!(
                                r#"<w:del xmlns:w="{W_NS}" w:id="{id}" w:author="{author}" w:date="{date}">"#
                            ),
                            "</w:del>".to_owned(),
                        ),
                    };
                    edits.push((before..before, opening));
                    if empty {
                        edits.push((after..after, closing));
                    } else {
                        closings.push(closing);
                    }
                }
                if !empty {
                    stack.push(frame);
                }
            }
            Event::End(ref element) => {
                let frame = stack.pop().ok_or_else(unbalanced)?;
                let parent = stack.last().copied().unwrap_or_default();
                let local = element.local_name();
                if word && frame.marked && local.as_ref() == b"r" {
                    edits.push((after..after, closings.pop().ok_or_else(unbalanced)?));
                } else if word && parent.marked && matches!(local.as_ref(), b"t" | b"instrText") {
                    let (start, renamed) = text.take().ok_or_else(unbalanced)?;
                    edits.push((
                        start..after,
                        oxoffice_core::xml_element::rename(&source[start..after], &renamed)?,
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if edits.is_empty() {
        return Ok(xml.to_vec());
    }
    let mut output = String::with_capacity(source.len() + edits.len() * 96);
    let mut cursor = 0;
    for (range, replacement) in edits {
        output.push_str(&source[cursor..range.start]);
        output.push_str(&replacement);
        cursor = range.end;
    }
    output.push_str(&source[cursor..]);
    Ok(output.into_bytes())
}

pub(super) fn text_box_spans(xml: &str, word_prefix: &str) -> Result<Vec<Range<usize>>> {
    let open = format!(
        r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:{word_prefix}="{W_NS}">"#
    );
    let wrapped = format!("{open}{xml}</rdocxcmp:root>");
    let offset = open.len();
    let mut reader = NsReader::from_reader(wrapped.as_bytes());
    reader.config_mut().trim_text(false);
    let mut spans = Vec::new();
    let mut stack = Vec::<Option<usize>>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_text_box =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                stack.push(
                    (is_text_box && element.local_name().as_ref() == b"txbxContent")
                        .then_some(before),
                );
            }
            Event::Empty(element)
                if is_text_box && element.local_name().as_ref() == b"txbxContent" =>
            {
                spans.push(before.saturating_sub(offset)..after.saturating_sub(offset));
            }
            Event::End(_) => {
                if let Some(Some(start)) = stack.pop() {
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

pub(super) fn comparison_text_box_markers(
    original: &Document,
    edited: &Document,
    stories: &[StoryPart],
    options: &ComparisonOptions,
) -> Result<TextBoxMarkers> {
    if !story_ignored(options, ComparisonStoryKind::TextBox) {
        return Ok(TextBoxMarkers::default());
    }
    let original_main = original
        .package
        .get_part(&original.doc_part_name)
        .ok_or_else(|| Error::Other(format!("missing main story {}", original.doc_part_name)))?;
    let edited_main = edited
        .package
        .get_part(&edited.doc_part_name)
        .ok_or_else(|| Error::Other(format!("missing main story {}", edited.doc_part_name)))?;
    let original_main = std::str::from_utf8(original_main).map_err(utf8_error)?;
    let edited_main = std::str::from_utf8(edited_main).map_err(utf8_error)?;
    let mut markers = TextBoxMarkers {
        main: text_box_marker_local(original_main, edited_main, "w")?,
        related: HashMap::new(),
    };
    for story in stories {
        let original_xml = std::str::from_utf8(story_xml(original, story)?).map_err(utf8_error)?;
        let edited_xml = std::str::from_utf8(story_xml(edited, story)?).map_err(utf8_error)?;
        let prefix = root_prefix(original_xml, story.kind.root_local())?;
        markers.related.insert(
            story.part_name.clone(),
            text_box_marker_local(original_xml, edited_xml, &prefix)?,
        );
    }
    for kind in NoteKind::ALL {
        if story_ignored(options, kind.story_kind()) {
            continue;
        }
        let left = NotePart::of(original, kind)?;
        let right = NotePart::of(edited, kind)?;
        if let Some(part) = left.as_ref().or(right.as_ref()) {
            let prefix = root_prefix(&part.xml, kind.part_root())?;
            let marker = text_box_marker_local(
                left.as_ref().map_or("", |part| part.xml.as_str()),
                right.as_ref().map_or("", |part| part.xml.as_str()),
                &prefix,
            )?;
            for part in left.iter().chain(right.iter()) {
                markers.related.insert(part.name.clone(), marker.clone());
            }
        }
    }
    Ok(markers)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RunHostRole {
    Raw(usize, usize),
    Typed(usize, usize),
}

/// The aligned pair, its role-local selections and actual source frame own this
/// policy. Projection happens before allocating the established outer records.
#[allow(clippy::type_complexity)]
pub(super) fn paired_run_replacement(
    runs: [&CT_R; 2],
    sources: [Option<&RunSource>; 2],
    selections: [Option<RunSourceSelection<'_>>; 2],
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let [original, edited] = runs;
    if run_field(original).is_some() && run_field(edited).is_some() {
        return compared_run_xml(
            original,
            edited,
            sources[0],
            selections[1],
            location,
            destination_parent,
            metadata,
            diagnostics,
        );
    }
    let left_source = sources[0]
        .or_else(|| metadata.run_source(original))
        .cloned();
    let mut right_source = sources[1].or_else(|| metadata.run_source(edited)).cloned();
    let left_complete = complete_run_selection(original);
    let right_complete = complete_run_selection(edited);
    let left_selection = selections[0].unwrap_or((&left_complete.0, &left_complete.1));
    let right_selection = selections[1].unwrap_or((&right_complete.0, &right_complete.1));
    let mut inserted = edited.clone();
    let mut projected = false;
    if metadata.options.ignore_formatting {
        // The raw children keep their place after the properties the
        // inserted copy takes from the original.
        match (inserted.properties.is_some(), original.properties.is_some()) {
            (true, false) => {
                for position in &mut inserted.extra_xml_positions {
                    let shifted = CT_R::raw_child_position(*position).saturating_sub(1);
                    CT_R::set_raw_child_position(position, shifted);
                }
            }
            (false, true) => {
                for position in &mut inserted.extra_xml_positions {
                    let shifted = CT_R::raw_child_position(*position) + 1;
                    CT_R::set_raw_child_position(position, shifted);
                }
            }
            _ => {}
        }
        inserted.properties = original.properties.clone();
        if let (Some(left), Some(right)) = (left_source.as_ref(), right_source.as_mut()) {
            projected = project_paired_textboxes(
                &mut inserted,
                original,
                left,
                right,
                [left_selection, right_selection],
                location,
                metadata,
                diagnostics,
            )?;
        } else if edited
            .extra_xml
            .iter()
            .any(|raw| std::str::from_utf8(raw).is_ok_and(|raw| raw.contains("txbxContent")))
        {
            return Err(Error::Other(format!(
                "comparison has no paired physical textbox source at {location}"
            )));
        }
    }
    // A box changed only inside is compared box by box before this point
    // ([`text_boxes_changed_inside`]). Replaced whole, the deleted copy of a
    // box has its content marked deleted once the story is written
    // ([`deleted_text_box_contents`]).
    let inserted_xml = publish_run_source(
        original,
        &inserted,
        paragraph_owned_run_xml(&inserted)?,
        left_source.as_ref(),
        right_source.as_ref(),
        Some(right_selection),
        destination_parent,
        metadata,
    )?;
    if projected {
        let original_xml = publish_run_source(
            original,
            original,
            paragraph_owned_run_xml(original)?,
            left_source.as_ref(),
            None,
            Some(left_selection),
            destination_parent,
            metadata,
        )?;
        // Both strings are faithful publications from their selected source
        // roles. Geometry, root attributes and opaque payloads participate.
        if inserted_xml == original_xml {
            metadata.projected_textbox_noop = true;
            return Ok(original_xml);
        }
    }
    let revision_parent = metadata.ids.revision_parent_scope(destination_parent);
    let inserted_xml = publish_run_source(
        original,
        &inserted,
        paragraph_owned_run_xml(&inserted)?,
        left_source.as_ref(),
        right_source.as_ref(),
        Some(right_selection),
        &revision_parent,
        metadata,
    )?;
    let deleted = publish_run_source(
        original,
        original,
        deleted_run_xml(original)?,
        left_source.as_ref(),
        None,
        Some(left_selection),
        &revision_parent,
        metadata,
    )?;
    let deleted = metadata
        .ids
        .revision("del", metadata.author, metadata.timestamp, &deleted)?;
    let inserted =
        metadata
            .ids
            .revision("ins", metadata.author, metadata.timestamp, &inserted_xml)?;
    Ok(format!("{deleted}{inserted}"))
}

/// Qualified paths include every physical child ordinal. Only outermost boxes
/// are returned, so nested work descends through the existing story comparator.
#[allow(clippy::type_complexity)]
pub(super) fn policy_textbox_intervals(
    xml: &str,
) -> Result<
    Vec<(
        Range<usize>,
        NamespaceBindings,
        Vec<(String, String, usize)>,
        Option<Range<usize>>,
    )>,
> {
    let opening = format!(r#"<rdocxPolicy:p xmlns:rdocxPolicy="{W_NS}"><rdocxPolicy:r>"#);
    let inner = format!("{opening}{xml}</rdocxPolicy:r></rdocxPolicy:p>");
    let (wrapped, offset) = story_wrapper(&inner, &[])?;
    let offset = offset + opening.len();
    let mut admitted = crate::document::scan_story_owners(&wrapped, StoryKind::Body)?
        .into_iter()
        .filter(|owner| owner.kind == StoryKind::TextBox)
        .map(|owner| owner.full.start.saturating_sub(offset)..owner.full.end.saturating_sub(offset))
        .filter(|span| span.end <= xml.len())
        .collect::<Vec<_>>();
    admitted.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
    let mut outermost: Vec<Range<usize>> = Vec::new();
    for span in admitted {
        if !outermost
            .last()
            .is_some_and(|parent| parent.start <= span.start && span.end <= parent.end)
        {
            outermost.push(span);
        }
    }
    let mut reader = NsReader::from_str(xml);
    let mut path = Vec::new();
    let mut starts = Vec::new();
    let mut children = vec![0usize];
    let mut active = None;
    let mut output = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let local = std::str::from_utf8(element.local_name().as_ref())
                    .map_err(utf8_error)?
                    .to_owned();
                let uri = match namespace {
                    ResolveResult::Bound(Namespace(uri)) => {
                        std::str::from_utf8(uri).map_err(utf8_error)?.to_owned()
                    }
                    ResolveResult::Unbound => String::new(),
                    ResolveResult::Unknown(_) => {
                        return Err(Error::Other(
                            "textbox source has an unresolved QName".into(),
                        ));
                    }
                };
                let ordinal = *children.last().unwrap();
                *children.last_mut().unwrap() += 1;
                path.push((uri.clone(), local.clone(), ordinal));
                starts.push(before);
                if outermost.iter().any(|span| span.start == before) && active.is_none() {
                    let drawing = path
                        .iter()
                        .rposition(|(uri, local, _)| uri == W_NS && local == "drawing")
                        .map(|index| {
                            let start = starts[index];
                            let mut scanner = Reader::from_str(&xml[start..]);
                            let Event::Start(root) = scanner
                                .read_event()
                                .map_err(|error| Error::Other(error.to_string()))?
                            else {
                                return Err(Error::Other(
                                    "admitted drawing ancestor is empty".into(),
                                ));
                            };
                            scanner
                                .read_to_end(root.name())
                                .map_err(|error| Error::Other(error.to_string()))?;
                            Ok(start..start + scanner.buffer_position() as usize)
                        })
                        .transpose()?;
                    let record = (
                        before,
                        resolved_source_bindings(&reader)?,
                        path.clone(),
                        drawing,
                    );
                    if empty {
                        output.push((
                            before..reader.buffer_position() as usize,
                            record.1,
                            record.2,
                            record.3,
                        ));
                    } else {
                        active = Some(record);
                    }
                }
                if empty {
                    path.pop();
                    starts.pop();
                } else {
                    children.push(0);
                }
            }
            Event::End(_) => {
                if active
                    .as_ref()
                    .is_some_and(|(_, _, owner_path, _)| owner_path.len() == path.len())
                {
                    let (start, bindings, owner_path, drawing) = active.take().unwrap();
                    output.push((
                        start..reader.buffer_position() as usize,
                        bindings,
                        owner_path,
                        drawing,
                    ));
                }
                path.pop();
                starts.pop();
                children.pop();
            }
            Event::Eof => return Ok(output),
            _ => {}
        }
    }
}

/// Actual role/slot provenance is retained while only admitted inner intervals
/// change. No source occurrence is discovered by searching its payload bytes.
fn project_paired_textboxes(
    inserted: &mut CT_R,
    original: &CT_R,
    left: &RunSource,
    right: &mut RunSource,
    selections: [RunSourceSelection<'_>; 2],
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<bool> {
    let mut hosts = [Vec::new(), Vec::new()];
    for side in 0..2 {
        let source = if side == 0 { left } else { &*right };
        let model = if side == 0 { original } else { &*inserted };
        for (selected_index, &(local, ordinal, boundary)) in selections[side].0.iter().enumerate() {
            let (span, actual_boundary, _) = source
                .raw_slots
                .get(ordinal)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    Error::Other(format!("textbox source slot is absent at {location}"))
                })?;
            if boundary != *actual_boundary {
                return Err(Error::Other(format!(
                    "textbox source boundary changed at {location}"
                )));
            }
            let mut event_reader = Reader::from_str(&source.scoped[span.clone()]);
            if !matches!(
                event_reader.read_event(),
                Ok(Event::Start(_) | Event::Empty(_))
            ) {
                continue;
            }
            if CT_R::raw_child_has_alternate_content_drawing(model.extra_xml_positions[local]) {
                let index = model.extra_xml_positions[..local]
                    .iter()
                    .filter(|encoded| CT_R::raw_child_has_alternate_content_drawing(**encoded))
                    .count();
                if !model
                    .alt_drawings
                    .get(index)
                    .and_then(|drawing| drawing.anchor.as_ref())
                    .and_then(|anchor| anchor.shape.as_ref())
                    .is_some_and(|shape| shape.text_body.is_some())
                {
                    continue;
                }
            }
            let closed = source_child(&source.scoped, span.clone())?;
            let boxes = policy_textbox_intervals(&closed)?;
            if !boxes.is_empty() {
                hosts[side].push((
                    RunHostRole::Raw(local, ordinal),
                    selected_index,
                    span.clone(),
                    closed,
                    boxes,
                ));
            }
        }
    }
    for side in 0..2 {
        let source = if side == 0 { left } else { &*right };
        let model = if side == 0 { original } else { &*inserted };
        for (local, ordinal) in selections[side].1.iter().enumerate() {
            if !matches!(model.content.get(local), Some(RunContent::Drawing(drawing)) if drawing.anchor.as_ref().and_then(|anchor| anchor.shape.as_ref()).is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty()))
            {
                continue;
            }
            let Some(selected) = ordinal else {
                continue;
            };
            if selected.scalars.is_some() {
                return Err(Error::Other(format!(
                    "drawing source has a text subrange at {location}"
                )));
            }
            let ordinal = selected.ordinal;
            let Some(span) = source.typed_slots.get(ordinal).cloned().flatten() else {
                continue;
            };
            let closed = source_child(&source.scoped, span.clone())?;
            let boxes = policy_textbox_intervals(&closed)?;
            if !boxes.is_empty() {
                hosts[side].push((
                    RunHostRole::Typed(local, ordinal),
                    local,
                    span,
                    closed,
                    boxes,
                ));
            }
        }
        hosts[side].sort_by_key(|host| host.2.start);
    }
    for (side, side_hosts) in hosts.iter().enumerate() {
        let model = if side == 0 { original } else { &*inserted };
        for (local, encoded) in model.extra_xml_positions.iter().enumerate() {
            if CT_R::raw_child_has_alternate_content_drawing(*encoded) {
                let index = model.extra_xml_positions[..local]
                    .iter()
                    .filter(|encoded| CT_R::raw_child_has_alternate_content_drawing(**encoded))
                    .count();
                if model
                    .alt_drawings
                    .get(index)
                    .and_then(|drawing| drawing.anchor.as_ref())
                    .and_then(|anchor| anchor.shape.as_ref())
                    .is_some_and(|shape| shape.text_body.is_some())
                    && !side_hosts
                        .iter()
                        .any(|host| matches!(host.0, RunHostRole::Raw(slot, _) if slot == local))
                {
                    return Err(Error::Other(format!(
                        "comparison cannot establish selected compatibility body source at {location}"
                    )));
                }
            }
        }
        for (local, content) in model.content.iter().enumerate() {
            if matches!(content, RunContent::Drawing(drawing) if drawing.anchor.as_ref().and_then(|anchor| anchor.shape.as_ref()).is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty()))
                && !side_hosts
                    .iter()
                    .any(|host| matches!(host.0, RunHostRole::Typed(index, _) if index == local))
            {
                return Err(Error::Other(format!(
                    "comparison cannot establish a physical typed textbox source at {location}"
                )));
            }
        }
    }
    if hosts.iter().all(Vec::is_empty) {
        return Ok(false);
    }
    // Boxes added or removed, or a box whose frame changed, are replaced
    // whole with their run, as when formatting is compared (#666).
    let frame = |host: &(RunHostRole, usize, Range<usize>, String, Vec<_>)| {
        let boxes = host
            .4
            .iter()
            .map(|text_box: &(Range<usize>, _, _, _)| text_box.0.clone());
        box_frame(&host.3, &boxes.collect::<Vec<_>>())
    };
    if hosts[0].len() != hosts[1].len() {
        return Ok(false);
    }
    for (left, right) in hosts[0].iter().zip(&hosts[1]) {
        if std::mem::discriminant(&left.0) != std::mem::discriminant(&right.0)
            || left.1 != right.1
            || left.4.len() != right.4.len()
            || frame(left)? != frame(right)?
        {
            return Ok(false);
        }
    }
    let mut replacements = Vec::new();
    for (left_host, right_host) in hosts[0].iter().zip(&hosts[1]) {
        if std::mem::discriminant(&left_host.0) != std::mem::discriminant(&right_host.0)
            || left_host.1 != right_host.1
            || left_host.4.len() != right_host.4.len()
        {
            return Err(Error::Other(format!(
                "comparison has ambiguous textbox source ownership at {location}"
            )));
        }
        let mut derived = right_host.3.clone();
        let mut inner_replacements = Vec::new();
        for (left_box, right_box) in left_host.4.iter().zip(&right_host.4) {
            if left_box.2 != right_box.2 {
                return Err(Error::Other(format!(
                    "comparison changed a qualified textbox owner path at {location}"
                )));
            }
            let left_xml = &left_host.3[left_box.0.clone()];
            let right_xml = &right_host.3[right_box.0.clone()];
            let left_inner = element_inner_range_any_prefix(left_xml, "txbxContent")?;
            let right_inner = element_inner_range_any_prefix(right_xml, "txbxContent")?;
            let used = metadata.ids.used.clone();
            let local_binding = metadata.ids.local_word_binding;
            // The qualified host already supplies the actual ancestor scope.
            // Closing a retained p here would turn extraction into authored bytes.
            let left_content = left_xml[left_inner.clone()].to_owned();
            let right_content = right_xml[right_inner].to_owned();
            let left_model = story_document(&left_content, &left_box.1)?;
            let right_model = story_document(&right_content, &right_box.1)?;
            let mut left_sources = SourceSnapshots::default();
            let mut right_sources = SourceSnapshots::default();
            left_sources.body(
                &left_model,
                (
                    &left_content,
                    &story_content_spans(&left_content, &left_box.1)?,
                    &left_box.1,
                ),
                &HeldRevisions::default(),
            )?;
            right_sources.body(
                &right_model,
                (
                    &right_content,
                    &story_content_spans(&right_content, &right_box.1)?,
                    &right_box.1,
                ),
                &HeldRevisions::default(),
            )?;
            if (!left_sources.shared_root_facts.is_empty()
                || !right_sources.shared_root_facts.is_empty())
                && left_content != right_content
            {
                return Err(Error::Other(format!(
                    "comparison cannot prove significant shared field run-root correspondence at {location}/textbox-slot[{}]",
                    right_host.1
                )));
            }
            let facts = |model: &CT_Document,
                         source: &SourceSnapshots|
             -> Result<Vec<BTreeMap<String, Vec<Vec<String>>>>> {
                let mut facts = model
                    .body
                    .content
                    .iter()
                    .map(|content| {
                        Ok(SourceMetadata::body(content, source, metadata.options)?
                            .facts
                            .into_iter()
                            .map(|(kind, records)| {
                                (
                                    kind,
                                    records.into_iter().map(|(_, _, facts)| facts).collect(),
                                )
                            })
                            .collect())
                    })
                    .collect::<Result<Vec<BTreeMap<String, Vec<Vec<String>>>>>>()?;
                facts.retain(|facts| !facts.is_empty());
                Ok(facts)
            };
            if facts(&left_model, &left_sources)? != facts(&right_model, &right_sources)? {
                return Err(Error::Other(format!(
                    "comparison cannot revise significant source metadata at {location}/textbox-slot[{}]",
                    right_host.1
                )));
            }
            let outer_leaves = std::mem::take(&mut metadata.leaf_publications);
            let outer_capture = std::mem::replace(&mut metadata.leaf_capture, true);
            let paired_location = format!("{location}/textbox-slot[{}]", right_host.1);
            let outer_field_story = metadata.paired_field_story.replace(paired_location.clone());
            let tracked_result = compare_story_inner_impl(
                &left_content,
                &right_content,
                &paired_location,
                "w",
                &left_box.1,
                &right_box.1,
                metadata,
                diagnostics,
                // Nested paired hosts recurse through this same source-qualified
                // publisher. Do not rescan them with the legacy prefix wrapper.
                false,
            );
            let mut leaf_publications =
                std::mem::replace(&mut metadata.leaf_publications, outer_leaves);
            metadata.leaf_capture = outer_capture;
            metadata.paired_field_story = outer_field_story;
            let tracked = tracked_result?;
            let temporary = metadata
                .ids
                .used
                .difference(&used)
                .copied()
                .collect::<HashSet<_>>();
            let (wrapped, wrapper_offset) = story_wrapper(&tracked, &left_box.1)?;
            shift_leaf_publications(&mut leaf_publications, wrapper_offset as isize)?;
            leaf_publications.sort_by_key(|leaf| leaf.range.start);
            let leaf_ranges = leaf_publications
                .iter()
                .map(|leaf| leaf.range.clone())
                .collect::<Vec<_>>();
            let (input, _) = story_wrapper(&left_xml[left_inner.clone()], &left_box.1)?;
            let input_held = crate::revision::modeled_revision_spans(&input)?
                .into_iter()
                .map(|(range, id)| (id, input[range].to_vec()))
                .collect::<Vec<_>>();
            let held = crate::revision::modeled_revision_spans(&wrapped)?
                .into_iter()
                .filter(|(_, id)| !temporary.contains(id))
                .map(|(range, id)| (id, wrapped[range].to_vec()))
                .collect::<Vec<_>>();
            let (resolved, _, mapped) = crate::revision::resolve_story_xml_with_leaf_ranges(
                &wrapped,
                crate::revision::Resolution::Accept,
                crate::revision::RevisionScope::Ids(&temporary),
                &leaf_ranges,
            )?;
            let resolved =
                restore_resolved_leaf_occurrences(&resolved, &leaf_publications, &mapped)?
                    .into_bytes();
            let remaining = crate::revision::modeled_revision_spans(&resolved)?;
            if held != input_held
                || remaining.iter().any(|(_, id)| temporary.contains(id))
                || held
                    != remaining
                        .into_iter()
                        .map(|(range, id)| (id, resolved[range].to_vec()))
                        .collect::<Vec<_>>()
                || word_ids(&resolved)?.iter().any(|id| temporary.contains(id))
            {
                return Err(Error::Other(format!(
                    "textbox projection changed held revision source at {location}"
                )));
            }
            let mut projected = extract_body_inner(&resolved)?.to_owned();
            if temporary.is_empty() {
                // The unchanged owners still come from the original frame.
                // Remove only closure introduced on their extraction roots.
                let original_inner = &left_xml[left_inner];
                let original_spans = story_content_spans(original_inner, &left_box.1)?;
                let projected_spans = story_content_spans(&projected, &left_box.1)?;
                if original_spans.len() == projected_spans.len() {
                    for (original_span, projected_span) in
                        original_spans.iter().zip(&projected_spans).rev()
                    {
                        let restored = retain_source_root_namespaces(
                            &projected[projected_span.clone()],
                            &original_inner[original_span.clone()],
                            &left_box.1,
                        )?;
                        projected.replace_range(projected_span.clone(), &restored);
                    }
                }
            }
            metadata.ids.used = used;
            metadata.ids.local_word_binding = local_binding;
            // The body was authored in the original owner context. Transport
            // only actual direct roots when the edited context differs.
            let same_context = left_box
                .1
                .iter()
                .map(|(key, value)| (key, value))
                .collect::<BTreeMap<_, _>>()
                == right_box
                    .1
                    .iter()
                    .map(|(key, value)| (key, value))
                    .collect::<BTreeMap<_, _>>();
            if !same_context {
                let spans = story_content_spans(&projected, &left_box.1)?;
                for span in spans.into_iter().rev() {
                    let closed = source_fragment(&projected[span.clone()], &left_box.1)?;
                    projected.replace_range(span, &closed);
                }
            }
            inner_replacements.push((
                right_box.0.clone(),
                replace_owner_inner(right_xml, "txbxContent", &projected)?.0,
            ));
        }
        for (span, projected) in inner_replacements.into_iter().rev() {
            derived.replace_range(span, &projected);
        }
        // Closure belongs to extraction, not to the retained edited host root.
        let actual = &right.scoped[right_host.2.clone()];
        let context = root_namespace_bindings(&right.scoped, "r", &right.parent)?;
        let derived = restore_extracted_root(&derived, actual, &context)?;
        match right_host.0 {
            RunHostRole::Raw(local, _) => {
                inserted.extra_xml[local] = derived.as_bytes().to_vec();
                if CT_R::raw_child_has_alternate_content_drawing(
                    inserted.extra_xml_positions[local],
                ) {
                    let index = inserted.extra_xml_positions[..local]
                        .iter()
                        .filter(|encoded| CT_R::raw_child_has_alternate_content_drawing(**encoded))
                        .count();
                    let closed = source_fragment(&derived, &context)?;
                    let drawing = oxoffice_docx_oxml::drawing::parse_alternate_content(
                        closed.as_bytes(),
                        &[],
                    )
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "projected compatibility drawing cannot be parsed at {location}"
                        ))
                    })?;
                    let target = inserted.alt_drawings.get_mut(index).ok_or_else(|| {
                        Error::Other("selected drawing source slot is absent".into())
                    })?;
                    let mut drawing = drawing;
                    reconcile_projected_drawing(&mut drawing, target, &context, location)?;
                    *target = drawing;
                }
            }
            RunHostRole::Typed(local, _) => {
                let mut drawing = match source_content_model(&derived, &context)? {
                    Some(RunContent::Drawing(drawing)) => drawing,
                    _ => {
                        return Err(Error::Other(
                            "projected source is not one typed drawing".into(),
                        ));
                    }
                };
                let Some(RunContent::Drawing(authored)) = inserted.content.get(local) else {
                    return Err(Error::Other(
                        "projected drawing lost its source model".into(),
                    ));
                };
                reconcile_projected_drawing(&mut drawing, authored, &context, location)?;
                if !drawing
                    .anchor
                    .as_ref()
                    .and_then(|anchor| anchor.shape.as_ref())
                    .is_some_and(|shape| shape.text_body.is_some())
                {
                    return Err(Error::Other(format!(
                        "projected drawing has no supported body at {location}"
                    )));
                }
                inserted.content[local] = RunContent::Drawing(drawing);
            }
        }
        replacements.push((right_host.2.clone(), derived, right_host.0));
    }
    // Every original slot keeps its ordinal/boundary. Only its physical range
    // moves when an admitted inner interval changes length.
    replacements.sort_by_key(|(span, _, _)| span.start);
    for slot in right.raw_slots.iter_mut().flatten() {
        slot.0 = projected_source_span(slot.0.clone(), &replacements)?;
    }
    for span in right.typed_slots.iter_mut().flatten() {
        *span = projected_source_span(span.clone(), &replacements)?;
    }
    for (span, payload, ordinal) in replacements.into_iter().rev() {
        right.scoped.replace_range(span, &payload);
        match ordinal {
            RunHostRole::Raw(_, source) => {
                right.projected_raw_slots.insert(source);
            }
            RunHostRole::Typed(_, source) => {
                right.projected_typed_slots.insert(source);
            }
        }
    }
    right.raw = retain_source_root_namespaces(&right.scoped, &right.raw, &right.parent)?;
    Ok(true)
}

/// The source pair owns non-text drawing meaning. Only its admitted inner
/// body changes. Extra inherited bindings introduced by the private parse
/// envelope are not new source dependencies of that non-text owner.
fn reconcile_projected_drawing(
    projected: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
    authored: &oxoffice_docx_oxml::drawing::CT_Drawing,
    context: &[(String, String)],
    location: &str,
) -> Result<()> {
    let mut expected = drawing_signature(authored);
    let mut actual = drawing_signature(projected);
    if let (Some(actual), Some(expected)) = (&mut actual.anchor, &mut expected.anchor) {
        for (prefix, uri) in &actual.source_namespace_bindings {
            if !expected
                .source_namespace_bindings
                .contains(&(prefix.clone(), uri.clone()))
                && !context.iter().any(|(key, value)| {
                    key == &if prefix.is_empty() {
                        "xmlns".into()
                    } else {
                        format!("xmlns:{prefix}")
                    } && value == uri
                })
            {
                return Err(Error::Other(format!(
                    "projected drawing has an unproved namespace dependency at {location}"
                )));
            }
        }
        actual.source_namespace_bindings = expected.source_namespace_bindings.clone();
        for anchor in [actual, expected] {
            if let Some(shape) = &mut anchor.shape {
                shape.text.clear();
                shape.text_body = None;
                shape.source_text_box_owner = None;
            }
        }
    }
    if actual != expected {
        return Err(Error::Other(format!(
            "projected drawing changed non-text source at {location}"
        )));
    }
    if let (Some(projected), Some(authored)) = (&mut projected.anchor, &authored.anchor) {
        projected.source_namespace_bindings = authored.source_namespace_bindings.clone();
    }
    Ok(())
}

fn projected_source_span(
    span: Range<usize>,
    replacements: &[(Range<usize>, String, RunHostRole)],
) -> Result<Range<usize>> {
    let mut start = span.start as i64;
    let mut end = span.end as i64;
    for (changed, value, _) in replacements {
        let delta = value.len() as i64 - changed.len() as i64;
        if changed.end <= span.start {
            start += delta;
            end += delta;
        } else if *changed == span {
            end += delta;
        } else if changed.start < span.end && span.start < changed.end {
            return Err(Error::Other(
                "textbox projection overlaps another physical run role".into(),
            ));
        }
    }
    Ok(start as usize..end as usize)
}
