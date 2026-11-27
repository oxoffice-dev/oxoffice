//! The sources of every story and the main story layout.

use super::*;

#[cfg(debug_assertions)]
fn assert_main_story_layout(xml: &[u8], layout: &MainStoryLayout, items: bool) {
    let whole = scan_story_owners(xml, StoryKind::Body).expect("the main part scans whole");
    let owners = layout
        .owners(xml)
        .expect("the main story layout gives its owners");
    let key = |owner: &StoryOwnerSpan| {
        (
            owner.kind,
            owner.owner_index,
            owner.full.clone(),
            owner.fingerprint,
        )
    };
    assert_eq!(
        owners.iter().map(key).collect::<Vec<_>>(),
        whole.iter().map(key).collect::<Vec<_>>()
    );
    if !items {
        return;
    }
    let body = whole
        .iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .expect("body");
    let whole_items = scan_story_items(xml, body, None).expect("body items");
    let item_key = |item: &StoryItemSpan| (item.kind, item.full.clone(), item.direct_owner_child);
    assert_eq!(
        layout.body_items().iter().map(item_key).collect::<Vec<_>>(),
        whole_items.iter().map(item_key).collect::<Vec<_>>()
    );
}

impl Document {
    pub(crate) fn generated_table_paragraph_positions(
        &self,
    ) -> Result<Vec<(ContentLocation, std::ops::Range<usize>)>> {
        let mut result = Vec::new();
        for story in self.stories()? {
            if story.kind == StoryKind::Comment {
                continue;
            }
            let (source, _) = self.story_source_and_owner(&story)?;
            let xml = self
                .package
                .get_part(&source.part_name)
                .unwrap_or(source.xml.as_ref());
            let owner = scan_story_owners(xml, source.root_kind)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
                .ok_or_else(|| {
                    Error::Other(
                        "generated paragraph owner disappeared from retained source".into(),
                    )
                })?;
            for (index, item) in scan_story_items(xml, &owner, None)?.iter().enumerate() {
                let spans = match item.kind {
                    StoryItemKind::Paragraph => vec![(vec![index], item.full.clone())],
                    StoryItemKind::ContentControl => scan_story_control_paragraphs(xml, item)?
                        .into_iter()
                        .enumerate()
                        .map(|(paragraph, span)| (vec![index, paragraph], span))
                        .collect(),
                    _ => continue,
                };
                for (path, range) in spans {
                    let location =
                        ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path);
                    self.story_range_paragraph_source(&location)?;
                    result.push((location, range));
                }
            }
        }
        result.sort_by_key(|(location, range)| {
            (location.story().part_name().to_owned(), range.start)
        });
        Ok(result)
    }

    /// Exact physical story-owner content, from the existing checked inventory.
    pub(crate) fn generated_table_story_sources(&self) -> Result<Vec<GeneratedStoryOwner>> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let xml = if source.part_name == self.doc_part_name {
                self.package
                    .get_part(&source.part_name)
                    .unwrap_or(source.xml.as_ref())
            } else {
                source.xml.as_ref()
            };
            for owner in scan_story_owners(xml, source.root_kind)? {
                if matches!(owner.kind, StoryKind::Comment | StoryKind::TableCell)
                    || !seen.insert((owner.kind, source.part_name.clone(), owner.owner_index))
                {
                    continue;
                }
                let mut reader =
                    oxoffice_core::xml_reader::Reader::from_reader(&xml[owner.full.clone()]);
                let mut buffer = Vec::new();
                let start = match reader.read_event_into(&mut buffer).map_err(|error| {
                    Error::Other(format!("story owner start scan failed: {error}"))
                })? {
                    Event::Start(_) => owner.full.start + reader.buffer_position() as usize,
                    Event::Empty(_) => continue,
                    _ => return Err(Error::Other("story owner has no start element".into())),
                };
                let end = story_owner_content_end(xml, &owner)?;
                let scope = story_namespace_scope_at(xml, start)?;
                result.push(GeneratedStoryOwner {
                    story: StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    },
                    range: start..end,
                    xml: xml.to_vec(),
                    namespaces: scope,
                });
            }
        }
        Ok(result)
    }

    pub(super) fn story_sources(&self) -> Result<Vec<StorySource<'_>>> {
        Ok(self.spanned_story_sources()?.0)
    }

    /// The identifiers of every bookmark end of the story sources.
    pub(crate) fn story_bookmark_end_ids(&self) -> Result<std::collections::HashSet<i32>> {
        let mut ids = std::collections::HashSet::new();
        for source in self.story_sources()? {
            ids.extend(super::source_bookmark_end_ids(&source.xml)?);
        }
        Ok(ids)
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn spanned_story_sources(
        &self,
    ) -> Result<(Vec<StorySource<'_>>, Range<usize>, Vec<Range<usize>>)> {
        let (main, content, children) = self.main_part_with_body_spans()?;
        let mut sources = vec![StorySource {
            root_kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            xml: Cow::Owned(main),
        }];
        sources.extend(self.related_story_sources()?);
        Ok((sources, content, children))
    }

    fn story_sources_and_main_layout(
        &self,
        items: bool,
    ) -> Result<(Vec<StorySource<'_>>, Option<MainStoryLayout>)> {
        let (sources, content, children) = self.spanned_story_sources()?;
        let layout = self
            .main_story_layout(&sources[0].xml, &content, &children, items)
            .ok();
        Ok((sources, layout))
    }

    /// The story sources after the main part, in [`Self::story_sources`]
    /// order, read from the package without serializing the main part.
    pub(super) fn related_story_sources(&self) -> Result<Vec<StorySource<'_>>> {
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        seen.insert((StoryKind::Body, self.doc_part_name.clone()));

        for (relationship_id, is_header) in self.header_footer_rel_ids() {
            let kind = if is_header {
                StoryKind::Header
            } else {
                StoryKind::Footer
            };
            let part_name = self
                .header_footer_part_name(&relationship_id, is_header)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "{kind:?} reference {relationship_id} has no valid internal relationship"
                    ))
                })?;
            if seen.insert((kind, part_name.clone())) {
                let xml = self.package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("{kind:?} story targets missing part {part_name}"))
                })?;
                sources.push(StorySource {
                    root_kind: kind,
                    part_name,
                    xml: Cow::Borrowed(xml),
                });
            }
        }

        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            for relationship in &relationships.items {
                let kind = match relationship.rel_type.as_str() {
                    rel_types::FOOTNOTES => StoryKind::Footnote,
                    rel_types::ENDNOTES => StoryKind::Endnote,
                    rel_types::COMMENTS => StoryKind::Comment,
                    _ => continue,
                };
                if !relationship_is_internal(relationship) {
                    continue;
                }
                let part_name =
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                if !seen.insert((kind, part_name.clone())) {
                    return Err(Error::Other(format!(
                        "{kind:?} story part {part_name} is referenced more than once"
                    )));
                }
                let part_xml = self.package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("{kind:?} story targets missing part {part_name}"))
                })?;
                let xml = if kind == StoryKind::Comment {
                    Cow::Owned(part_xml.to_vec())
                } else {
                    Cow::Borrowed(part_xml)
                };
                sources.push(StorySource {
                    root_kind: kind,
                    part_name,
                    xml,
                });
            }
        }
        Ok(sources)
    }

    /// Discover every supported Word story owner in stable document and
    /// relationship order.
    pub fn stories(&self) -> Result<Vec<StoryId>> {
        let (sources, main) = self.story_sources_and_main_layout(false)?;
        Self::stories_in_sources(sources, main)
    }

    /// The stories of [`Self::stories`] whose part is `part_name`, in the
    /// same order. A related part (a header, a footer, the notes, the
    /// comments) is read alone, without serializing the main part, so
    /// finding a header story does not cost the body.
    pub fn stories_in_part(&self, part_name: &str) -> Result<Vec<StoryId>> {
        if part_name == self.doc_part_name {
            return Ok(self
                .stories()?
                .into_iter()
                .filter(|story| story.part_name == part_name)
                .collect());
        }
        let sources = self
            .related_story_sources()?
            .into_iter()
            .filter(|source| source.part_name == part_name)
            .collect();
        let stories = Self::stories_in_sources(sources, None)?;
        #[cfg(debug_assertions)]
        if let Ok(all) = self.stories() {
            assert_eq!(
                stories,
                all.into_iter()
                    .filter(|story| story.part_name == part_name)
                    .collect::<Vec<_>>(),
                "the stories of {part_name} read alone differ from the document's"
            );
        }
        Ok(stories)
    }

    pub(super) fn stories_in_sources(
        sources: Vec<StorySource<'_>>,
        main: Option<MainStoryLayout>,
    ) -> Result<Vec<StoryId>> {
        let mut stories = Vec::new();
        let mut seen = HashSet::new();
        for (index, source) in sources.into_iter().enumerate() {
            let owners = match main.as_ref().filter(|_| index == 0) {
                Some(layout) => layout.owners(&source.xml)?,
                None => scan_story_owners(&source.xml, source.root_kind)?,
            };
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if seen.insert(identity) {
                    stories.push(StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    });
                }
            }
        }
        Ok(stories)
    }

    pub(super) fn story_source_and_owner<'a>(
        &'a self,
        story: &StoryId,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan)> {
        let (source, owner, _) = self.story_source_owner_and_layout(story, false)?;
        Ok((source, owner))
    }

    pub(super) fn story_source_owner_and_layout<'a>(
        &'a self,
        story: &StoryId,
        items: bool,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Option<MainStoryLayout>)> {
        if story.part_name != self.doc_part_name {
            // A story of a related part (a header, a note, a comment) is read
            // from its part alone, without serializing the main part.
            let sources = self.related_story_sources()?;
            return self.story_source_owner_and_layout_in(story, items, sources, &(0..0), &[]);
        }
        let (sources, content, children) = self.spanned_story_sources()?;
        self.story_source_owner_and_layout_in(story, items, sources, &content, &children)
    }

    pub(super) fn story_source_owner_and_layout_in<'a>(
        &self,
        story: &StoryId,
        items: bool,
        mut sources: Vec<StorySource<'a>>,
        content: &Range<usize>,
        children: &[Range<usize>],
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Option<MainStoryLayout>)> {
        let source = sources.iter().position(|source| {
            source.part_name == story.part_name
                && (source.root_kind == story.kind
                    || matches!(story.kind, StoryKind::TableCell | StoryKind::TextBox))
        });
        let (source, layout) = match source {
            Some(index) => {
                let source = sources.swap_remove(index);
                let layout = if index == 0 && source.root_kind == StoryKind::Body {
                    self.main_story_layout(&source.xml, content, children, items)
                        .ok()
                } else {
                    None
                };
                (source, layout)
            }
            None => {
                if let Some(actual) = sources
                    .iter()
                    .find(|source| source.part_name == story.part_name)
                    .map(|source| source.root_kind)
                {
                    return Err(StoryError::WrongOwner {
                        expected: story.kind,
                        actual,
                    }
                    .into());
                }
                return Err(StoryError::OwnerNotFound {
                    story: story.clone(),
                }
                .into());
            }
        };
        let owner = match &layout {
            Some(layout) => layout.owner(&source.xml, story.kind, story.owner_index)?,
            None => scan_story_owners(&source.xml, source.root_kind)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index),
        }
        .ok_or_else(|| StoryError::OwnerNotFound {
            story: story.clone(),
        })?;
        if story.fingerprint != owner.fingerprint {
            return Err(StoryError::Stale {
                story: story.clone(),
            }
            .into());
        }
        Ok((source, owner, layout))
    }

    pub(super) fn main_story_layout(
        &self,
        xml: &[u8],
        content: &Range<usize>,
        children: &[Range<usize>],
        items: bool,
    ) -> Result<MainStoryLayout> {
        let (head, tail) = (&xml[..content.start], &xml[content.end..]);
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = match &scans.frame {
            Some(frame) if scans.head == head && scans.tail == tail => Arc::clone(frame),
            _ => {
                let serialized = scans.serialized.take();
                *scans = MainBodyScans::default();
                let frame = Arc::new(scan_body_block(head, None, tail, true)?);
                *scans = MainBodyScans {
                    head: head.to_vec(),
                    tail: tail.to_vec(),
                    frame: Some(Arc::clone(&frame)),
                    blocks: Vec::new(),
                    fingerprint: None,
                    serialized,
                };
                frame
            }
        };
        let old = std::mem::take(&mut scans.blocks);
        // Owners-only scans have no items. An item lookup must read those
        // children again. An owners-only lookup keeps a scan that already
        // holds items.
        let same = |scan: &Arc<BodyBlockScan>, range: &Range<usize>| {
            scan.bytes == xml[range.clone()] && (!items || scan.items.is_some())
        };
        let prefix = old
            .iter()
            .zip(children)
            .take_while(|(scan, range)| same(scan, range))
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(children[prefix..].iter().rev())
            .take_while(|(scan, range)| same(scan, range))
            .count();
        let mut blocks = children
            .iter()
            .enumerate()
            .map(|(index, _range)| {
                if index < prefix {
                    Some(Arc::clone(&old[index]))
                } else if suffix > 0 && index >= children.len() - suffix {
                    Some(Arc::clone(&old[old.len() - (children.len() - index)]))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let todo = blocks
            .iter()
            .enumerate()
            .filter(|(_, scan)| scan.is_none())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let alone = todo
            .iter()
            .map(|index| head.len() + children[*index].len() + tail.len())
            .sum::<usize>();
        if prefix != children.len() || prefix != old.len() {
            scans.fingerprint = None;
        }
        let whole_walk = alone >= xml.len();
        let scanned = if !whole_walk {
            let mut scanned = Vec::with_capacity(todo.len());
            for index in &todo {
                let bytes = &xml[children[*index].clone()];
                match scan_body_block(head, Some(bytes), tail, items) {
                    Ok(scan) => scanned.push(scan),
                    Err(error) => {
                        scans.blocks = old;
                        return Err(error);
                    }
                }
            }
            scanned
        } else {
            match scan_body_children(xml, children, &todo, items) {
                Ok((scanned, fingerprint)) => {
                    scans.fingerprint = Some(fingerprint);
                    scanned
                }
                Err(error) => {
                    scans.blocks = old;
                    return Err(error);
                }
            }
        };
        for (index, scan) in todo.into_iter().zip(scanned) {
            blocks[index] = Some(Arc::new(scan));
        }
        let blocks = blocks.into_iter().flatten().collect::<Vec<_>>();
        scans.blocks = blocks.clone();
        let fingerprint = scans.fingerprint;
        drop(scans);
        let layout = MainStoryLayout {
            content: content.clone(),
            frame,
            blocks: children
                .iter()
                .map(|range| range.start)
                .zip(blocks)
                .collect(),
            fingerprint,
        };
        #[cfg(debug_assertions)]
        {
            assert_main_story_layout(xml, &layout, items);
        }
        Ok(layout)
    }
}
