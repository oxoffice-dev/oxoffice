//! Footnote custody: note references, markers, ids and parts across a comparison.

use super::*;

const FOOTNOTE_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

#[derive(PartialEq, Eq)]
enum FootnoteRootAnchor {
    Normal(i32),
    Retained(usize),
}

impl FootnotePart {
    pub(super) fn of(document: &Document) -> Result<Option<Self>> {
        let Some(name) = Self::name_of(document)? else {
            return Ok(None);
        };
        let xml = std::str::from_utf8(
            document
                .package
                .get_part(&name)
                .ok_or_else(|| Error::Other(format!("missing footnotes part {name}")))?,
        )
        .map_err(utf8_error)?
        .to_owned();
        Self::parse(name, xml).map(Some)
    }

    /// The footnotes part name, from the one internal relationship and its
    /// content type, without reading the part.
    fn name_of(document: &Document) -> Result<Option<String>> {
        let relationships = document.package.get_part_rels(&document.doc_part_name);
        let links = relationships
            .into_iter()
            .flat_map(|rels| &rels.items)
            .filter(|rel| rel.rel_type == rel_types::FOOTNOTES)
            .collect::<Vec<_>>();
        if links.is_empty() {
            return Ok(None);
        }
        if links.len() != 1 || !crate::document::relationship_is_internal(links[0]) {
            return Err(Error::Other(
                "footnotes require exactly one internal relationship".into(),
            ));
        }
        let name = OpcPackage::resolve_rel_target(&document.doc_part_name, &links[0].target);
        if document.package.content_types.content_type_for(&name) != Some(FOOTNOTE_CONTENT_TYPE) {
            return Err(Error::Other(format!(
                "footnotes content type is invalid in {name}"
            )));
        }
        if !document.package.contains_part(&name) {
            return Err(Error::Other(format!("missing footnotes part {name}")));
        }
        Ok(Some(name))
    }

    pub(super) fn parse(name: String, xml: String) -> Result<Self> {
        let bindings = root_namespace_bindings(&xml, "footnotes", &[])?;
        let tree = crate::revision::XmlTree::parse(xml.as_bytes())?;
        let mut owners = Vec::new();
        let mut ids = HashSet::new();
        for &index in &tree.elements[tree.root].children {
            let element = &tree.elements[index];
            if !element.word || element.local != "footnote" {
                continue;
            }
            let id = tree
                .word_attribute(index, "id")?
                .and_then(|value| value.parse().ok());
            if let Some(id) = id
                && !ids.insert(id)
            {
                return Err(Error::Other(format!(
                    "ambiguous duplicate footnote owner {id} in {name}"
                )));
            }
            let span = element.start..element.end;
            let owner_xml = &xml[span.clone()];
            owners.push(FootnoteOwner {
                id,
                kind: raw_note_type_in_scope(owner_xml, &bindings)?,
                inner: element.open_end..element.close_start,
                bindings: root_namespace_bindings(owner_xml, "footnote", &bindings)?,
                span,
            });
        }
        drop(tree);
        Ok(Self { name, xml, owners })
    }

    pub(super) fn normal(&self) -> BTreeMap<i32, &FootnoteOwner> {
        self.owners
            .iter()
            .filter(|owner| {
                matches!(
                    owner.kind,
                    RawNoteType::Omitted | RawNoteType::ExplicitNormal
                )
            })
            .filter_map(|owner| owner.id.map(|id| (id, owner)))
            .collect()
    }

    fn root_items(&self) -> Result<Vec<(Option<i32>, Range<usize>)>> {
        let tree = crate::revision::XmlTree::parse(self.xml.as_bytes())?;
        let root = &tree.elements[tree.root];
        let mut cursor = root.open_end;
        let mut items = Vec::new();
        for &index in &root.children {
            let node = &tree.elements[index];
            if !self.xml[cursor..node.start]
                .bytes()
                .all(|byte| byte.is_ascii_whitespace())
            {
                items.push((None, cursor..node.start));
            }
            let normal = self
                .owners
                .iter()
                .find(|owner| {
                    owner.span.start == node.start
                        && matches!(
                            owner.kind,
                            RawNoteType::Omitted | RawNoteType::ExplicitNormal
                        )
                })
                .and_then(|owner| owner.id);
            items.push((normal, node.start..node.end));
            cursor = node.end;
        }
        if !self.xml[cursor..root.close_start]
            .bytes()
            .all(|byte| byte.is_ascii_whitespace())
        {
            items.push((None, cursor..root.close_start));
        }
        Ok(items)
    }

    fn retained_records(&self) -> Result<Vec<String>> {
        Ok(self
            .root_items()?
            .into_iter()
            .filter(|(id, _)| id.is_none())
            .map(|(_, range)| self.xml[range].to_owned())
            .collect())
    }

    fn unowned_record_count(&self) -> Result<usize> {
        let specials = self
            .owners
            .iter()
            .filter(|owner| {
                matches!(
                    owner.kind,
                    RawNoteType::Separator
                        | RawNoteType::ContinuationSeparator
                        | RawNoteType::ContinuationNotice
                )
            })
            .count();
        Ok(self.retained_records()?.len().saturating_sub(specials))
    }

    fn retained_shell(&self) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let mut xml = self.xml.clone();
        for owner in self.owners.iter().rev().filter(|owner| {
            matches!(
                owner.kind,
                RawNoteType::Omitted | RawNoteType::ExplicitNormal
            ) && owner.id.is_some()
        }) {
            xml.replace_range(owner.span.clone(), "");
        }
        canonical_owned_story(&xml, "footnote")
    }
}

#[derive(Clone)]
struct FootnoteReferenceSite {
    part: String,
    id: i32,
    run: Range<usize>,
    paragraph: Range<usize>,
    revision: Option<Range<usize>>,
    /// The metadata of that nearest revision wrapper, read with the site.
    metadata: Option<crate::revision::RevisionMetadata>,
}

fn footnote_reference_sites(document: &Document) -> Result<Vec<FootnoteReferenceSite>> {
    let mut parts = vec![(StoryKind::Body, document.doc_part_name.clone())];
    parts.extend(
        revision_story_parts(document)?
            .into_iter()
            .filter(|(kind, _)| !matches!(kind, StoryKind::Footnote | StoryKind::Endnote)),
    );
    parts.sort_by(|left, right| left.1.cmp(&right.1));
    parts.dedup_by(|left, right| left.1 == right.1);
    let mut sites = Vec::new();
    for (kind, part) in parts {
        let bytes = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("missing reference story {part}")))?;
        let tree = crate::revision::XmlTree::parse(bytes)?;
        let admitted = tree.admitted_story_elements(kind)?;
        for (index, element) in tree.elements.iter().enumerate() {
            if !element.word || element.local != "footnoteReference" {
                continue;
            }
            let Some(run_index) = element.parent.filter(|&parent| {
                tree.elements[parent].word && tree.elements[parent].local == "r" && admitted[parent]
            }) else {
                continue;
            };
            let run = &tree.elements[run_index];
            let mut parent = run.parent;
            let mut paragraph = None;
            let mut revision = None;
            let mut metadata = None;
            while let Some(at) = parent {
                let ancestor = &tree.elements[at];
                if ancestor.revision.is_some() && revision.is_none() {
                    revision = Some(ancestor.start..ancestor.end);
                    metadata = ancestor.revision.clone();
                }
                if ancestor.word && ancestor.local == "p" {
                    paragraph = Some(ancestor.start..ancestor.end);
                    break;
                }
                parent = ancestor.parent;
            }
            let paragraph = paragraph
                .ok_or_else(|| Error::Other("footnote reference has no paragraph owner".into()))?;
            let id = tree
                .word_attribute(index, "id")?
                .and_then(|value| value.parse::<i32>().ok())
                .ok_or_else(|| {
                    Error::Other(format!("invalid footnote reference identity in {part}"))
                })?;
            sites.push(FootnoteReferenceSite {
                part: part.clone(),
                id,
                run: run.start..run.end,
                paragraph,
                revision,
                metadata,
            });
        }
    }
    Ok(sites)
}

