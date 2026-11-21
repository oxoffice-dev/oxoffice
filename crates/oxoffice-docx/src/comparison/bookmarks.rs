//! Body bookmarks: reconciling and rewriting them after a comparison.

use super::*;

/// A bookmark start of a main story.
struct BodyBookmark {
    id: Option<i32>,
    name: Option<String>,
}

/// Word and [`Document::rebuild_toc`] name the bookmarks a table of contents
/// links to `_Toc` followed by digits, and rename them when they rebuild it.
fn generated_heading_bookmark(name: &str) -> bool {
    name.strip_prefix("_Toc").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// Word renumbers bookmark ids on every save, and a rebuilt table of contents
/// renames the `_Toc` bookmarks of its headings. Neither changes what the
/// document says, and no tracked change can record either. Before comparing,
/// the edited main story takes the original's id for each bookmark name both
/// inputs hold, and the original's name for each `_Toc` bookmark of a body
/// paragraph that the comparison pairs with one of the original holding as
/// many. Its links and field instructions that name a renamed bookmark
/// follow, so it reads the same. A new heading's `_Toc` bookmark takes a
/// fresh name where the original uses its own, and a bookmark only the
/// edited input holds a fresh id. When a reference cannot follow, because a
/// field splits the name across runs or another story names it, or when the
/// comparison would pair a renamed heading otherwise, the names stay as they
/// are and only the ids follow, but a body heading's `_Toc` name, or one a
/// paired heading claims, does not take the original's id for it unless a
/// pairing kept it, since the original may give that name to another
/// heading: it keeps its own id, or a fresh one where the original uses
/// that. Links to another file are left alone.
pub(super) fn reconcile_body_bookmarks(
    original: &Document,
    edited: &mut Document,
    options: &ComparisonOptions,
) -> Result<()> {
    fn part_xml<'a>(document: &'a Document, name: &str) -> Result<&'a [u8]> {
        document
            .package
            .get_part(name)
            .ok_or_else(|| Error::Other(format!("missing comparison story {name}")))
    }
    let left_xml = part_xml(original, &original.doc_part_name)?;
    let right_xml = part_xml(edited, &edited.doc_part_name)?;
    let left = body_bookmarks(left_xml)?;
    let right = body_bookmarks(right_xml)?;
    if right.is_empty() {
        return Ok(());
    }
    let mut parts = Vec::new();
    for document in [original, &*edited] {
        parts.push((document, document.doc_part_name.clone()));
        for story in story_parts(document)? {
            if story.part_name != document.doc_part_name {
                parts.push((document, story.part_name));
            }
        }
    }

    // The generated bookmarks of each body paragraph, in order. The blocks
    // of the body pair as the comparison pairs them, by signature and then
    // by kind in each gap, and a paired heading's bookmarks pair in order
    // when both hold as many.
    let generated = |document: &Document| {
        document
            .document
            .body
            .content
            .iter()
            .map(|content| match content {
                BodyContent::Paragraph(paragraph) => paragraph
                    .bookmark_markers
                    .iter()
                    .filter(|marker| marker.is_start())
                    .filter_map(|marker| marker.name())
                    .filter(|name| generated_heading_bookmark(name))
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            })
            .collect::<Vec<Vec<String>>>()
    };
    // Signatures as the comparison reads them once the identities agree:
    // without the generated bookmarks, and without the ids of the others,
    // which the rewrite aligns.
    let signatures = |document: &Document| {
        document
            .document
            .body
            .content
            .iter()
            .map(|content| {
                let BodyContent::Paragraph(paragraph) = content else {
                    return body_signature_with_options(content, options);
                };
                if paragraph.bookmark_markers.is_empty() {
                    return body_signature_with_options(content, options);
                }
                let generated = paragraph
                    .bookmark_markers
                    .iter()
                    .filter(|marker| {
                        marker.is_start() && marker.name().is_some_and(generated_heading_bookmark)
                    })
                    .filter_map(|marker| marker.id())
                    .collect::<HashSet<_>>();
                let mut paragraph = paragraph.clone();
                paragraph
                    .bookmark_markers
                    .retain(|marker| !marker.id().is_some_and(|id| generated.contains(&id)));
                super::signatures::retain_raw_children(&mut paragraph, |raw| {
                    match raw_bookmark(raw) {
                        Some((_, Some(id), _)) if generated.contains(&id) => false,
                        Some((start, _, name)) => {
                            *raw = format!("bookmark:{start}:{name:?}").into_bytes();
                            true
                        }
                        None => true,
                    }
                });
                body_signature_with_options(&BodyContent::Paragraph(paragraph), options)
            })
            .collect::<Vec<_>>()
    };
    // Plain signatures, as the comparison reads the rewritten input.
    let plain_signatures = |document: &Document| {
        document
            .document
            .body
            .content
            .iter()
            .map(|content| body_signature_with_options(content, options))
            .collect::<Vec<_>>()
    };
    let left_generated = generated(original);
    let right_generated = generated(edited);
    let seen = right_generated
        .iter()
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    let left_names = left
        .iter()
        .filter_map(|bookmark| bookmark.name.as_deref())
        .collect::<HashSet<_>>();
    let mut names = HashMap::new();
    let mut unpaired = HashSet::new();
    let mut renamed_pairs = Vec::new();
    // Generated names a pairing keeps as they are, among those of the body
    // paragraphs it saw.
    let mut kept = HashSet::new();
    if right_generated.iter().any(|names| !names.is_empty()) {
        let (left_signatures, right_signatures) = (signatures(original), signatures(edited));
        let mut paired = vec![false; right_generated.len()];
        for pair in pair_gaps_by_kind(
            align(&left_signatures, &right_signatures),
            &left_signatures,
            &right_signatures,
        ) {
            if let (Some(left), Some(right)) = pair
                && !right_generated[right].is_empty()
                && right_generated[right].len() == left_generated[left].len()
            {
                paired[right] = true;
                if right_generated[right] == left_generated[left] {
                    kept.extend(right_generated[right].iter().cloned());
                } else {
                    renamed_pairs.push((left, right));
                }
                names.extend(
                    right_generated[right]
                        .iter()
                        .cloned()
                        .zip(left_generated[left].iter().cloned()),
                );
            }
        }
        unpaired.extend(
            right_generated
                .iter()
                .zip(paired)
                .filter(|(_, paired)| !paired)
                .flat_map(|(names, _)| names.iter().cloned()),
        );
    }
    // A name the original gives the heading another one pairs with, or a
    // body heading this one does not pair with, would meet that bookmark in
    // the redline: such a heading takes a name neither input uses. A nested
    // heading the comparison may still pair keeps its name otherwise.
    let claimed = names.values().cloned().collect::<HashSet<_>>();
    let mut renamed = right
        .iter()
        .filter_map(|bookmark| bookmark.name.as_deref())
        .filter(|name| generated_heading_bookmark(name) && !names.contains_key(*name))
        .filter(|name| {
            claimed.contains(*name) || (unpaired.contains(*name) && left_names.contains(*name))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    renamed.sort();
    // A name of the original on a heading it does not pair would meet that
    // bookmark in the redline: such a heading takes a name neither input uses.
    let mut taken = None;
    for name in &renamed {
        {
            let (taken, suffix) = match &mut taken {
                Some(taken) => taken,
                slot => {
                    let mut tokens = HashSet::new();
                    for (document, name) in &parts {
                        for token in name_tokens(part_xml(document, name)?) {
                            tokens.insert(token.to_owned());
                        }
                    }
                    let suffix = tokens
                        .iter()
                        .filter_map(|token| token.strip_prefix("_Toc")?.parse::<u64>().ok())
                        .max()
                        .unwrap_or(0);
                    slot.insert((tokens, suffix))
                }
            };
            let fresh = loop {
                *suffix = suffix.checked_add(1).ok_or_else(|| {
                    Error::Other("comparison bookmark names are exhausted".into())
                })?;
                let candidate = format!("_Toc{suffix}");
                if !taken.contains(&candidate) {
                    break candidate;
                }
            };
            names.insert(name.clone(), fresh);
        }
    }
    names.retain(|from, to| from != to);

    // The original's id for a name both hold, and a fresh id where a
    // bookmark only the edited input holds uses an id of the original.
    let mut left_ids = HashMap::new();
    let mut ambiguous = HashSet::new();
    for bookmark in &left {
        if let (Some(name), Some(id)) = (bookmark.name.as_deref(), bookmark.id)
            && left_ids.insert(name, id).is_some()
        {
            ambiguous.insert(name);
        }
    }
    for name in ambiguous {
        left_ids.remove(name);
    }
    // An id the original gives a bookmark of another story is taken too.
    let mut left_id_set = left
        .iter()
        .filter_map(|bookmark| bookmark.id)
        .collect::<HashSet<_>>();
    for (document, name) in &parts {
        if std::ptr::eq(*document, original) && *name != original.doc_part_name {
            left_id_set.extend(bookmark_ids(part_xml(document, name)?)?);
        }
    }
    // The edited ids under these names, or None when two bookmarks would
    // share a name or an id.
    let id_map =
        |names: &HashMap<String, String>, fallback: bool| -> Result<Option<HashMap<i32, i32>>> {
            let final_name = |bookmark: &BodyBookmark| {
                bookmark
                    .name
                    .as_ref()
                    .map(|name| names.get(name).unwrap_or(name).clone())
            };
            let mut final_names = HashSet::new();
            if right
                .iter()
                .filter_map(final_name)
                .any(|name| !final_names.insert(name))
            {
                return Ok(None);
            }
            let mut allocator = None;
            let mut ids = HashMap::new();
            let mut targets = HashSet::new();
            for bookmark in &right {
                let Some(id) = bookmark.id else {
                    continue;
                };
                let name = final_name(bookmark);
                // Footnote custody markers are reconciled by their own owner.
                let custody = name
                    .as_deref()
                    .is_some_and(|name| name.starts_with("_rdocxCmp"));
                // Without the renames, a body heading's name, or a name a
                // paired heading claims, may name another heading of the
                // original unless a pairing kept it: it keeps its own id, or
                // a fresh one where the original uses that.
                let unkept = fallback
                    && name.as_deref().is_some_and(|name| {
                        (seen.contains(name) || claimed.contains(name)) && !kept.contains(name)
                    });
                let target = match name
                    .as_deref()
                    .filter(|_| !unkept)
                    .and_then(|name| left_ids.get(name))
                {
                    _ if custody => id,
                    Some(&target) => target,
                    None if left_id_set.contains(&id) => {
                        let allocator = match &mut allocator {
                            Some(allocator) => allocator,
                            slot => {
                                let mut occupied = HashSet::new();
                                for (document, name) in &parts {
                                    occupied.extend(word_ids(part_xml(document, name)?)?);
                                }
                                slot.insert(IdAllocator::new(occupied))
                            }
                        };
                        allocator.allocate()?
                    }
                    None => id,
                };
                if ids.insert(id, target).is_some() || !targets.insert(target) {
                    return Ok(None);
                }
            }
            ids.retain(|from, to| from != to);
            Ok(Some(ids))
        };
    let unnamed = HashMap::new();
    // The rewritten main story, when references can follow every rename.
    let rewritten = |names: &HashMap<String, String>, fallback: bool| -> Result<Option<Vec<u8>>> {
        let Some(ids) = id_map(names, fallback)? else {
            return Ok(None);
        };
        if names.is_empty() && ids.is_empty() {
            return Ok(None);
        }
        for (document, name) in &parts {
            if std::ptr::eq(*document, &*edited)
                && *name != edited.doc_part_name
                && name_tokens(part_xml(document, name)?).any(|token| names.contains_key(token))
            {
                return Ok(None);
            }
        }
        rewrite_body_bookmarks(right_xml, names, &ids)
    };
    let part = edited.doc_part_name.clone();
    let staged = |xml: Vec<u8>| -> Result<Document> {
        let mut candidate = edited.clone_for_staging();
        crate::document::set_story_source_xml(&mut candidate, &part, xml)?;
        Ok(candidate)
    };
    let candidate = match rewritten(&names, false)? {
        Some(xml) if renamed_pairs.is_empty() => staged(xml)?,
        Some(xml) => {
            let candidate = staged(xml)?;
            // The comparison pairs the rewritten blocks again: a renamed
            // heading it pairs with another one would hold that heading's
            // bookmark beside the original's, so the names stay as they are
            // unless every pair holds.
            let (left, right) = (plain_signatures(original), plain_signatures(&candidate));
            let pairs = pair_gaps_by_kind(align(&left, &right), &left, &right)
                .into_iter()
                .filter_map(|pair| Some((pair.0?, pair.1?)))
                .collect::<HashSet<_>>();
            if renamed_pairs.iter().all(|pair| pairs.contains(pair)) {
                candidate
            } else {
                // Ids are matched by name and need no rename: they still
                // follow the original's.
                let Some(xml) = rewritten(&unnamed, true)? else {
                    return Ok(());
                };
                staged(xml)?
            }
        }
        None if names.is_empty() => return Ok(()),
        None => {
            let Some(xml) = rewritten(&unnamed, true)? else {
                return Ok(());
            };
            staged(xml)?
        }
    };
    *edited = candidate;
    Ok(())
}

/// The main story, the header, footer and comment stories and the footnotes
/// part of a document, each once.
fn bookmark_parts(document: &Document) -> Result<Vec<String>> {
    let mut parts = vec![document.doc_part_name.clone()];
    parts.extend(
        story_parts(document)?
            .into_iter()
            .map(|story| story.part_name),
    );
    parts.extend(FootnotePart::name_of(document)?);
    let mut seen = HashSet::new();
    parts.retain(|part| seen.insert(part.clone()));
    Ok(parts)
}

/// The ids of the bookmarks both inputs hold under one name, in any story.
///
/// [`reconcile_body_bookmarks`] gave the edited main story the original's id
/// for each name both hold, so one id names one bookmark on both sides. No
/// tracked change records a bookmark, so the emitters write such a bookmark
/// once, where the edited input has it. An id that names two bookmarks in
/// one input, and the note custody markers, are left out.
pub(super) fn shared_bookmark_ids(original: &Document, edited: &Document) -> Result<HashSet<i32>> {
    let names = |document: &Document| -> Result<HashMap<i32, String>> {
        let mut names = HashMap::new();
        let mut ambiguous = HashSet::new();
        for part in bookmark_parts(document)? {
            let Some(xml) = document.package.get_part(&part) else {
                continue;
            };
            for bookmark in body_bookmarks(xml)? {
                let (Some(id), Some(name)) = (bookmark.id, bookmark.name) else {
                    continue;
                };
                if name.starts_with("_rdocxCmp") || names.insert(id, name).is_some() {
                    ambiguous.insert(id);
                }
            }
        }
        names.retain(|id, _| !ambiguous.contains(id));
        Ok(names)
    };
    let edited = names(edited)?;
    Ok(names(original)?
        .into_iter()
        .filter(|(id, name)| edited.get(id) == Some(name))
        .map(|(id, _)| id)
        .collect())
}

/// `document` without the bookmarks of these ids, in every story the
/// comparison checks, as the postconditions read it.
pub(super) fn without_bookmarks(document: &Document, ids: &HashSet<i32>) -> Result<Document> {
    let mut copy = document.clone_for_staging();
    for part in bookmark_parts(document)? {
        let Some(xml) = document.package.get_part(&part) else {
            continue;
        };
        let ranges = bookmark_marker_ranges(xml, ids)?;
        if ranges.is_empty() {
            continue;
        }
        let mut xml = xml.to_vec();
        for (range, _) in ranges.into_iter().rev() {
            xml.drain(range);
        }
        copy.package.set_part(&part, xml);
    }
    Ok(copy)
}

/// Whether `document` holds each of these bookmarks once, in the stories the
/// comparison checks: one start, under the name `original` gives it, and one
/// end. Rejecting a paragraph inserted whole leaves its untracked bookmarks
/// beside the restored text, where the original may not have them.
pub(super) fn holds_bookmarks_once(
    document: &Document,
    original: &Document,
    ids: &HashSet<i32>,
) -> Result<bool> {
    // How many markers each document holds by id, and by name for a start.
    let markers = |document: &Document| -> Result<HashMap<(i32, Option<String>), usize>> {
        let mut counts = HashMap::new();
        for part in bookmark_parts(document)? {
            let Some(xml) = document.package.get_part(&part) else {
                continue;
            };
            let tree = crate::revision::XmlTree::parse(xml)?;
            for (index, node) in tree.elements.iter().enumerate() {
                let start = match node.local.as_str() {
                    "bookmarkStart" => true,
                    "bookmarkEnd" => false,
                    _ => continue,
                };
                if !node.word || in_fallback(&tree, index) {
                    continue;
                }
                let attributes = word_attributes(&tree, xml, index)?;
                let Some(id) = attributes
                    .get("id")
                    .and_then(|id| id.parse::<i32>().ok())
                    .filter(|id| ids.contains(id))
                else {
                    continue;
                };
                let name = start.then(|| attributes.get("name").cloned().unwrap_or_default());
                *counts.entry((id, name)).or_default() += 1;
            }
        }
        Ok(counts)
    };
    let held = markers(document)?;
    let wanted = markers(original)?;
    Ok(ids.iter().all(|&id| {
        let name = wanted
            .keys()
            .find_map(|(key, name)| (*key == id).then(|| name.clone()).flatten());
        name.is_some_and(|name| held.get(&(id, Some(name))) == Some(&1))
            && held.get(&(id, None)) == Some(&1)
            && held.keys().filter(|(key, _)| *key == id).count() == 2
    }))
}

/// Each `w:bookmarkStart` and `w:bookmarkEnd` of these ids in a part, as
/// its byte range and whether it starts, in document order.
pub(super) fn bookmark_marker_ranges(
    xml: &[u8],
    ids: &HashSet<i32>,
) -> Result<Vec<(Range<usize>, bool)>> {
    let tree = crate::revision::XmlTree::parse(xml)?;
    let mut ranges = Vec::new();
    for (index, node) in tree.elements.iter().enumerate() {
        let start = match node.local.as_str() {
            "bookmarkStart" => true,
            "bookmarkEnd" => false,
            _ => continue,
        };
        if node.word
            && word_attributes(&tree, xml, index)?
                .get("id")
                .and_then(|id| id.parse::<i32>().ok())
                .is_some_and(|id| ids.contains(&id))
        {
            ranges.push((node.start..node.end, start));
        }
    }
    Ok(ranges)
}

/// The bookmark starts of a main story, in document order. Copies in an
/// `mc:Fallback` repeat their `mc:Choice` and are left out.
fn body_bookmarks(xml: &[u8]) -> Result<Vec<BodyBookmark>> {
    let tree = crate::revision::XmlTree::parse(xml)?;
    let mut bookmarks = Vec::new();
    for (index, node) in tree.elements.iter().enumerate() {
        if !node.word || node.local != "bookmarkStart" || in_fallback(&tree, index) {
            continue;
        }
        let attributes = word_attributes(&tree, xml, index)?;
        bookmarks.push(BodyBookmark {
            id: attributes.get("id").and_then(|id| id.parse().ok()),
            name: attributes.get("name").cloned(),
        });
    }
    Ok(bookmarks)
}

/// A raw bookmark start or end the paragraph model keeps verbatim: whether
/// it starts, its id and its name.
fn raw_bookmark(raw: &[u8]) -> Option<(bool, Option<i32>, Option<String>)> {
    let mut reader = Reader::from_reader(raw);
    let (Ok(Event::Start(element)) | Ok(Event::Empty(element))) = reader.read_event() else {
        return None;
    };
    let start = match element.local_name().as_ref() {
        b"bookmarkStart" => true,
        b"bookmarkEnd" => false,
        _ => return None,
    };
    let mut id = None;
    let mut name = None;
    for attribute in element.attributes().flatten() {
        let value = std::str::from_utf8(&attribute.value).ok();
        match attribute.key.local_name().as_ref() {
            b"id" => id = value.and_then(|value| value.parse().ok()),
            b"name" => name = value.map(str::to_owned),
            _ => {}
        }
    }
    Some((start, id, name))
}

/// The ids of the bookmark starts of a story.
fn bookmark_ids(xml: &[u8]) -> Result<HashSet<i32>> {
    let tree = crate::revision::XmlTree::parse(xml)?;
    let mut ids = HashSet::new();
    for (index, node) in tree.elements.iter().enumerate() {
        if node.word && node.local == "bookmarkStart" {
            ids.extend(
                word_attributes(&tree, xml, index)?
                    .get("id")
                    .and_then(|id| id.parse::<i32>().ok()),
            );
        }
    }
    Ok(ids)
}

fn in_fallback(tree: &crate::revision::XmlTree<'_>, index: usize) -> bool {
    let mut current = tree.elements[index].parent;
    while let Some(parent) = current {
        let node = &tree.elements[parent];
        if !node.word && node.local == "Fallback" {
            return true;
        }
        current = node.parent;
    }
    false
}

/// The Word attributes of an element, raw values by local name.
pub(super) fn word_attributes(
    tree: &crate::revision::XmlTree<'_>,
    xml: &[u8],
    index: usize,
) -> Result<BTreeMap<String, String>> {
    let node = &tree.elements[index];
    let scope = tree
        .source_scope(index)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut reader = Reader::from_reader(&xml[node.start..node.open_end]);
    let (Event::Start(start) | Event::Empty(start)) = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    else {
        unreachable!("element opening");
    };
    let mut attributes = BTreeMap::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let key = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        if let Some((prefix, local)) = key.split_once(':')
            && scope.get(prefix).is_some_and(|uri| uri == W_NS)
        {
            let value = std::str::from_utf8(&attribute.value).map_err(utf8_error)?;
            attributes.insert(local.to_owned(), value.to_owned());
        }
    }
    Ok(attributes)
}

