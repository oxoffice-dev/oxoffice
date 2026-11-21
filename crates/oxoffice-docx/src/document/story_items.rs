//! Locating story items, their snapshots and links.

use super::*;

impl Document {
    pub(super) fn story_item_source<'a>(
        &'a self,
        location: &ContentLocation,
    ) -> Result<(StorySource<'a>, StoryItemSpan)> {
        let (source, owner, layout) = self.story_source_owner_and_layout(&location.story, true)?;
        let item = Self::story_item_span(source.xml.as_ref(), &owner, layout.as_ref(), location)?;
        Ok((source, item))
    }

    pub(super) fn story_item_span(
        xml: &[u8],
        owner: &StoryOwnerSpan,
        layout: Option<&MainStoryLayout>,
        location: &ContentLocation,
    ) -> Result<StoryItemSpan> {
        if location.index_path.len() != 1 {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let index = location.index_path[0];
        let (item, len) = story_owner_item(xml, owner, layout, index)?;
        let item = item.ok_or(StoryError::OutOfBounds { index, len })?;
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        Ok(item)
    }
}

impl Document {
    /// Traverse one story's supported content without constructing a second
    /// document tree.
    pub fn story_items<'a>(&'a self, story: &StoryId) -> Result<Vec<StoryItemRef<'a>>> {
        let (source, owner, layout) = self.story_source_owner_and_layout(story, true)?;
        let spans = story_owner_items(source.xml.as_ref(), &owner, layout.as_ref())?;
        Ok(spans
            .into_iter()
            .enumerate()
            .map(|(index, item)| StoryItemRef {
                document: self,
                location: ContentLocation {
                    story: story.clone(),
                    item_kind: item.kind,
                    index_path: vec![index],
                    is_end: false,
                },
            })
            .collect())
    }
}

impl Document {
    /// The body story location of the direct body child at `slot`.
    /// `Ok(None)` is absence. A scan that cannot be proved uses the whole
    /// story snapshot lookup. The error is that lookup's error.
    #[doc(hidden)]
    pub fn direct_body_item_location(&self, slot: usize) -> Result<Option<ContentLocation>> {
        let (xml, content, children) = self.main_part_with_body_spans()?;
        self.direct_body_item_location_in(&xml, &content, &children, slot)
    }