fn note_references_have_same_owners(
    original: &Document,
    edited: &Document,
    id: i32,
) -> Result<bool> {
    let left = footnote_reference_sites(original)?;
    let right = footnote_reference_sites(edited)?;
    let parts = left
        .iter()
        .chain(&right)
        .filter(|site| site.id == id)
        .map(|site| site.part.clone())
        .collect::<HashSet<_>>();
    for part in parts {
        let (Some(left_xml), Some(right_xml)) = (
            original.package.get_part(&part),
            edited.package.get_part(&part),
        ) else {
            return Ok(false);
        };
        let left_source = std::str::from_utf8(left_xml).map_err(utf8_error)?;
        let right_source = std::str::from_utf8(right_xml).map_err(utf8_error)?;
        let left_tree = crate::revision::XmlTree::parse(left_xml)?;
        let right_tree = crate::revision::XmlTree::parse(right_xml)?;
        let paragraphs = |tree: &crate::revision::XmlTree<'_>,
                          source: &str|
         -> Result<Vec<(Range<usize>, String)>> {
            tree.elements
                .iter()
                .filter(|node| node.word && node.local == "p" && node.modeled)
                .map(|node| {
                    let closed = source_child(source, node.start..node.end)?;
                    let signature =
                        canonical_owned_story(&format!("<scope>{closed}</scope>"), "p")?;
                    Ok((node.start..node.end, format!("{signature:?}")))
                })
                .collect()
        };
        let l = paragraphs(&left_tree, left_source)?;
        let r = paragraphs(&right_tree, right_source)?;
        let aligned = align(
            &l.iter()
                .map(|(_, signature)| signature.clone())
                .collect::<Vec<_>>(),
            &r.iter()
                .map(|(_, signature)| signature.clone())
                .collect::<Vec<_>>(),
        );
        let mut paired = HashMap::new();
        for (a, b) in aligned {
            if let (Some(a), Some(b)) = (a, b) {
                paired.insert(l[a].0.start, r[b].0.start);
            }
        }
        let key = |site: &FootnoteReferenceSite,
                   tree: &crate::revision::XmlTree<'_>,
                   sites: &[FootnoteReferenceSite],
                   at: usize| {
            let run = tree
                .elements
                .iter()
                .filter(|node| {
                    node.word
                        && node.local == "r"
                        && node.start >= site.paragraph.start
                        && node.end <= site.paragraph.end
                        && node.start < site.run.start
                })
                .count();
            let ordinal = sites[..at]
                .iter()
                .filter(|other| other.part == site.part && other.run == site.run)
                .count();
            (run, ordinal)
        };
        let mut a = Vec::new();
        let mut b = Vec::new();
        for (at, site) in left
            .iter()
            .enumerate()
            .filter(|(_, site)| site.part == part && site.id == id)
        {
            let Some(&paragraph) = paired.get(&site.paragraph.start) else {
                return Ok(false);
            };
            a.push((paragraph, key(site, &left_tree, &left, at)));
        }
        for (at, site) in right
            .iter()
            .enumerate()
            .filter(|(_, site)| site.part == part && site.id == id)
        {
            b.push((site.paragraph.start, key(site, &right_tree, &right, at)));
        }
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_footnote_reference_owners(
    part: Option<&FootnotePart>,
    sites: &[FootnoteReferenceSite],
) -> Result<()> {
    for site in sites {
        let owner = part
            .and_then(|part| part.owners.iter().find(|owner| owner.id == Some(site.id)))
            .ok_or_else(|| {
                Error::Other(format!(
                    "footnote reference {} in {} has no owner",
                    site.id, site.part
                ))
            })?;
        if matches!(
            owner.kind,
            RawNoteType::Separator
                | RawNoteType::ContinuationSeparator
                | RawNoteType::ContinuationNotice
        ) {
            return Err(Error::Other(format!(
                "footnote reference {} targets a special owner",
                site.id
            )));
        }
    }
    Ok(())
}

pub(super) fn preflight_footnote_inputs(
    original: &Document,
    edited: &Document,
    ignored: bool,
) -> Result<()> {
    let left = FootnotePart::of(original)?;
    let right = FootnotePart::of(edited)?;
    let left_refs = footnote_reference_sites(original)?;
    let right_refs = footnote_reference_sites(edited)?;
    for document in [original, edited] {
        if let Some(part) = FootnotePart::of(document)?
            && part.normal().is_empty()
            && part.retained_records()?.is_empty()
            && validate_footnote_custody(document, &crate::revision::RevisionScope::All)?.is_empty()
        {
            return Err(Error::Other(format!(
                "empty footnotes part {} has no custody or retained records",
                part.name
            )));
        }
    }
    for (part, sites) in [(&left, &left_refs), (&right, &right_refs)] {
        validate_footnote_reference_owners(part.as_ref(), sites)?;
    }
    for (part, other) in [(&left, &right), (&right, &left)] {
        if let Some(part) = part {
            for owner in &part.owners {
                if !matches!(owner.kind, RawNoteType::Empty | RawNoteType::Unknown) {
                    continue;
                }
                let same = other.as_ref().and_then(|other| {
                    other
                        .owners
                        .iter()
                        .find(|candidate| candidate.id == owner.id)
                        .map(|candidate| &other.xml[candidate.span.clone()])
                }) == Some(&part.xml[owner.span.clone()]);
                let references_same = owner
                    .id
                    .map(|id| note_references_have_same_owners(original, edited, id))
                    .transpose()?
                    .unwrap_or(true);
                if !same || !references_same {
                    return Err(Error::Other(format!(
                        "changed invalid footnote owner or reference {:?}",
                        owner.id
                    )));
                }
            }
        }
    }
    if ignored {
        for site in right_refs {
            if !left
                .as_ref()
                .is_some_and(|part| part.normal().contains_key(&site.id))
            {
                return Err(Error::Other(format!(
                    "ignored footnotes cannot supply reference {}",
                    site.id
                )));
            }
        }
    }
    Ok(())
}

fn note_marker_name(name: &str) -> Option<(i32, bool, bool, char)> {
    let tail = name.strip_prefix("_rdocxCmpNoteF")?;
    let (digits, bits) = tail.split_once('P')?;
    if digits.is_empty()
        || digits.len() > 9
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || bits.len() != 3
    {
        return None;
    }
    let bytes = bits.as_bytes();
    if !matches!(bytes[0], b'0' | b'1')
        || !matches!(bytes[1], b'0' | b'1')
        || !matches!(bytes[2], b'B' | b'N')
    {
        return None;
    }
    Some((
        digits.parse().ok()?,
        bytes[0] == b'1',
        bytes[1] == b'1',
        char::from(bytes[2]),
    ))
}

pub(super) fn note_markers(document: &Document) -> Result<Vec<NoteMarker>> {
    let note_part = FootnotePart::of(document)?;
    let mut parts = vec![(document.doc_part_name.clone(), StoryKind::Body)];
    parts.extend(story_parts(document)?.into_iter().map(|part| {
        let kind = match part.kind {
            ComparisonStoryKind::Header => StoryKind::Header,
            ComparisonStoryKind::Footer => StoryKind::Footer,
            ComparisonStoryKind::Comment => StoryKind::Comment,
            ComparisonStoryKind::Footnote => StoryKind::Footnote,
            ComparisonStoryKind::Endnote => StoryKind::Endnote,
            ComparisonStoryKind::Main | ComparisonStoryKind::TextBox => StoryKind::Body,
        };
        (part.part_name, kind)
    }));
    parts.sort_by(|left, right| left.0.cmp(&right.0));
    parts.dedup_by(|left, right| left.0 == right.0);
    let mut markers = Vec::new();
    for (part, kind) in parts {
        let source = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("missing custody story {part}")))?;
        let tree = crate::revision::XmlTree::parse(source)?;
        let paragraphs = crate::document::admitted_story_paragraph_spans(source, kind)?;
        // The real owner inventory excludes opaque descendant stories. Also
        // require the marker's own path to that paragraph to stay modeled.
        let marker_paragraph = |index: usize| {
            // Bookmarks themselves are retained raw leaves in XmlTree. Their
            // actual containing path, rather than that leaf classification,
            // must be admitted and modeled.
            let mut at = tree.elements[index].parent;
            while let Some(index) = at {
                let node = &tree.elements[index];
                if !node.word || !node.modeled {
                    return None;
                }
                if node.local == "p" {
                    return paragraphs
                        .contains(&(node.start..node.end))
                        .then_some(index);
                }
                at = node.parent;
            }
            None
        };
        let mut names = HashSet::new();
        for (index, node) in tree.elements.iter().enumerate() {
            if !node.word || node.local != "bookmarkStart" || marker_paragraph(index).is_none() {
                continue;
            }
            let Some(name) = tree.word_attribute(index, "name")? else {
                continue;
            };
            let Some((token, original, edited, family)) = note_marker_name(&name) else {
                continue;
            };
            let Some(parent) = node.parent else {
                continue;
            };
            let siblings = &tree.elements[parent].children;
            let position = siblings
                .iter()
                .position(|child| *child == index)
                .expect("bookmark child");
            let Some(&end_index) = siblings.get(position + 1) else {
                return Err(Error::Other(
                    "custody bookmark pair has no adjacent end".into(),
                ));
            };
            let end = &tree.elements[end_index];
            let Some(pair_id) = tree
                .word_attribute(index, "id")?
                .and_then(|id| id.parse::<i32>().ok())
            else {
                return Err(Error::Other("custody bookmark pair has no valid id".into()));
            };
            if !end.word
                || end.local != "bookmarkEnd"
                || node.end != end.start
                || tree.word_attribute(end_index, "id")?.as_deref()
                    != Some(pair_id.to_string().as_str())
            {
                return Err(Error::Other(
                    "custody bookmark pair has an invalid adjacent end".into(),
                ));
            }
            let occurrences = tree
                .elements
                .iter()
                .enumerate()
                .filter(|(at, e)| {
                    e.word
                        && matches!(e.local.as_str(), "bookmarkStart" | "bookmarkEnd")
                        && marker_paragraph(*at).is_some()
                })
                .map(|(i, _)| tree.word_attribute(i, "id"))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .filter(|id| id.as_deref() == Some(pair_id.to_string().as_str()))
                .count();
            if occurrences != 2 || !names.insert(name) {
                return Err(Error::Other(
                    "duplicate custody bookmark name or pair id".into(),
                ));
            }
            let p = if tree.elements[parent].word && tree.elements[parent].local == "p" {
                Some(parent)
            } else {
                tree.elements[parent].parent.filter(|&at| {
                    tree.elements[at].word
                        && tree.elements[at].local == "p"
                        && tree.elements[parent].revision.is_some()
                })
            };
            let Some(paragraph) = p else {
                return Err(Error::Other(
                    "custody pair has no direct paragraph owner".into(),
                ));
            };
            // Word for Mac 16.115 merges adjacent insertions of one author and
            // date, which can move the body pair into its own reference's
            // insertion. Admit only that position: an insertion directly in a
            // main-story paragraph whose next child after the pair is the
            // reference run, the same next-sibling proof as for a direct pair.
            // validate_footnote_custody also requires the note pair there.
            let wrapped = family == 'B' && paragraph != parent;
            if wrapped {
                let wrapper = &tree.elements[parent];
                let own_reference = part == document.doc_part_name
                    && wrapper.word
                    && wrapper.local == "ins"
                    && wrapper.revision.as_ref().is_some_and(|metadata| {
                        metadata.kind == crate::revision::RevisionKind::Insertion
                    })
                    && siblings.get(position + 2).is_some_and(|&next| {
                        let run = &tree.elements[next];
                        run.word
                            && run.local == "r"
                            && run.children.iter().any(|&leaf| {
                                tree.elements[leaf].word
                                    && tree.elements[leaf].local == "footnoteReference"
                            })
                    });
                if !own_reference {
                    return Err(Error::Other(
                        "body custody pair is inside a revision".into(),
                    ));
                }
            }
            let mut owner = None;
            let mut owner_index = None;
            let mut ancestor = Some(paragraph);
            while let Some(at) = ancestor {
                if tree.elements[at].word && tree.elements[at].local == "footnote" {
                    owner = tree
                        .word_attribute(at, "id")?
                        .and_then(|value| value.parse().ok());
                    owner_index = Some(at);
                    break;
                }
                ancestor = tree.elements[at].parent;
            }
            if family == 'N' {
                let Some(at) = owner_index else {
                    return Err(Error::Other("note custody pair has no note owner".into()));
                };
                let owner_node = &tree.elements[at];
                let actual_owner = note_part
                    .as_ref()
                    .filter(|notes| notes.name == part)
                    .and_then(|notes| {
                        notes.owners.iter().find(|owner| {
                            owner.span == (owner_node.start..owner_node.end)
                                && matches!(
                                    owner.kind,
                                    RawNoteType::Omitted | RawNoteType::ExplicitNormal
                                )
                                && owner.id.is_some()
                        })
                    });
                if actual_owner.is_none() {
                    return Err(Error::Other(
                        "note custody pair has no actual normal definition".into(),
                    ));
                }
                let first = paragraphs.iter().find(|span| {
                    span.start >= owner_node.open_end && span.end <= owner_node.close_start
                });
                if first != Some(&(tree.elements[paragraph].start..tree.elements[paragraph].end)) {
                    return Err(Error::Other(
                        "note custody pair is not in its first note paragraph".into(),
                    ));
                }
            }
            if family == 'B' && owner.is_some() {
                return Err(Error::Other("body custody pair is in a note owner".into()));
            }
            let mut reference = None;
            if family == 'B'
                && let Some(&next) = siblings.get(position + 2)
            {
                let following = &tree.elements[next];
                let eligible =
                    following.word && (following.local == "r" || following.revision.is_some());
                for (at, child) in tree.elements.iter().enumerate().filter(|(_, e)| {
                    eligible && e.start >= following.start && e.end <= following.end
                }) {
                    if child.word && child.local == "footnoteReference" {
                        reference = tree
                            .word_attribute(at, "id")?
                            .and_then(|value| value.parse().ok());
                        break;
                    }
                }
            }

            markers.push(NoteMarker {
                token,
                original,
                edited,
                family,
                part: part.clone(),
                span: node.start..end.end,
                paragraph: tree.elements[paragraph].start..tree.elements[paragraph].end,
                pair_id,
                owner,
                reference,
                wrapped,
            });
        }
    }
    Ok(markers)
}

pub(super) fn footnote_custody_reserved_ids(document: &Document) -> Result<HashSet<i32>> {
    Ok(note_markers(document)?
        .iter()
        .flat_map(|marker| [marker.token, marker.pair_id])
        .collect())
}