/// The runs of letters, digits and underscores in `text`, the characters a
/// bookmark name is made of, as they appear in field instructions and anchors.
fn name_token_ranges(text: &[u8]) -> impl Iterator<Item = Range<usize>> + '_ {
    let name = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    let mut at = 0;
    std::iter::from_fn(move || {
        let start = at + text[at..].iter().position(name)?;
        let end = start
            + text[start..]
                .iter()
                .position(|byte| !name(byte))
                .unwrap_or(text.len() - start);
        at = end;
        Some(start..end)
    })
}

fn name_tokens(text: &[u8]) -> impl Iterator<Item = &str> {
    name_token_ranges(text).map(|range| std::str::from_utf8(&text[range]).expect("ASCII token"))
}

/// The main story with these bookmark names and ids, with its links and field
/// instructions naming a renamed bookmark. None when a field instruction
/// splits a renamed name across runs.
fn rewrite_body_bookmarks(
    xml: &[u8],
    names: &HashMap<String, String>,
    ids: &HashMap<i32, i32>,
) -> Result<Option<Vec<u8>>> {
    let tree = crate::revision::XmlTree::parse(xml)?;
    let renamed = |value: &[u8]| -> Option<Vec<u8>> {
        let mut output = Vec::new();
        let mut at = 0;
        for range in name_token_ranges(value) {
            let token = std::str::from_utf8(&value[range.clone()]).expect("ASCII token");
            if let Some(name) = names.get(token) {
                output.extend_from_slice(&value[at..range.start]);
                output.extend_from_slice(name.as_bytes());
                at = range.end;
            }
        }
        (at > 0).then(|| {
            output.extend_from_slice(&value[at..]);
            output
        })
    };
    let renumbered = |value: &[u8]| {
        let id = std::str::from_utf8(value).ok()?.parse::<i32>().ok()?;
        ids.get(&id).map(|id| id.to_string().into_bytes())
    };
    let mut edits = Vec::new();
    // The instruction elements of each open complex field, while its
    // instruction is read: a renamed name must sit in one of them.
    let mut fields: Vec<(Vec<usize>, bool)> = Vec::new();
    let instruction = |elements: Vec<usize>, edits: &mut Vec<(Range<usize>, Vec<u8>)>| {
        let mut text = Vec::new();
        let mut segments = Vec::new();
        for index in elements {
            let node = &tree.elements[index];
            if node.empty {
                continue;
            }
            segments.push((text.len(), node.open_end));
            text.extend_from_slice(&xml[node.open_end..node.close_start]);
        }
        if links_another_file(&text) {
            return true;
        }
        for range in name_token_ranges(&text) {
            let token = std::str::from_utf8(&text[range.clone()]).expect("ASCII token");
            let Some(name) = names.get(token) else {
                continue;
            };
            let at = segments.partition_point(|(start, _)| *start <= range.start) - 1;
            let segment_end = segments.get(at + 1).map_or(text.len(), |(start, _)| *start);
            if range.end > segment_end {
                return false;
            }
            let (start, source) = segments[at];
            let from = source + range.start - start;
            edits.push((from..from + range.len(), name.clone().into_bytes()));
        }
        true
    };
    for (index, node) in tree.elements.iter().enumerate() {
        if !node.word {
            continue;
        }
        match node.local.as_str() {
            "fldChar" => {
                match word_attributes(&tree, xml, index)?
                    .get("fldCharType")
                    .map(String::as_str)
                {
                    Some("begin") => fields.push((Vec::new(), true)),
                    Some("separate") => {
                        if let Some(field) = fields.last_mut() {
                            field.1 = false;
                        }
                    }
                    Some("end") => {
                        if let Some((elements, _)) = fields.pop()
                            && !instruction(elements, &mut edits)
                        {
                            return Ok(None);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            "instrText" | "delInstrText" => {
                match fields.last_mut() {
                    Some((elements, true)) => elements.push(index),
                    _ => {
                        if !instruction(vec![index], &mut edits) {
                            return Ok(None);
                        }
                    }
                }
                continue;
            }
            "bookmarkStart" | "bookmarkEnd" | "hyperlink" | "fldSimple" => {}
            _ => continue,
        }
        // A link to another part names a bookmark of that target.
        let external_link = node.local == "hyperlink" && relationship_link(&tree, xml, index)?;
        let updated = rewritten_opening(&tree, xml, index, |local, value| {
            match (node.local.as_str(), local) {
                ("bookmarkStart" | "bookmarkEnd", "id") => renumbered(value),
                ("bookmarkStart", "name") => renamed(value),
                ("hyperlink", "anchor") if !external_link => renamed(value),
                ("fldSimple", "instr") if !links_another_file(value) => renamed(value),
                _ => None,
            }
        })?;
        if let Some(updated) = updated {
            edits.push((node.start..node.open_end, updated));
        }
    }
    for (elements, _) in std::mem::take(&mut fields) {
        if !instruction(elements, &mut edits) {
            return Ok(None);
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut updated = xml.to_vec();
    for (range, value) in edits.into_iter().rev() {
        updated.splice(range, value);
    }
    Ok(Some(updated))
}

/// Whether an element carries an `r:id`, the relationship of a link target.
fn relationship_link(
    tree: &crate::revision::XmlTree<'_>,
    xml: &[u8],
    index: usize,
) -> Result<bool> {
    let node = &tree.elements[index];
    let scope = tree
        .source_scope(index)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut reader = Reader::from_reader(&xml[node.start..node.open_end]);
    let (Event::Start(start) | Event::Empty(start)) = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    else {
        unreachable!("element opening");
    };
    Ok(start.attributes().flatten().any(|attribute| {
        std::str::from_utf8(attribute.key.as_ref())
            .ok()
            .and_then(|key| key.split_once(':'))
            .is_some_and(|(prefix, local)| {
                local == "id"
                    && scope
                        .get(prefix)
                        .is_some_and(|uri| uri == oxoffice_docx_oxml::namespace::R_NS)
            })
    }))
}

/// Whether a field instruction is a `HYPERLINK` to another file, whose `\l`
/// names a bookmark of that file.
fn links_another_file(instruction: &[u8]) -> bool {
    let mut words = instruction
        .split(u8::is_ascii_whitespace)
        .filter(|word| !word.is_empty());
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case(b"HYPERLINK"))
        && words.next().is_some_and(|word| !word.starts_with(b"\\"))
}

/// The opening tag of an element with some Word attribute values replaced,
/// or None when `value` replaces none. `value` takes the local name and the
/// raw value, and returns the raw replacement.
pub(super) fn rewritten_opening(
    tree: &crate::revision::XmlTree<'_>,
    xml: &[u8],
    index: usize,
    value: impl Fn(&str, &[u8]) -> Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let node = &tree.elements[index];
    let scope = tree
        .source_scope(index)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut reader = Reader::from_reader(&xml[node.start..node.open_end]);
    let (Event::Start(start) | Event::Empty(start)) = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    else {
        unreachable!("element opening");
    };
    let mut changed = false;
    let mut updated = BytesStart::new(node.name.as_str());
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let key = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        let replacement = key
            .split_once(':')
            .filter(|(prefix, _)| scope.get(*prefix).is_some_and(|uri| uri == W_NS))
            .and_then(|(_, local)| value(local, &attribute.value));
        match replacement {
            Some(replacement) => {
                changed = true;
                updated.push_attribute((key.as_bytes(), replacement.as_slice()));
            }
            None => updated.push_attribute((key.as_bytes(), attribute.value.as_ref())),
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(if node.empty {
        Event::Empty(updated)
    } else {
        Event::Start(updated)
    })?;
    Ok(Some(writer.into_inner()))
}