    fn direct_body_item_location_in(
        &self,
        xml: &[u8],
        content: &Range<usize>,
        children: &[Range<usize>],
        slot: usize,
    ) -> Result<Option<ContentLocation>> {
        let Ok(layout) = self.main_story_layout(xml, content, children, true) else {
            return full_direct_body_item_location(xml, &self.doc_part_name, slot);
        };
        let owner = layout
            .owner(xml, StoryKind::Body, 0)?
            .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
        let story = StoryId {
            kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            owner_index: 0,
            fingerprint: owner.fingerprint,
        };
        let located = layout.direct_body_item(xml, slot, true).map(|item| {
            item.map(|(index, item)| ContentLocation::new(story, item.kind, vec![index]))
        });
        #[cfg(debug_assertions)]
        {
            let whole = full_direct_body_item_location(xml, &self.doc_part_name, slot);
            match (&located, &whole) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "direct body item location {slot}")
                }
                (Err(actual), Err(expected)) => assert_eq!(
                    actual.to_string(),
                    expected.to_string(),
                    "direct body item location {slot}"
                ),
                (Ok(actual), Err(expected)) => panic!(
                    "direct body item location {slot}: fast Ok {actual:?} hid full error {expected}"
                ),
                (Err(actual), Ok(expected)) => panic!(
                    "direct body item location {slot}: fast error {actual} but full Ok {expected:?}"
                ),
            }
        }
        located
    }

    /// Resolve a physical direct-body cell from the actual current main output.
    /// Current writer proof shares that output. Other cases keep the full lookup.
    #[doc(hidden)]
    pub fn direct_body_cell_story(
        &self,
        slot: usize,
        row: usize,
        cell: usize,
    ) -> Result<Option<StoryId>> {
        let (xml, content, children) = self.main_part_with_body_spans()?;
        let Some(location) = self.direct_body_item_location_in(&xml, &content, &children, slot)?
        else {
            return Ok(None);
        };
        // This proof describes the actual current writer bytes above. It is
        // not an input/read receipt and never grants scoped Source credit.
        let same_current_writer = {
            let scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            offered_canonical_blocks(scans.serialized.as_ref())
                .is_some_and(|record| record.writer_bytes_match(&self.document, &xml) == Some(true))
        };
        if !same_current_writer || location.item_kind != StoryItemKind::Table || location.is_end {
            return self.story_table_cell(&location, row, cell).map(Some);
        }
        // The old second source preparation also collects related sources.
        // Preserve those relationship/part errors before owner/item lookup.
        let mut sources = vec![StorySource {
            root_kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            xml: Cow::Borrowed(xml.as_slice()),
        }];
        sources.extend(self.related_story_sources()?);
        let (source, owner, layout) = self.story_source_owner_and_layout_in(
            &location.story,
            true,
            sources,
            &content,
            &children,
        )?;
        let item = Self::story_item_span(source.xml.as_ref(), &owner, layout.as_ref(), &location)?;
        let owner =
            physical_table_cell_owner(source.xml.as_ref(), source.root_kind, &item, row, cell)?;
        Ok(Some(StoryId {
            kind: owner.kind,
            part_name: source.part_name,
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        }))
    }

    /// Materialize every story item after building each package source once.
    ///
    /// Unlike repeated calls through [`StoryItemRef`], this owned projection
    /// serializes the main document once and scans each story owner once for
    /// the complete snapshot.
    pub fn story_item_snapshots(&self) -> Result<Vec<StoryItemSnapshot>> {
        self.collect_story_item_snapshots(false)
    }

    /// [`Document::story_item_snapshots`] with each item's XML standalone.
    ///
    /// Every item declares the namespaces in scope at its start, as
    /// [`Document::story_range_paragraph_snapshot`] does, so it parses alone.
    /// The exact-slice API stays [`Document::story_item_snapshots`].
    #[doc(hidden)]
    pub fn standalone_story_item_snapshots(&self) -> Result<Vec<StoryItemSnapshot>> {
        self.collect_story_item_snapshots(true)
    }

    fn collect_story_item_snapshots(&self, standalone: bool) -> Result<Vec<StoryItemSnapshot>> {
        let mut snapshots = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let owners = scan_story_owners(source.xml.as_ref(), source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                owners.iter().map(|owner| owner.full.start),
            )?;
            let mut inventories = Vec::new();
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                let items = scan_story_items_with_scope(source.xml.as_ref(), &owner, scope, None)?;
                let direct_items = if owner.kind == StoryKind::Body {
                    let mut direct = Vec::new();
                    for item in items.iter().filter(|item| item.direct_owner_child) {
                        if item.kind != StoryItemKind::PreservedNode
                            || !content_fragment_root_is_section_properties(
                                source.xml.as_ref(),
                                item,
                            )?
                        {
                            direct.push(item.clone());
                        }
                    }
                    direct
                } else {
                    Vec::new()
                };
                inventories.push((story, items, direct_items));
            }
            let item_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                inventories.iter().flat_map(|(_, items, _)| {
                    items.iter().flat_map(|item| {
                        [item.scan.start]
                            .into_iter()
                            .chain((standalone && !item.complex_field).then_some(item.full.start))
                    })
                }),
            )?;
            for (story, items, direct_items) in inventories {
                let mut direct_cursor = 0usize;
                for (index, item) in items.iter().enumerate() {
                    while direct_items
                        .get(direct_cursor)
                        .is_some_and(|direct| direct.full.end <= item.full.start)
                    {
                        direct_cursor += 1;
                    }
                    let direct_body_index = direct_items.get(direct_cursor).and_then(|direct| {
                        (direct.full.start <= item.full.start && item.full.end <= direct.full.end)
                            .then_some(direct_cursor)
                    });
                    let scope = item_scopes.get(&item.scan.start).ok_or_else(|| {
                        Error::Other("story item namespace scope was not inventoried".to_owned())
                    })?;
                    let text =
                        story_item_text_with_scope(source.xml.as_ref(), item, scope, &items)?;
                    let xml = if item.complex_field {
                        complex_story_field_xml(source.xml.as_ref(), item)?
                    } else if standalone {
                        let scope = item_scopes.get(&item.full.start).ok_or_else(|| {
                            Error::Other(
                                "story item namespace scope was not inventoried".to_owned(),
                            )
                        })?;
                        close_content_fragment_namespaces(&source.xml[item.full.clone()], scope)?
                    } else {
                        source.xml[item.full.clone()].to_vec()
                    };
                    snapshots.push(StoryItemSnapshot {
                        location: ContentLocation {
                            story: story.clone(),
                            item_kind: item.kind,
                            index_path: vec![index],
                            is_end: false,
                        },
                        direct_body_index,
                        direct_child: item.direct_owner_child,
                        text,
                        xml,
                    });
                }
            }
        }
        Ok(snapshots)
    }

    pub(super) fn story_link_info(
        &self,
        story: &StoryId,
        xml: &[u8],
        link: StoryLinkSpan,
        scope: &BTreeMap<String, String>,
    ) -> Result<LinkInfo> {
        let text_item = StoryItemSpan {
            kind: StoryItemKind::Paragraph,
            full: link.full.clone(),
            scan: link.full,
            direct_owner_child: false,
            complex_field: false,
            complex_ancestors: Vec::new(),
            sdt_context: None,
        };
        let text = story_item_text_with_scope(xml, &text_item, scope, &[])?.unwrap_or_default();
        let url = link
            .rel_id
            .as_deref()
            .map(|relationship_id| self.hyperlink_url_for_validated_story(story, relationship_id))
            .transpose()?;
        Ok(LinkInfo {
            text,
            url,
            anchor: link.anchor,
            rel_id: link.rel_id,
        })
    }

    /// Return one story's modeled hyperlinks in physical source order together
    /// with the checked location of the item that owns each hyperlink.
    pub fn story_links(&self, story: &StoryId) -> Result<Vec<(ContentLocation, LinkInfo)>> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let links = story_link_spans(source.xml.as_ref(), &owner, story)?;
        let link_scopes = story_namespace_scopes_at(
            source.xml.as_ref(),
            links.iter().map(|(_, link)| link.full.start),
        )?;
        links
            .into_iter()
            .map(|(location, link)| {
                let scope = link_scopes.get(&link.full.start).ok_or_else(|| {
                    Error::Other("story hyperlink namespace scope was not inventoried".to_owned())
                })?;
                let info = self.story_link_info(story, source.xml.as_ref(), link, scope)?;
                Ok((location, info))
            })
            .collect()
    }

    /// Materialize every modeled story hyperlink from one package inventory.
    pub fn story_link_snapshots(&self) -> Result<Vec<(ContentLocation, LinkInfo)>> {
        let mut snapshots = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let owners = scan_story_owners(source.xml.as_ref(), source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                owners.iter().map(|owner| owner.full.start),
            )?;
            let mut inventories = Vec::new();
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                let items = scan_story_items_with_scope(source.xml.as_ref(), &owner, scope, None)?;
                inventories.push((story, items));
            }
            let item_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                inventories
                    .iter()
                    .flat_map(|(_, items)| items.iter().map(|item| item.scan.start)),
            )?;
            let mut link_inventories = Vec::new();
            for (story, items) in inventories {
                let links =
                    ordered_story_link_spans(source.xml.as_ref(), &story, items, &item_scopes)?;
                link_inventories.push((story, links));
            }
            let link_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                link_inventories
                    .iter()
                    .flat_map(|(_, links)| links.iter().map(|(_, link)| link.full.start)),
            )?;
            for (story, links) in link_inventories {
                for (location, link) in links {
                    let scope = link_scopes.get(&link.full.start).ok_or_else(|| {
                        Error::Other(
                            "story hyperlink namespace scope was not inventoried".to_owned(),
                        )
                    })?;
                    let info = self.story_link_info(&story, source.xml.as_ref(), link, scope)?;
                    snapshots.push((location, info));
                }
            }
        }
        Ok(snapshots)
    }
}