pub(crate) fn validate_footnote_custody(
    document: &Document,
    scope: &crate::revision::RevisionScope<'_>,
) -> Result<Vec<FootnoteCustody>> {
    let markers = note_markers(document)?;
    let Some(part) = FootnotePart::of(document)? else {
        if markers.iter().any(|marker| marker.family == 'N') {
            return Err(Error::Other("custody marker has no footnotes part".into()));
        }
        // Without its note pair, a body pair inside a revision has no proof.
        if markers.iter().any(|marker| marker.wrapped) {
            return Err(Error::Other(
                "body custody pair is inside a revision".into(),
            ));
        }
        // Physical reference coherence is independent of the orphan bookmark.
        validate_footnote_reference_owners(None, &footnote_reference_sites(document)?)?;
        return Ok(markers
            .iter()
            .filter(|marker| marker.family == 'B')
            .map(|marker| FootnoteCustody {
                token: Some(marker.token),
                original: marker.original,
                edited: marker.edited,
                owner: None,
                addition: None,
                selected: false,
                revisions: Vec::new(),
                wrapped_body: marker.wrapped.then(|| marker.span.clone()),
            })
            .collect());
    };
    let normal = part.normal();
    let sites = footnote_reference_sites(document)?;
    let mut note_tree = crate::revision::XmlTree::parse(part.xml.as_bytes())?;
    note_tree.qualify_note_revisions()?;
    let mut units = Vec::new();
    let mut used_notes = HashSet::new();
    let mut owned_definitions = HashSet::new();
    let mut wrapped_unmatched = Vec::new();
    for body in markers.iter().filter(|marker| marker.family == 'B') {
        let notes = markers
            .iter()
            .filter(|note| note.family == 'N' && note.token == body.token)
            .collect::<Vec<_>>();
        if notes.len() > 1 {
            return Err(Error::Other(
                "custody token binds multiple note owners".into(),
            ));
        }
        let note = notes.first().copied();
        // The next-sibling proof inside a revision holds only with the note
        // pair, which binds the owner. An unmatched pair stays where Word left
        // it only at the direct position, or before a tracked note's reference.
        if note.is_none() && body.wrapped {
            wrapped_unmatched.push(body.reference);
        }
        if let Some(note) = note {
            if note.owner.is_none() {
                return Err(Error::Other(
                    "note custody counterpart has no physical note identity".into(),
                ));
            }
            if note.original != body.original || note.edited != body.edited {
                return Err(Error::Other("custody part-presence bits disagree".into()));
            }
            if body.reference != note.owner {
                return Err(Error::Other(
                    "custody reference and note owner disagree".into(),
                ));
            }
            if !used_notes.insert((note.part.clone(), note.span.start))
                || !owned_definitions.insert(note.owner)
            {
                return Err(Error::Other(
                    "multiple custody units bind one physical note owner".into(),
                ));
            }
        }
        // An unmatched body pair has no surviving note-owner identity. A
        // following run or revision wrapper may belong to an unrelated note.
        let owner = note.and_then(|note| note.owner);
        if let Some(owner) = owner
            && !normal.contains_key(&owner)
        {
            return Err(Error::Other(
                "custody is bound to a missing or non-normal note".into(),
            ));
        }
        let mut revisions = Vec::new();
        let mut addition = None;
        if let Some(id) = owner {
            let actual_owner = normal[&id];
            for node in &note_tree.elements {
                if node.start >= actual_owner.inner.start
                    && node.end <= actual_owner.inner.end
                    && let Some(metadata) = &node.revision
                {
                    let add = match metadata.kind {
                        crate::revision::RevisionKind::Insertion => true,
                        crate::revision::RevisionKind::Deletion => false,
                        _ => {
                            return Err(Error::Other(
                                "custody note has an unsupported revision kind".into(),
                            ));
                        }
                    };
                    if addition.is_some_and(|value| value != add) {
                        return Err(Error::Other(
                            "custody note revision directions disagree".into(),
                        ));
                    }
                    addition = Some(add);
                    revisions.push(metadata.clone());
                }
            }
            for site in sites.iter().filter(|site| site.id == id) {
                if let Some(metadata) = site.metadata.clone() {
                    let add = match metadata.kind {
                        crate::revision::RevisionKind::Insertion => true,
                        crate::revision::RevisionKind::Deletion => false,
                        _ => {
                            return Err(Error::Other(
                                "custody reference has an unsupported revision kind".into(),
                            ));
                        }
                    };
                    if addition.is_some_and(|value| value != add) {
                        return Err(Error::Other(
                            "custody body/note revision directions disagree".into(),
                        ));
                    }
                    addition = Some(add);
                    revisions.push(metadata);
                } else if addition.is_some() {
                    return Err(Error::Other(
                        "pending custody reference lost its revision wrapper".into(),
                    ));
                }
            }
        }
        let selected = revisions
            .iter()
            .filter(|metadata| scope.matches(metadata))
            .count();
        if selected > 0 && selected != revisions.len() {
            return Err(Error::Other(format!(
                "partial footnote custody selection for token {}",
                body.token
            )));
        }
        units.push(FootnoteCustody {
            token: Some(body.token),
            original: body.original,
            edited: body.edited,
            owner,
            addition,
            selected: selected > 0,
            revisions,
            wrapped_body: body.wrapped.then(|| body.span.clone()),
        });
    }
    if markers
        .iter()
        .filter(|marker| marker.family == 'N')
        .any(|note| !used_notes.contains(&(note.part.clone(), note.span.start)))
    {
        return Err(Error::Other(
            "note custody marker is missing its body counterpart".into(),
        ));
    }
    let tracked = tracked_note_units(&part, &note_tree, &sites, &owned_definitions, scope);
    // An unmatched pair inside its reference's insertion stays only when that
    // reference's note is a tracked note, proved from the note's own bytes.
    // The pair is still not associated with it: it only keeps its place, and
    // the insertion resolves with the tracked note.
    if wrapped_unmatched.iter().any(|reference| {
        !reference.is_some_and(|id| tracked.iter().any(|unit| unit.owner == Some(id)))
    }) {
        return Err(Error::Other(
            "body custody pair is inside a revision".into(),
        ));
    }
    units.extend(tracked);
    Ok(units)
}

/// Tracked-note units. Word can drop a note pair, which leaves its body pair
/// without an owner (Q-A430-29). A normal owner that no marker binds is still
/// a unit when its own bytes prove that the whole definition and its only
/// reference are tracked as one insertion or one deletion. The proof reads the
/// owner and binds the reference by its footnote id. It never reads a marker
/// or an adjacent element, so an unmatched body pair stays unassociated.
fn tracked_note_units(
    part: &FootnotePart,
    note_tree: &crate::revision::XmlTree<'_>,
    sites: &[FootnoteReferenceSite],
    owned: &HashSet<Option<i32>>,
    scope: &crate::revision::RevisionScope<'_>,
) -> Vec<FootnoteCustody> {
    let mut references = BTreeMap::<i32, Vec<&FootnoteReferenceSite>>::new();
    for site in sites {
        references.entry(site.id).or_default().push(site);
    }
    let mut units = Vec::new();
    for (&id, owner) in &part.normal() {
        if owned.contains(&Some(id)) {
            continue;
        }
        let sites = references.get(&id).map(Vec::as_slice).unwrap_or_default();
        // Two or more references belong to the shared-reference rules.
        if sites.len() > 1 {
            continue;
        }
        let Some((kind, mut revisions)) = tracked_note_owner(note_tree, owner) else {
            continue;
        };
        if let Some(site) = sites.first() {
            match &site.metadata {
                Some(metadata) if metadata.kind == kind => revisions.push(metadata.clone()),
                _ => continue,
            }
        }
        let selected = revisions
            .iter()
            .filter(|metadata| scope.matches(metadata))
            .count();
        units.push(FootnoteCustody {
            token: None,
            original: true,
            edited: true,
            owner: Some(id),
            addition: Some(kind == crate::revision::RevisionKind::Insertion),
            // Only a whole selection resolves the unit as one. A partial one
            // keeps the ordinary element rules, and the final-paragraph rule.
            selected: selected == revisions.len(),
            revisions,
            wrapped_body: None,
        });
    }
    units
}

/// The revision kind and every revision of a note owner whose paragraphs all
/// carry a paragraph-mark revision of one kind, and whose other paragraph
/// children are all wrappers of that kind or zero-width range markers.
fn tracked_note_owner(
    tree: &crate::revision::XmlTree<'_>,
    owner: &FootnoteOwner,
) -> Option<(
    crate::revision::RevisionKind,
    Vec<crate::revision::RevisionMetadata>,
)> {
    use crate::revision::RevisionKind;
    let first = tree
        .elements
        .partition_point(|node| node.start < owner.span.start);
    let node = tree.elements.get(first)?;
    if node.start != owner.span.start || !node.word || node.local != "footnote" {
        return None;
    }
    let mut kind = None;
    let mut agrees = |metadata: &crate::revision::RevisionMetadata| {
        matches!(
            metadata.kind,
            RevisionKind::Insertion | RevisionKind::Deletion
        ) && *kind.get_or_insert(metadata.kind) == metadata.kind
    };
    if node.children.is_empty() {
        return None;
    }
    for &paragraph in &node.children {
        let p = &tree.elements[paragraph];
        if !p.word || p.local != "p" {
            return None;
        }
        let mut mark = false;
        for &child in &p.children {
            let element = &tree.elements[child];
            if element.word && element.local == "pPr" {
                for &properties in &element.children {
                    let run_properties = &tree.elements[properties];
                    if !run_properties.word || run_properties.local != "rPr" {
                        continue;
                    }
                    for &marker in &run_properties.children {
                        if let Some(metadata) = &tree.elements[marker].revision {
                            if !agrees(metadata) {
                                return None;
                            }
                            mark = true;
                        }
                    }
                }
                continue;
            }
            match &element.revision {
                Some(metadata)
                    if element.word && matches!(element.local.as_str(), "ins" | "del") =>
                {
                    if !agrees(metadata) {
                        return None;
                    }
                }
                None if element.word
                    && matches!(
                        element.local.as_str(),
                        "bookmarkStart" | "bookmarkEnd" | "proofErr"
                    ) => {}
                _ => return None,
            }
        }
        if !mark {
            return None;
        }
    }
    let mut revisions = Vec::new();
    for element in tree.elements[first..]
        .iter()
        .take_while(|element| element.start < owner.span.end)
    {
        if let Some(metadata) = &element.revision {
            if !agrees(metadata) {
                return None;
            }
            revisions.push(metadata.clone());
        }
    }
    Some((kind?, revisions))
}

/// Selected definition retirement precedes the ordinary story renderer, whose
/// final-paragraph rule applies to retained owners, not a removed note owner.
pub(crate) fn remove_selected_footnote_definitions(
    document: &mut Document,
    units: &[FootnoteCustody],
    accept: bool,
    publication: Option<&mut crate::revision::NotePublication>,
) -> Result<usize> {
    let Some(mut part) = FootnotePart::of(document)? else {
        return Ok(0);
    };
    let mut tree = crate::revision::XmlTree::parse(part.xml.as_bytes())?;
    tree.qualify_note_revisions()?;
    let normal = part.normal();
    let mut ranges = Vec::new();
    let mut count = 0usize;
    let mut removed_links = HashSet::new();
    for unit in units
        .iter()
        .filter(|unit| unit.selected && unit.addition.is_some_and(|addition| addition != accept))
    {
        if let Some(owner) = unit.owner.and_then(|id| normal.get(&id)) {
            count = count
                .checked_add(
                    tree.elements
                        .iter()
                        .filter(|node| {
                            node.start >= owner.inner.start
                                && node.end <= owner.inner.end
                                && node.revision.is_some()
                        })
                        .count(),
                )
                .ok_or_else(|| Error::Other("resolved note revision count overflowed".into()))?;
            removed_links.extend(crate::document::xml_relationship_ids_in_order(
                close_note_fragment(&part.xml[owner.span.clone()], &owner.bindings)?.as_bytes(),
            )?);
            ranges.push(owner.span.clone());
        }
    }
    if ranges.is_empty() {
        return Ok(0);
    }
    drop(normal);
    drop(tree);
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    if let Some(trace) = publication {
        trace.remove_ranges(&part.name, part.xml.as_bytes(), &ranges)?;
    }
    for range in ranges.into_iter().rev() {
        part.xml.replace_range(range, "");
    }
    let retained_links = crate::document::xml_relationship_ids_in_order(part.xml.as_bytes())?
        .into_iter()
        .collect::<HashSet<_>>();
    removed_links.retain(|id| !retained_links.contains(id));
    crate::document::set_story_source_xml(document, &part.name, part.xml.into_bytes())?;
    remove_note_owned_relationships(document, &part.name, &removed_links)?;
    Ok(count)
}

pub(crate) fn settle_footnote_custody(
    document: &mut Document,
    units: &[FootnoteCustody],
    accept: bool,
    note_part_resolved: bool,
    mut publication: Option<&mut crate::revision::NotePublication>,
) -> Result<()> {
    if !units.iter().any(|unit| !unit.revisions.is_empty()) {
        // No unit is recognized as pending, for example after Word dropped a
        // note pair. A resolution that edits the note part or resolves a note
        // reference, and leaves no note revision pending, still settles the
        // part, and settlement retires its remaining custody pairs. A pair in
        // an untouched part stays.
        if note_part_resolved && !units.is_empty() && !note_revisions_pending(document, true)? {
            retire_note_markers(document, units, publication)?;
        }
        return Ok(());
    }
    let removed = units
        .iter()
        .filter(|unit| unit.selected && unit.addition.is_some_and(|addition| addition != accept))
        .filter_map(|unit| unit.owner)
        .collect::<Vec<_>>();
    let unit_pending = units
        .iter()
        .any(|unit| !unit.revisions.is_empty() && !unit.selected);
    // Nothing to remove, and either a unit still pending or no marked unit
    // whose pairs or bits settlement would act on: nothing settles here, so
    // the note part is not read again. A publication trace still checks its
    // source.
    if removed.is_empty()
        && publication.is_none()
        && (unit_pending || !units.iter().any(|unit| unit.token.is_some()))
    {
        return if FootnotePart::name_of(document)?.is_some() {
            Ok(())
        } else {
            Err(Error::Other(
                "footnotes part disappeared before custody settlement".into(),
            ))
        };
    }
    let Some(mut part) = FootnotePart::of(document)? else {
        return Err(Error::Other(
            "footnotes part disappeared before custody settlement".into(),
        ));
    };
    let refs = if removed.is_empty() {
        Vec::new()
    } else {
        footnote_reference_sites(document)?
    };
    let mut removals = Vec::new();
    for id in removed {
        if refs.iter().any(|site| site.id == id) {
            return Err(Error::Other(
                "resolved note deletion retained a live reference".into(),
            ));
        }
        if let Some(owner) = part.normal().get(&id) {
            removals.push(owner.span.clone());
        }
    }
    removals.sort_by_key(|range| range.start);
    removals.dedup();
    if let Some(trace) = publication.as_deref_mut() {
        trace.remove_ranges(&part.name, part.xml.as_bytes(), &removals)?;
    }
    if !removals.is_empty() || publication.is_some() {
        for range in removals.into_iter().rev() {
            part.xml.replace_range(range, "");
        }
        crate::document::set_story_source_xml(document, &part.name, part.xml.into_bytes())?;
        part = FootnotePart::of(document)?.expect("installed notes");
    }
    // A tracked note that settles here leaves the pairs in place while a note
    // owner outside every recognized unit still holds a revision, the rule of
    // a settlement that recognizes no unit. Only marked units carry pairs.
    let pending = unit_pending
        || (units
            .iter()
            .any(|unit| unit.token.is_none() && unit.selected)
            && units.iter().any(|unit| unit.token.is_some())
            && note_revisions_pending(document, false)?);
    if pending {
        return Ok(());
    }
    let open = units.iter().any(|unit| !unit.revisions.is_empty());
    let mut marked = units.iter().filter(|unit| unit.token.is_some());
    let absent = open
        && part.normal().is_empty()
        && marked.any(|unit| !unit.original || !unit.edited)
        && part.unowned_record_count()? == 0;
    retire_note_markers(document, units, publication)?;
    if absent {
        remove_footnotes_part(document, &part.name)?;
    }
    Ok(())
}

/// Whether a normal note owner, or with `references` a note reference, still
/// carries a revision.
fn note_revisions_pending(document: &Document, references: bool) -> Result<bool> {
    if references
        && footnote_reference_sites(document)?
            .iter()
            .any(|site| site.revision.is_some())
    {
        return Ok(true);
    }
    let Some(part) = FootnotePart::of(document)? else {
        return Ok(false);
    };
    let mut tree = crate::revision::XmlTree::parse(part.xml.as_bytes())?;
    tree.qualify_note_revisions()?;
    let owners = part.normal();
    Ok(tree.elements.iter().any(|node| {
        node.revision.is_some()
            && owners
                .values()
                .any(|owner| node.start >= owner.inner.start && node.end <= owner.inner.end)
    }))
}

pub(super) fn retire_note_markers(
    document: &mut Document,
    units: &[FootnoteCustody],
    mut publication: Option<&mut crate::revision::NotePublication>,
) -> Result<()> {
    let tokens = units
        .iter()
        .filter_map(|unit| unit.token)
        .collect::<HashSet<_>>();
    if tokens.is_empty() {
        return Ok(());
    }
    let mut by_part = BTreeMap::<String, Vec<Range<usize>>>::new();
    for marker in note_markers(document)?
        .into_iter()
        .filter(|marker| tokens.contains(&marker.token))
    {
        by_part.entry(marker.part).or_default().push(marker.span);
    }
    for (part, mut spans) in by_part {
        let mut bytes = document
            .package
            .get_part(&part)
            .expect("marker part")
            .to_vec();
        spans.sort_by_key(|span| span.start);
        spans.dedup();
        if let Some(trace) = publication.as_deref_mut() {
            trace.remove_ranges(&part, &bytes, &spans)?;
        }
        for span in spans.into_iter().rev() {
            bytes.drain(span);
        }
        crate::document::set_story_source_xml(document, &part, bytes)?;
    }
    Ok(())
}

fn remove_note_owned_relationships(
    document: &mut Document,
    owner: &str,
    ids: &HashSet<String>,
) -> Result<()> {
    let mut targets = Vec::new();
    if let Some(rels) = document.package.get_part_rels_mut(owner) {
        rels.items.retain(|rel| {
            if !ids.contains(&rel.id) {
                return true;
            }
            if crate::document::relationship_is_internal(rel) {
                targets.push(OpcPackage::resolve_rel_target(owner, &rel.target));
            }
            false
        });
    }
    remove_unshared_note_targets(document, targets, None)?;
    Ok(())
}

fn remove_unshared_note_targets(
    document: &mut Document,
    mut targets: Vec<String>,
    required_absent: Option<&str>,
) -> Result<()> {
    // Sharing is tested outside the complete retiring closure. Internal edges,
    // including cycles, do not make an otherwise unshared closure live.
    let mut candidates = BTreeMap::new();
    while let Some(target) = targets.pop() {
        let key = target.to_ascii_lowercase();
        if candidates.contains_key(&key) {
            continue;
        }
        if !document.package.contains_part(&target) {
            return Err(Error::Other(format!(
                "note-owned target is missing: {target}"
            )));
        }
        if let Some(rels) = document.package.get_part_rels(&target) {
            targets.extend(
                rels.items
                    .iter()
                    .filter(|rel| crate::document::relationship_is_internal(rel))
                    .map(|rel| OpcPackage::resolve_rel_target(&target, &rel.target)),
            );
        }
        candidates.insert(key, target);
    }
    let mut live = HashSet::new();
    let mut roots = Vec::new();
    for (owner, rels) in &document.package.part_rels {
        if candidates.contains_key(&owner.to_ascii_lowercase()) {
            continue;
        }
        roots.extend(
            rels.items
                .iter()
                .filter(|rel| crate::document::relationship_is_internal(rel))
                .map(|rel| OpcPackage::resolve_rel_target(owner, &rel.target)),
        );
    }
    roots.extend(
        document
            .package
            .package_rels
            .items
            .iter()
            .filter(|rel| crate::document::relationship_is_internal(rel))
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target)),
    );
    while let Some(target) = roots.pop() {
        let key = target.to_ascii_lowercase();
        if !candidates.contains_key(&key) || !live.insert(key) {
            continue;
        }
        if let Some(rels) = document.package.get_part_rels(&target) {
            roots.extend(
                rels.items
                    .iter()
                    .filter(|rel| crate::document::relationship_is_internal(rel))
                    .map(|rel| OpcPackage::resolve_rel_target(&target, &rel.target)),
            );
        }
    }
    if required_absent.is_some_and(|owner| live.contains(&owner.to_ascii_lowercase())) {
        return Err(Error::Other(
            "footnotes part is shared by a retained package relationship".into(),
        ));
    }
    for (key, target) in candidates {
        if live.contains(&key) {
            continue;
        }
        document.package.remove_part(&target);
        document.package.remove_part_rels(&target);
        document.package.content_types.remove_override(&target);
    }
    Ok(())
}

fn remove_footnotes_part(document: &mut Document, name: &str) -> Result<()> {
    if let Some(relationships) = document.package.get_part_rels_mut(&document.doc_part_name) {
        relationships.items.retain(|relationship| {
            relationship.rel_type != rel_types::FOOTNOTES
                || OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target)
                    != name
        });
    }
    remove_unshared_note_targets(document, vec![name.to_owned()], Some(name))?;
    let settings_part = document
        .package
        .get_part_rels(&document.doc_part_name)
        .into_iter()
        .flat_map(|relationships| &relationships.items)
        .find(|relationship| {
            relationship.rel_type == rel_types::SETTINGS
                && crate::document::relationship_is_internal(relationship)
        })
        .map(|relationship| {
            OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target)
        });
    if let Some(part) = settings_part {
        let source = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("missing settings part: {part}")))?;
        let tree = crate::revision::XmlTree::parse(source)?;
        let root = &tree.elements[tree.root];
        let mut removals = tree
            .elements
            .iter()
            .filter(|node| {
                root.word
                    && root.local == "settings"
                    && node.word
                    && node.local == "footnote"
                    && node.parent.is_some_and(|parent| {
                        let properties = &tree.elements[parent];
                        properties.word
                            && properties.local == "footnotePr"
                            && properties.parent == Some(tree.root)
                    })
            })
            .map(|node| node.start..node.end)
            .collect::<Vec<_>>();
        if !removals.is_empty() {
            removals.sort_by_key(|range| range.start);
            let mut updated = source.to_vec();
            for range in removals.into_iter().rev() {
                updated.drain(range);
            }
            let settings = oxoffice_docx_oxml::settings::CT_Settings::from_xml(&updated)?;
            document.package.set_part(&part, updated);
            document.settings = Some(settings);
        }
    }
    document.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::new();
    document.footnotes_part_name = None;
    document.footnotes_dirty = false;
    Ok(())
}

fn rewrite_note_marker_pair(
    document: &mut Document,
    marker: &NoteMarker,
    token: i32,
    pair_id: i32,
) -> Result<()> {
    let bytes = document
        .package
        .get_part(&marker.part)
        .expect("custody marker part")
        .to_vec();
    let tree = crate::revision::XmlTree::parse(&bytes)?;
    let name = format!(
        "_rdocxCmpNoteF{token}P{}{}{}",
        u8::from(marker.original),
        u8::from(marker.edited),
        marker.family
    );
    let id = pair_id.to_string();
    let mut edits = Vec::new();
    for (index, node) in tree
        .elements
        .iter()
        .enumerate()
        .filter(|(_, node)| node.start >= marker.span.start && node.end <= marker.span.end)
    {
        if !node.word || !matches!(node.local.as_str(), "bookmarkStart" | "bookmarkEnd") {
            continue;
        }
        let scope = tree
            .source_scope(index)
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut reader = Reader::from_reader(&bytes[node.start..node.open_end]);
        let event = reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let start = match event {
            Event::Start(start) | Event::Empty(start) => start,
            _ => unreachable!("marker opening"),
        };
        let mut updated = BytesStart::new(node.name.as_str());
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let key = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
            let local = key
                .split_once(':')
                .filter(|(prefix, _)| scope.get(*prefix).is_some_and(|uri| uri == W_NS))
                .map(|(_, local)| local);
            match local {
                Some("id") => updated.push_attribute((key, id.as_str())),
                Some("name") => updated.push_attribute((key, name.as_str())),
                _ => oxoffice_core::xml::push_raw_attribute(&mut updated, attribute),
            }
        }
        let mut writer = Writer::new(Vec::new());
        writer.write_event(if node.empty {
            Event::Empty(updated)
        } else {
            Event::Start(updated)
        })?;
        edits.push((node.start..node.open_end, writer.into_inner()));
    }
    let mut updated = bytes;
    for (range, value) in edits.into_iter().rev() {
        updated.splice(range, value);
    }
    crate::document::set_story_source_xml(document, &marker.part, updated)
}

/// Input collisions change only the positively associated bookmark identities.
/// Numeric footnote and current revision identities remain untouched.
pub(super) fn reconcile_footnote_custody_inputs(
    original: &Document,
    edited: &mut Document,
) -> Result<()> {
    let marked = |units: Vec<FootnoteCustody>| {
        units
            .into_iter()
            .filter(|unit| unit.token.is_some())
            .collect::<Vec<_>>()
    };
    let left = marked(validate_footnote_custody(
        original,
        &crate::revision::RevisionScope::All,
    )?);
    let right = marked(validate_footnote_custody(
        edited,
        &crate::revision::RevisionScope::All,
    )?);
    let left_markers = note_markers(original)?;
    if left_markers.is_empty() {
        return Ok(());
    }
    let mut occupied = footnote_custody_reserved_ids(original)?;
    occupied.extend(footnote_custody_reserved_ids(edited)?);
    for document in [original, &*edited] {
        let mut parts = vec![document.doc_part_name.clone()];
        parts.extend(
            story_parts(document)?
                .into_iter()
                .map(|story| story.part_name),
        );
        for name in parts {
            occupied.extend(word_ids(
                document.package.get_part(&name).expect("comparison story"),
            )?);
        }
    }
    let mut ids = IdAllocator::new(occupied);
    for unit in right {
        let same = if let Some(owner) = unit.owner {
            let candidates = left
                .iter()
                .filter(|other| other.owner == Some(owner))
                .collect::<Vec<_>>();
            if candidates.len() > 1 {
                return Err(Error::Other(
                    "ambiguous imported custody owner association".into(),
                ));
            }
            if note_references_have_same_owners(original, edited, owner)? {
                candidates.first().copied()
            } else {
                None
            }
        } else {
            let right_marker = note_markers(edited)?
                .into_iter()
                .find(|marker| Some(marker.token) == unit.token && marker.family == 'B');
            left.iter().find(|other| {
                other.owner.is_none()
                    && other.token == unit.token
                    && other.original == unit.original
                    && other.edited == unit.edited
                    && right_marker.as_ref().is_some_and(|right| {
                        left_markers.iter().any(|left| {
                            Some(left.token) == other.token
                                && left.family == 'B'
                                && left.part == right.part
                                && left.pair_id == right.pair_id
                                && left.span.start - left.paragraph.start
                                    == right.span.start - right.paragraph.start
                                && original
                                    .package
                                    .get_part(&left.part)
                                    .map(|source| &source[left.paragraph.clone()])
                                    == edited
                                        .package
                                        .get_part(&right.part)
                                        .map(|source| &source[right.paragraph.clone()])
                        })
                    })
            })
        };
        if let Some(other) = same {
            if unit.original != other.original || unit.edited != other.edited {
                return Err(Error::Other(
                    "paired custody owner has different transaction bits".into(),
                ));
            }
            for family in ['B', 'N'] {
                let left = left_markers
                    .iter()
                    .find(|marker| Some(marker.token) == other.token && marker.family == family);
                let right = note_markers(edited)?
                    .into_iter()
                    .find(|marker| Some(marker.token) == unit.token && marker.family == family);
                if let (Some(left), Some(right)) = (left, right) {
                    rewrite_note_marker_pair(edited, &right, left.token, left.pair_id)?;
                }
            }
        } else {
            let markers = note_markers(edited)?
                .into_iter()
                .filter(|marker| Some(marker.token) == unit.token)
                .collect::<Vec<_>>();
            let collision = left.iter().any(|other| other.token == unit.token)
                || markers.iter().any(|right| {
                    left_markers
                        .iter()
                        .any(|left| left.pair_id == right.pair_id && left.part == right.part)
                });
            if collision {
                let token = ids.allocate_note_token()?;
                for marker in markers {
                    let pair_id = ids.allocate()?;
                    rewrite_note_marker_pair(edited, &marker, token, pair_id)?;
                }
            }
        }
    }
    validate_footnote_custody(edited, &crate::revision::RevisionScope::All)?;
    Ok(())
}

pub(super) fn retire_completed_footnote_inputs(
    original: &mut Document,
    edited: &mut Document,
) -> Result<()> {
    let left = FootnotePart::of(original)?;
    let right = FootnotePart::of(edited)?;
    if left.as_ref().map(|part| &part.xml) == right.as_ref().map(|part| &part.xml) {
        return Ok(());
    }
    for document in [original, edited] {
        let units = validate_footnote_custody(document, &crate::revision::RevisionScope::All)?;
        if !units.iter().any(|unit| !unit.revisions.is_empty()) {
            retire_note_markers(document, &units, None)?;
        }
    }
    Ok(())
}

fn note_bookmark(token: i32, original: bool, edited: bool, family: char, id: i32) -> String {
    format!(
        r#"<w:bookmarkStart xmlns:w="{W_NS}" w:id="{id}" w:name="_rdocxCmpNoteF{token}P{}{}{family}"/><w:bookmarkEnd xmlns:w="{W_NS}" w:id="{id}"/>"#,
        u8::from(original),
        u8::from(edited)
    )
}

fn close_note_fragment(xml: &str, bindings: &NamespaceBindings) -> Result<String> {
    let scope = bindings
        .iter()
        .filter_map(|(name, value)| {
            if name == "xmlns" {
                Some((String::new(), value.clone()))
            } else {
                name.strip_prefix("xmlns:")
                    .map(|prefix| (prefix.to_owned(), value.clone()))
            }
        })
        .collect::<BTreeMap<_, _>>();
    String::from_utf8(crate::document::close_content_fragment_namespaces(
        xml.as_bytes(),
        &scope,
    )?)
    .map_err(utf8_error)
}

// Temporary read grammar carries the inherited context on its own wrapper.
// Retained child bytes, especially an opaque first child, remain untouched.
fn note_fragment_envelope(xml: &str, bindings: &NamespaceBindings, note: bool) -> Result<String> {
    let scoped = close_note_fragment(xml, bindings)?;
    let mut reader = NsReader::from_str(&scoped);
    let modeled_root = loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        match event {
            Event::Start(root) | Event::Empty(root) => {
                break matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == W_NS.as_bytes())
                    && matches!(root.local_name().as_ref(), b"p" | b"tbl" | b"sdt");
            }
            Event::Eof => break false,
            _ => {}
        }
    };
    // Preserve the established modeled-root namespace publication. An opaque
    // first child instead receives context from the temporary wrapper alone.
    if modeled_root {
        return Ok(if note {
            format!(
                "<w:footnotes xmlns:w=\"{W_NS}\"><w:footnote>{scoped}</w:footnote></w:footnotes>"
            )
        } else {
            format!("<w:tc xmlns:w=\"{W_NS}\">{scoped}</w:tc>")
        });
    }
    let mut suffix = 0usize;
    let prefix = loop {
        let prefix = format!("rdocxnote{suffix}");
        if !bindings
            .iter()
            .any(|(name, _)| name == &format!("xmlns:{prefix}"))
        {
            break prefix;
        }
        suffix += 1;
    };
    let name = format!("{prefix}:{}", if note { "footnotes" } else { "tc" });
    let note_name = format!("{prefix}:footnote");
    let binding = format!("xmlns:{prefix}");
    let mut root = BytesStart::new(name.as_str());
    root.push_attribute((binding.as_str(), W_NS));
    for (key, value) in bindings {
        root.push_attribute((key.as_str(), value.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Start(root))?;
    if note {
        writer.write_event(Event::Start(BytesStart::new(note_name.as_str())))?;
    }
    writer.get_mut().extend_from_slice(xml.as_bytes());
    if note {
        writer.write_event(Event::End(BytesEnd::new(note_name.as_str())))?;
    }
    writer.write_event(Event::End(BytesEnd::new(name.as_str())))?;
    String::from_utf8(writer.into_inner()).map_err(utf8_error)
}

/// The footnotes both inputs reference that would reach the pending result
/// with more physical references than either input gives them. That happens
/// when a reference moves to another site, for example a new note inserted
/// before it takes its id in the edited input: the deleted original reference
/// and the inserted edited one then name one owner, a shared reference that
/// Word for Mac 16.115 cannot open, and the owner's text change pairs two
/// different notes. Found before any note custody is written.
fn moved_pending_references(
    candidate: &Document,
    left_refs: &[FootnoteReferenceSite],
    right_refs: &[FootnoteReferenceSite],
    left_normal: &BTreeMap<i32, &FootnoteOwner>,
    right_normal: &BTreeMap<i32, &FootnoteOwner>,
) -> Result<Vec<i32>> {
    let count = |sites: &[FootnoteReferenceSite], id: i32| {
        sites.iter().filter(|site| site.id == id).count()
    };
    let common = left_normal
        .keys()
        .filter(|id| right_normal.contains_key(id))
        .filter(|&&id| count(left_refs, id) > 0 && count(right_refs, id) > 0)
        .copied()
        .collect::<Vec<_>>();
    if common.is_empty() {
        return Ok(Vec::new());
    }
    let pending = footnote_reference_sites(candidate)?;
    Ok(common
        .into_iter()
        .filter(|&id| count(&pending, id) > count(left_refs, id).max(count(right_refs, id)))
        .collect())
}

/// Word numbers footnotes by position when it saves, so one note added near
/// the top gives every later note a new id, and the comparison would pair
/// different notes by id. Before comparing, the edited notes take the ids of
/// the original's: the notes referenced in the main story are paired in
/// reference order by their text (the longest common sequence, then the
/// notes left between two pairs, by position, when both sides leave as many).
/// An unpaired edited note keeps its id, so the comparison revises in place
/// a note one version does not reference; it takes a new id only when a
/// paired note takes its id or both versions reference a note under it. A
/// pair is dropped when it would take the id of a note the edited version
/// does not reference, or move an edited note away from the id of a note the
/// original does not reference. Notes in `renumbered`, moved to new ids by an
/// earlier pass, are left out of the pairing: pairing them back would undo
/// the move and the passes would never end. Nothing changes when a note is
/// referenced twice or outside the main story, when a reference sits in a
/// tracked change, or while note custody of an earlier comparison is
/// pending. Returns each edited id it changed, mapped to the id it gave.
pub(super) fn reconcile_footnote_ids(
    original: &Document,
    edited: &mut Document,
    renumbered: &[i32],
) -> Result<HashMap<i32, i32>> {
    let (Some(left_part), Some(right_part)) =
        (FootnotePart::of(original)?, FootnotePart::of(edited)?)
    else {
        return Ok(HashMap::new());
    };
    if left_part.xml == right_part.xml
        || !note_markers(original)?.is_empty()
        || !note_markers(edited)?.is_empty()
    {
        return Ok(HashMap::new());
    }
    let references = |document: &Document| -> Result<Option<Vec<i32>>> {
        let sites = footnote_reference_sites(document)?;
        if sites
            .iter()
            .any(|site| site.part != document.doc_part_name || site.revision.is_some())
        {
            return Ok(None);
        }
        let ids = sites.iter().map(|site| site.id).collect::<Vec<_>>();
        let unique = ids.iter().collect::<HashSet<_>>();
        Ok((unique.len() == ids.len()).then_some(ids))
    };
    let (Some(left_ids), Some(right_ids)) = (references(original)?, references(edited)?) else {
        return Ok(HashMap::new());
    };
    let texts = |part: &FootnotePart| -> Result<HashMap<i32, String>> {
        let tree = crate::revision::XmlTree::parse(part.xml.as_bytes())?;
        let mut texts = HashMap::new();
        for (index, node) in tree.elements.iter().enumerate() {
            if node.word && node.local == "footnote" && node.parent == Some(tree.root) {
                let id = word_attributes(&tree, part.xml.as_bytes(), index)?
                    .get("id")
                    .and_then(|id| id.parse::<i32>().ok());
                if let Some(id) = id {
                    texts.insert(id, paragraph_text(&tree, part.xml.as_bytes(), index));
                }
            }
        }
        Ok(texts)
    };
    let (left_texts, right_texts) = (texts(&left_part)?, texts(&right_part)?);
    let left_normal = left_part.normal();
    let right_normal = right_part.normal();
    let sequence =
        |ids: &[i32], texts: &HashMap<i32, String>, normal: &BTreeMap<i32, &FootnoteOwner>| {
            ids.iter()
                .filter(|id| normal.contains_key(id))
                .map(|id| (*id, texts.get(id).cloned().unwrap_or_default()))
                .collect::<Vec<_>>()
        };
    let left = sequence(&left_ids, &left_texts, &left_normal);
    let mut right = sequence(&right_ids, &right_texts, &right_normal);
    right.retain(|(id, _)| !renumbered.contains(id));
    let text_of = |notes: &[(i32, String)]| {
        notes
            .iter()
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>()
    };
    let mut ids = HashMap::new();
    for (index, other) in strict_pairs(&text_of(&left), &text_of(&right)) {
        ids.insert(right[other].0, left[index].0);
    }
    // A note one version does not reference keeps its id and is revised in
    // place, so a pair is dropped when it would take the id of such an
    // edited note, or leave the id of such an original note to nothing.
    let unreferenced = |normal: &BTreeMap<i32, &FootnoteOwner>, referenced: &[i32]| {
        normal
            .keys()
            .filter(|id| !referenced.contains(id))
            .copied()
            .collect::<HashSet<_>>()
    };
    let (left_unreferenced, right_unreferenced) = (
        unreferenced(&left_normal, &left_ids),
        unreferenced(&right_normal, &right_ids),
    );
    ids.retain(|from, to| !right_unreferenced.contains(to) && !left_unreferenced.contains(from));
    let mut next = left_normal
        .keys()
        .chain(right_normal.keys())
        .copied()
        .max()
        .unwrap_or(0);
    // A note both versions reference under the same id without pairing, or
    // whose id a paired note takes, moves to a new id. A note one version
    // does not reference keeps its id: the comparison revises it in place.
    let targets = ids.values().copied().collect::<HashSet<_>>();
    let referenced =
        |notes: &[(i32, String)]| notes.iter().map(|(id, _)| *id).collect::<HashSet<_>>();
    let (left_referenced, right_referenced) = (referenced(&left), referenced(&right));
    for &id in right_normal.keys() {
        if !ids.contains_key(&id)
            && (targets.contains(&id)
                || (right_referenced.contains(&id) && left_referenced.contains(&id)))
        {
            next = next
                .checked_add(1)
                .ok_or_else(|| Error::Other("comparison footnote ids are exhausted".into()))?;
            ids.insert(id, next);
        }
    }
    ids.retain(|from, to| from != to);
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let notes = rewrite_word_ids(right_part.xml.as_bytes(), "footnote", &ids)?;
    crate::document::set_story_source_xml(edited, &right_part.name, notes)?;
    let main = edited.doc_part_name.clone();
    let body = rewrite_word_ids(
        edited
            .package
            .get_part(&main)
            .ok_or_else(|| Error::Other("comparison input has no main part".into()))?,
        "footnoteReference",
        &ids,
    )?;
    crate::document::set_story_source_xml(edited, &main, body)?;
    Ok(ids)
}

/// The `w:t` text under the element `index`.
fn paragraph_text(tree: &crate::revision::XmlTree<'_>, xml: &[u8], index: usize) -> String {
    let end = tree.elements[index].end;
    tree.elements[index + 1..]
        .iter()
        .take_while(|child| child.start < end)
        .filter(|child| child.word && child.local == "t" && !child.empty)
        .map(|child| String::from_utf8_lossy(&xml[child.open_end..child.close_start]))
        .collect()
}

/// The pairs [`align`] makes, less those it makes by position in a gap where
/// one side leaves more items than the other.
fn strict_pairs(left: &[String], right: &[String]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut gap = Vec::new();
    let mut uneven = false;
    for entry in align(left, right) {
        match entry {
            (Some(index), Some(other)) if left[index] == right[other] => {
                if !uneven {
                    pairs.append(&mut gap);
                }
                gap.clear();
                uneven = false;
                pairs.push((index, other));
            }
            (Some(index), Some(other)) => gap.push((index, other)),
            _ => uneven = true,
        }
    }
    if !uneven {
        pairs.append(&mut gap);
    }
    pairs
}

/// The footnote and endnote references of a document's stories, counted by
/// element name and note id. The notes parts themselves are left out.
fn note_reference_counts(document: &Document) -> Result<HashMap<(String, i32), usize>> {
    let mut parts = vec![document.doc_part_name.clone()];
    parts.extend(
        story_parts(document)?
            .into_iter()
            .filter(|story| {
                !matches!(
                    story.kind,
                    ComparisonStoryKind::Footnote | ComparisonStoryKind::Endnote
                )
            })
            .map(|story| story.part_name),
    );
    parts.sort();
    parts.dedup();
    let mut counts = HashMap::new();
    for part in parts {
        let xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other(format!("missing reference story {part}")))?;
        let mut reader = NsReader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
            match event {
                Event::Start(element) | Event::Empty(element)
                    if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                        && matches!(
                            element.local_name().as_ref(),
                            b"footnoteReference" | b"endnoteReference"
                        ) =>
                {
                    let local = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        if name.as_ref() == b"id"
                            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                            && let Ok(id) = std::str::from_utf8(&attribute.value)
                                .unwrap_or_default()
                                .parse::<i32>()
                        {
                            *counts.entry((local.clone(), id)).or_insert(0) += 1;
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(counts)
}

/// No note may reach the result with more references than either input gives
/// it. A reference that moved, or the content holding it written as a
/// deletion and an insertion, leaves a deleted and an inserted reference
/// naming one note, which Word for Mac cannot open. Moved footnotes became
/// new notes before this point. Endnotes, and footnotes whose story is
/// ignored, have no such path: the pair is refused. Nor may the result name
/// an endnote the original does not hold while endnotes are ignored, since
/// the result keeps the original's endnotes.
pub(super) fn refuse_shared_note_references(
    original: &Document,
    edited: &Document,
    candidate: &Document,
    endnotes_ignored: bool,
) -> Result<()> {
    let left = note_reference_counts(original)?;
    let right = note_reference_counts(edited)?;
    // Read only when the result references an endnote.
    let mut held_endnotes = None;
    let mut pending = note_reference_counts(candidate)?
        .into_iter()
        .collect::<Vec<_>>();
    pending.sort();
    for ((element, id), count) in pending {
        let key = (element, id);
        if endnotes_ignored && key.0 == "endnoteReference" {
            if held_endnotes.is_none() {
                held_endnotes = Some(original.normal_endnote_ids()?);
            }
            if held_endnotes
                .as_ref()
                .is_some_and(|held: &HashSet<i32>| !held.contains(&id))
            {
                return Err(Error::Other(format!(
                    "ignored endnotes cannot supply the edited reference to endnote {id}"
                )));
            }
        }
        let (Some(&left), Some(&right)) = (left.get(&key), right.get(&key)) else {
            continue;
        };
        let most = left.max(right);
        if count > most {
            let note = if key.0 == "endnoteReference" {
                "endnote"
            } else {
                "footnote"
            };
            return Err(Error::Other(format!(
                "comparison would give {note} {id} {count} references where each input has at most {most}: its reference moved to another site, or what holds it was deleted and inserted again"
            )));
        }
    }
    Ok(())
}

/// A copy of the edited input with each of these footnotes under a new id,
/// in its footnotes part and at its references, and the new ids; or None
/// when that cannot be done faithfully: a reference outside the main story,
/// or the pending note custody of an earlier comparison, which keeps its
/// notes' ids.
pub(super) fn renumbered_footnotes(
    original: &Document,
    edited: &Document,
    moved: &[i32],
) -> Result<Option<(Document, Vec<i32>)>> {
    let mut renumbered = comparison_input(edited)?;
    // Completed custody is retired as the first pass retired it.
    let units = validate_footnote_custody(&renumbered, &crate::revision::RevisionScope::All)?;
    if !units.iter().any(|unit| !unit.revisions.is_empty()) {
        retire_note_markers(&mut renumbered, &units, None)?;
    }
    let Some(part) = FootnotePart::of(&renumbered)? else {
        return Ok(None);
    };
    if !note_markers(&renumbered)?.is_empty()
        || footnote_reference_sites(&renumbered)?
            .iter()
            .any(|site| moved.contains(&site.id) && site.part != renumbered.doc_part_name)
    {
        return Ok(None);
    }
    let normal = part.normal();
    let moved = moved
        .iter()
        .copied()
        .filter(|id| normal.contains_key(id))
        .collect::<Vec<_>>();
    if moved.is_empty() {
        return Ok(None);
    }
    let mut next = FootnotePart::of(original)?
        .iter()
        .chain([&part])
        .flat_map(|part| part.owners.iter().filter_map(|owner| owner.id))
        .max()
        .unwrap_or(0);
    let mut ids = HashMap::new();
    for id in moved {
        next = next
            .checked_add(1)
            .ok_or_else(|| Error::Other("comparison footnote ids are exhausted".into()))?;
        ids.insert(id, next);
    }
    let notes = rewrite_word_ids(part.xml.as_bytes(), "footnote", &ids)?;
    crate::document::set_story_source_xml(&mut renumbered, &part.name, notes)?;
    let main = renumbered.doc_part_name.clone();
    let body = rewrite_word_ids(
        renumbered
            .package
            .get_part(&main)
            .ok_or_else(|| Error::Other("comparison input has no main part".into()))?,
        "footnoteReference",
        &ids,
    )?;
    crate::document::set_story_source_xml(&mut renumbered, &main, body)?;
    Ok(Some((renumbered, ids.into_values().collect())))
}

/// `xml` with the `w:id` of every Word element of this local name mapped.
fn rewrite_word_ids(xml: &[u8], local: &str, ids: &HashMap<i32, i32>) -> Result<Vec<u8>> {
    let tree = crate::revision::XmlTree::parse(xml)?;
    let mut edits = Vec::new();
    for (index, node) in tree.elements.iter().enumerate() {
        if !node.word || node.local != local {
            continue;
        }
        let updated = rewritten_opening(&tree, xml, index, |name, value| {
            let id = std::str::from_utf8(value).ok()?.parse::<i32>().ok()?;
            (name == "id")
                .then(|| ids.get(&id))
                .flatten()
                .map(|id| id.to_string().into_bytes())
        })?;
        if let Some(updated) = updated {
            edits.push((node.start..node.open_end, updated));
        }
    }
    let mut updated = xml.to_vec();
    for (range, value) in edits.into_iter().rev() {
        updated.splice(range, value);
    }
    Ok(updated)
}

pub(super) fn compare_footnote_parts(
    original: &Document,
    edited: &Document,
    candidate: &mut Document,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<Vec<i32>> {
    if story_ignored(metadata.options, ComparisonStoryKind::Footnote) {
        return Ok(Vec::new());
    }
    let left = FootnotePart::of(original)?;
    let right = FootnotePart::of(edited)?;
    if left.is_none() && right.is_none() {
        return Ok(Vec::new());
    }
    // Generated Word markup must carry its own binding beside retained aliases
    // and producer-local shadows in the related part.
    metadata.ids.local_word_binding = true;
    if let (Some(left), Some(right)) = (&left, &right)
        && (left.name != right.name
            || left.retained_shell()? != right.retained_shell()?
            || left.retained_records()? != right.retained_records()?)
    {
        return Err(Error::Other(
            "footnotes retained root/special/opaque shell changed".into(),
        ));
    }

    let empty = BTreeMap::new();
    let left_normal = left
        .as_ref()
        .map(FootnotePart::normal)
        .unwrap_or_else(|| empty.clone());
    let right_normal = right
        .as_ref()
        .map(FootnotePart::normal)
        .unwrap_or_else(|| empty.clone());
    let left_refs = footnote_reference_sites(original)?;
    let right_refs = footnote_reference_sites(edited)?;
    let moved = moved_pending_references(
        candidate,
        &left_refs,
        &right_refs,
        &left_normal,
        &right_normal,
    )?;
    if !moved.is_empty() {
        return Ok(moved);
    }
    let added = right
        .as_ref()
        .map(|part| {
            part.owners
                .iter()
                .filter_map(|owner| {
                    owner
                        .id
                        .filter(|id| right_normal.contains_key(id) && !left_normal.contains_key(id))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let removed = left
        .as_ref()
        .map(|part| {
            part.owners
                .iter()
                .filter_map(|owner| {
                    owner
                        .id
                        .filter(|id| left_normal.contains_key(id) && !right_normal.contains_key(id))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for id in &added {
        if !right_refs.iter().any(|site| site.id == *id) {
            return Err(Error::Other(format!(
                "new unused footnote {id} cannot be compared"
            )));
        }
    }
    for id in &removed {
        if !left_refs.iter().any(|site| site.id == *id) {
            return Err(Error::Other(format!(
                "comparison cannot remove original unused footnote {id}"
            )));
        }
    }
    if (left.is_none() || right.is_none()) && added.is_empty() && removed.is_empty() {
        return Err(Error::Other(
            "footnote part presence changed without a referenced normal owner".into(),
        ));
    }

    let mut part = left
        .as_ref()
        .or(right.as_ref())
        .expect("one part exists")
        .clone();
    let mut replacements = Vec::new();
    let mut insertions = BTreeMap::<usize, String>::new();
    let common = left_normal
        .keys()
        .filter(|id| right_normal.contains_key(id))
        .copied()
        .collect::<HashSet<_>>();
    let anchors = |part: &FootnotePart| -> Result<Vec<(FootnoteRootAnchor, Range<usize>)>> {
        let mut retained = 0usize;
        Ok(part
            .root_items()?
            .into_iter()
            .filter_map(|(id, range)| {
                if let Some(id) = id {
                    common
                        .contains(&id)
                        .then_some((FootnoteRootAnchor::Normal(id), range))
                } else {
                    let key = FootnoteRootAnchor::Retained(retained);
                    retained += 1;
                    Some((key, range))
                }
            })
            .collect())
    };
    let left_anchors = left.as_ref().map(anchors).transpose()?.unwrap_or_default();
    let right_anchors = right.as_ref().map(anchors).transpose()?.unwrap_or_default();
    if left.is_some()
        && right.is_some()
        && left_anchors.iter().map(|(key, _)| key).collect::<Vec<_>>()
            != right_anchors.iter().map(|(key, _)| key).collect::<Vec<_>>()
    {
        return Err(Error::Other("footnote retained owner order changed".into()));
    }
    let mut emitted = false;
    for (&id, left_owner) in &left_normal {
        if let Some(right_owner) = right_normal.get(&id) {
            let l = left.as_ref().expect("left owner");
            let r = right.as_ref().expect("right owner");
            let _classes = RelationshipClassScope::enter(relationship_classes(original, &l.name)?);
            let inner = compare_story_inner(
                &l.xml[left_owner.inner.clone()],
                &r.xml[right_owner.inner.clone()],
                &format!(
                    "{}:{}/footnote-id[{id}]",
                    ComparisonStoryKind::Footnote.label(),
                    l.name
                ),
                "w",
                &left_owner.bindings,
                &right_owner.bindings,
                metadata,
                diagnostics,
            )?;
            let mut owner = l.xml[left_owner.span.clone()].to_owned();
            let relative = left_owner.inner.start - left_owner.span.start
                ..left_owner.inner.end - left_owner.span.start;
            owner.replace_range(relative, &inner);
            replacements.push((left_owner.span.clone(), owner));
        }
    }
    for (ids, addition) in [(&removed, false), (&added, true)] {
        for &id in ids {
            let (source, owner) = if addition {
                (right.as_ref().expect("added owner"), right_normal[&id])
            } else {
                (left.as_ref().expect("removed owner"), left_normal[&id])
            };
            let token = metadata.ids.allocate_note_token()?;
            let body_id = metadata.ids.allocate()?;
            let note_id = metadata.ids.allocate()?;
            let inner = &source.xml[owner.inner.clone()];
            let envelope = note_fragment_envelope(inner, &owner.bindings, false)?;
            let marked =
                marked_cell_xml(&envelope, if addition { "ins" } else { "del" }, metadata)?;
            let mut tracked = marked[element_inner_range_any_prefix(&marked, "tc")?].to_owned();
            tracked = close_note_revision_headers(&tracked, &owner.bindings)?;
            let note_mark = note_bookmark(token, left.is_some(), right.is_some(), 'N', note_id);
            tracked = insert_note_marker(&tracked, &owner.bindings, &note_mark, addition)?;
            let mut owner_xml = source.xml[owner.span.clone()].to_owned();
            owner_xml.replace_range(
                owner.inner.start - owner.span.start..owner.inner.end - owner.span.start,
                &tracked,
            );
            owner_xml = close_note_fragment(&owner_xml, &owner.bindings)?;
            if addition && left.is_some() {
                let next = right_anchors
                    .iter()
                    .find(|(_, range)| range.start >= owner.span.end);
                let at = if let Some((key, _)) = next {
                    left_anchors
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .expect("matched retained anchor")
                        .1
                        .start
                } else {
                    root_inner_range(&part.xml, "footnotes")?.end
                };
                insertions.entry(at).or_default().push_str(&owner_xml);
            } else {
                replacements.push((owner.span.clone(), owner_xml));
            }
            let body_mark = note_bookmark(token, left.is_some(), right.is_some(), 'B', body_id);
            bind_note_reference_revisions(candidate, id, addition, &body_mark, metadata)?;
            emitted = true;
        }
    }
    replacements.extend(insertions.into_iter().map(|(at, value)| (at..at, value)));
    replacements.sort_by_key(|(range, _)| (range.start, range.end));
    for (range, value) in replacements.into_iter().rev() {
        part.xml.replace_range(range, &value);
    }
    if left.is_none() {
        let name = candidate.reserve_document_part_bundle(
            None,
            &part.name,
            rel_types::FOOTNOTES,
            FOOTNOTE_CONTENT_TYPE,
        )?;
        if name != part.name {
            return Err(Error::Other(
                "created footnotes part name changed destination identity".into(),
            ));
        }
        candidate.footnotes_part_name = Some(name);
    }
    crate::document::set_story_source_xml(candidate, &part.name, part.xml.into_bytes())?;
    if emitted {
        validate_footnote_custody(candidate, &crate::revision::RevisionScope::All)?;
    }
    Ok(Vec::new())
}

fn close_note_revision_headers(xml: &str, bindings: &NamespaceBindings) -> Result<String> {
    let wrapped = note_fragment_envelope(xml, bindings, true)?;
    let mut tree = crate::revision::XmlTree::parse(wrapped.as_bytes())?;
    tree.qualify_note_revisions()?;
    let root = &tree.elements[tree.elements[tree.root].children[0]];
    let mut edits = Vec::new();
    for (index, node) in tree.elements.iter().enumerate() {
        if let Some(revision) = &node.revision {
            edits.push((
                node.start..node.open_end,
                note_revision_opening(&wrapped, &tree, index, revision.id)?,
            ));
        }
    }
    let mut output = wrapped[root.open_end..root.close_start].to_owned();
    for (range, value) in edits.into_iter().rev() {
        output.replace_range(
            range.start - root.open_end..range.end - root.open_end,
            &value,
        );
    }
    Ok(output)
}

fn note_revision_opening(
    xml: &str,
    tree: &crate::revision::XmlTree,
    index: usize,
    id: i32,
) -> Result<String> {
    let node = &tree.elements[index];
    let mut reader = Reader::from_str(&xml[node.start..node.open_end]);
    let event = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let start = match event {
        Event::Start(start) | Event::Empty(start) => start,
        _ => return Err(Error::Other("revision header has no opening".into())),
    };
    let mut out = BytesStart::new(node.name.clone());
    let mut chain = Vec::new();
    let mut parent = Some(index);
    while let Some(at) = parent {
        chain.push(at);
        parent = tree.elements[at].parent;
    }
    let mut scope = BTreeMap::new();
    for at in chain.into_iter().rev() {
        scope.extend(tree.elements[at].namespace_declarations.iter().cloned());
    }
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let key = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        if key == "xmlns:w"
            || key.split_once(':').is_some_and(|(prefix, local)| {
                local == "id" && scope.get(prefix).is_some_and(|uri| uri == W_NS)
            })
        {
            continue;
        }
        oxoffice_core::xml::push_raw_attribute(&mut out, attribute);
    }
    out.push_attribute(("w:id", id.to_string().as_str()));
    out.push_attribute(("xmlns:w", W_NS));
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(if node.empty {
            Event::Empty(out)
        } else {
            Event::Start(out)
        })
        .map_err(|error| Error::Other(error.to_string()))?;
    String::from_utf8(writer.into_inner()).map_err(utf8_error)
}

fn insert_note_marker(
    xml: &str,
    bindings: &NamespaceBindings,
    marker: &str,
    addition: bool,
) -> Result<String> {
    let wrapped = note_fragment_envelope(xml, bindings, true)?;
    let mut tree = crate::revision::XmlTree::parse(wrapped.as_bytes())?;
    tree.qualify_note_revisions()?;
    let root = &tree.elements[tree.elements[tree.root].children[0]];
    let paragraphs =
        crate::document::admitted_story_paragraph_spans(wrapped.as_bytes(), StoryKind::Footnote)?;
    let first = paragraphs
        .first()
        .ok_or_else(|| Error::Other("note custody requires its first note paragraph".into()))?;
    let paragraph = tree
        .elements
        .iter()
        .position(|node| node.word && node.local == "p" && (node.start..node.end) == *first)
        .ok_or_else(|| Error::Other("admitted note paragraph has no physical node".into()))?;
    let p = &tree.elements[paragraph];
    let mut at = p.open_end;
    let mut insertion = false;
    for &child in &p.children {
        let node = &tree.elements[child];
        if node.word && node.local == "pPr" {
            at = node.end;
            continue;
        }
        let wrapper = if addition { "ins" } else { "del" };
        if node.word && node.local == wrapper {
            // Word 16.115 drops a zero-width pair that precedes the note's
            // reference mark, and keeps one that follows it. Place the pair
            // after the first reference-mark run of the content wrapper.
            let mark = node.children.iter().copied().find(|&run| {
                let run_node = &tree.elements[run];
                run_node.word
                    && run_node.local == "r"
                    && run_node.children.iter().any(|&leaf| {
                        tree.elements[leaf].word && tree.elements[leaf].local == "footnoteRef"
                    })
            });
            if let Some(run) = mark {
                at = tree.elements[run].end;
                insertion = addition;
            } else if addition {
                at = node.open_end;
                insertion = true;
            }
        }
        break;
    }
    let mut owned_marker = marker.to_owned();
    if addition && !insertion {
        let mark = tree
            .elements
            .iter()
            .find(|node| {
                node.start >= p.open_end
                    && node.end <= p.close_start
                    && node.revision.as_ref().is_some_and(|revision| {
                        revision.kind == crate::revision::RevisionKind::Insertion
                    })
            })
            .ok_or_else(|| Error::Other("added note has no paragraph-mark insertion".into()))?;
        let mut reader = Reader::from_str(&wrapped[mark.start..mark.open_end]);
        let start = match reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        {
            Event::Empty(start) | Event::Start(start) => start,
            _ => unreachable!("revision opening"),
        };
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(start.to_owned()))?;
        writer.get_mut().extend_from_slice(marker.as_bytes());
        writer.write_event(Event::End(BytesEnd::new(mark.name.as_str())))?;
        owned_marker = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    }
    let mut result = wrapped[root.open_end..root.close_start].to_owned();
    result.insert_str(at - root.open_end, &owned_marker);
    Ok(result)
}

fn bind_note_reference_revisions(
    document: &mut Document,
    id: i32,
    addition: bool,
    marker: &str,
    metadata: &mut Metadata<'_>,
) -> Result<()> {
    let sites = footnote_reference_sites(document)?
        .into_iter()
        .filter(|site| site.id == id)
        .collect::<Vec<_>>();
    if sites.is_empty() {
        return Err(Error::Other(format!(
            "missing tracked physical reference for footnote {id}"
        )));
    }
    let mut parts = BTreeMap::<String, Vec<FootnoteReferenceSite>>::new();
    for site in sites {
        parts.entry(site.part.clone()).or_default().push(site);
    }
    let mut first = true;
    for (part, sites) in parts {
        let source = std::str::from_utf8(document.package.get_part(&part).expect("reference part"))
            .map_err(utf8_error)?
            .to_owned();
        let tree = crate::revision::XmlTree::parse(source.as_bytes())?;
        let mut edits = BTreeMap::<usize, (Range<usize>, String)>::new();
        let mut grouped = BTreeMap::<usize, (Range<usize>, Vec<Range<usize>>)>::new();
        for site in sites {
            let revision = site
                .revision
                .clone()
                .ok_or_else(|| Error::Other(format!("new note {id} reference is not tracked")))?;
            let node = tree
                .elements
                .iter()
                .find(|node| node.start == revision.start && node.end == revision.end)
                .expect("reference revision");
            let expected = if addition { "ins" } else { "del" };
            if node.local != expected
                || node
                    .parent
                    .is_none_or(|parent| tree.elements[parent].start != site.paragraph.start)
            {
                return Err(Error::Other(format!(
                    "footnote {id} has an ambiguous reference revision owner"
                )));
            }
            grouped
                .entry(node.start)
                .or_insert_with(|| (revision, Vec::new()))
                .1
                .push(site.run);
        }
        for (at, (revision, mut runs)) in grouped {
            let node_index = tree
                .elements
                .iter()
                .position(|node| node.start == at)
                .expect("grouped revision");
            let node = &tree.elements[node_index];
            let expected = if addition { "ins" } else { "del" };
            runs.sort_by_key(|range| range.start);
            runs.dedup();
            let old_open = &source[node.start..node.open_end];
            let close = &source[node.close_start..node.end];
            let mut replacement = String::new();
            let mut cursor = node.open_end;
            let mut retained_old_open = false;
            for run in runs {
                if cursor < run.start {
                    if retained_old_open {
                        let fresh = metadata.ids.allocate()?;
                        replacement
                            .push_str(&note_revision_opening(&source, &tree, node_index, fresh)?);
                    } else {
                        replacement.push_str(old_open);
                        retained_old_open = true;
                    }
                    replacement.push_str(&source[cursor..run.start]);
                    replacement.push_str(close);
                }
                if first {
                    replacement.push_str(marker);
                    first = false;
                }
                let reference_id = metadata.ids.allocate()?;
                replacement.push_str(&format!("<w:{expected} xmlns:w=\"{W_NS}\" w:id=\"{reference_id}\" w:author=\"{}\" w:date=\"{}\">",oxoffice_core::xml::escape_attribute_value(metadata.author),oxoffice_core::xml::escape_attribute_value(metadata.timestamp)));
                replacement.push_str(&source[run.clone()]);
                replacement.push_str(&format!("</w:{expected}>"));
                cursor = run.end;
            }
            if cursor < node.close_start {
                if retained_old_open {
                    let fresh = metadata.ids.allocate()?;
                    replacement
                        .push_str(&note_revision_opening(&source, &tree, node_index, fresh)?);
                } else {
                    replacement.push_str(old_open);
                }
                replacement.push_str(&source[cursor..node.close_start]);
                replacement.push_str(close);
            }
            edits.insert(at, (revision, replacement));
        }
        let mut output = source;
        for (_, (range, value)) in edits.into_iter().rev() {
            output.replace_range(range, &value);
        }
        crate::document::set_story_source_xml(document, &part, output.into_bytes())?;
    }
    Ok(())
}

/// Check the footnote part of `actual`, a [`resolved_candidate`], against the
/// input that resolution must reproduce.
pub(super) fn verify_footnote_destination(
    actual: &Document,
    wanted: &Document,
    accept: bool,
    options: &ComparisonOptions,
) -> Result<()> {
    if story_ignored(options, ComparisonStoryKind::Footnote) {
        return Ok(());
    }
    let left = FootnotePart::of(actual)?;
    let right = FootnotePart::of(wanted)?;
    match (&left, &right) {
        (None, None) => Ok(()),
        (Some(left), Some(right)) if left.name == right.name => {
            let (left_root, left_owners) = canonical_owned_story(&left.xml, "footnote")?;
            let (right_root, right_owners) = canonical_owned_story(&right.xml, "footnote")?;
            // The grouped package guard proves each normal body, including
            // its source residuals and established empty-property equivalence.
            // Root owner tokens independently retain exact note identities/types.
            if left_root != right_root
                || left_owners
                    .iter()
                    .map(|owner| owner.first())
                    .collect::<Vec<_>>()
                    != right_owners
                        .iter()
                        .map(|owner| owner.first())
                        .collect::<Vec<_>>()
                || left.retained_records()? != right.retained_records()?
            {
                return Err(Error::Other(format!(
                    "comparison {} does not reproduce exact footnote destination",
                    if accept { "acceptance" } else { "rejection" }
                )));
            }
            Ok(())
        }
        _ => Err(Error::Other(
            "comparison does not reproduce footnote destination part identity/presence".into(),
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawNoteType {
    Omitted,
    Empty,
    ExplicitNormal,
    Separator,
    ContinuationSeparator,
    ContinuationNotice,
    Unknown,
}

pub(crate) fn raw_note_attribute_in_scope(
    xml: &str,
    inherited: &[(String, String)],
    wanted: &str,
) -> Result<Option<String>> {
    let mut reader = Reader::from_str(xml);
    let event = reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(element) | Event::Empty(element)) = event else {
        return Err(Error::Other("comparison note has no owner element".into()));
    };
    let local = std::str::from_utf8(element.local_name().as_ref())
        .map_err(utf8_error)?
        .to_owned();
    let bindings = root_namespace_bindings(xml, &local, inherited)?;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let key = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
        let Some((prefix, local)) = key.split_once(':') else {
            continue;
        };
        if local != wanted
            || !bindings
                .iter()
                .any(|(name, uri)| name == &format!("xmlns:{prefix}") && uri == W_NS)
        {
            continue;
        }
        return attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map(|value| Some(value.into_owned()))
            .map_err(|error| Error::Other(error.to_string()));
    }
    Ok(None)
}

pub(crate) fn raw_note_type_in_scope(
    xml: &str,
    inherited: &[(String, String)],
) -> Result<RawNoteType> {
    Ok(
        match raw_note_attribute_in_scope(xml, inherited, "type")?.as_deref() {
            None => RawNoteType::Omitted,
            Some("") => RawNoteType::Empty,
            Some("normal") => RawNoteType::ExplicitNormal,
            Some("separator") => RawNoteType::Separator,
            Some("continuationSeparator") => RawNoteType::ContinuationSeparator,
            Some("continuationNotice") => RawNoteType::ContinuationNotice,
            Some(_) => RawNoteType::Unknown,
        },
    )
}

pub(super) fn normal_note_owner(xml: &str, inherited: &[(String, String)]) -> Result<bool> {
    Ok(matches!(
        raw_note_type_in_scope(xml, inherited)?,
        RawNoteType::Omitted | RawNoteType::ExplicitNormal
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revision_opening_keeps_a_single_quoted_attribute_well_formed() {
        let xml = concat!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
            r#"xmlns:x="urn:example"><w:ins w:id="4" w:author="A" x:note='a"b'></w:ins>"#,
            r#"</w:document>"#,
        );
        let tree = crate::revision::XmlTree::parse(xml.as_bytes()).unwrap();
        let index = tree
            .elements
            .iter()
            .position(|element| element.local == "ins")
            .unwrap();
        let opening = note_revision_opening(xml, &tree, index, 9).unwrap();
        assert!(opening.contains(r#"x:note="a&quot;b""#), "{opening}");
        assert!(opening.contains(r#"w:id="9""#), "{opening}");
    }
}
