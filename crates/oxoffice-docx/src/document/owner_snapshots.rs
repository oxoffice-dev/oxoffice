//! Owner snapshots of stories and tables, and cells found from them.

use super::*;

impl Document {
    pub(super) fn story_table_model(&self, location: &ContentLocation) -> Result<CT_Tbl> {
        if location.item_kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "rich table needs an existing table location".to_owned(),
            ));
        }
        let (source, item) = self.story_item_source(location)?;
        Self::story_table_from_source(&source.xml, &item)
    }

    pub(super) fn story_table_from_source(xml: &[u8], item: &StoryItemSpan) -> Result<CT_Tbl> {
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let closed = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let mut wrapped =
            format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
        wrapped.extend_from_slice(&closed);
        wrapped.extend_from_slice(b"</w:body></w:document>");
        let mut parsed = CT_Document::from_xml(&wrapped)?;
        if parsed.body.content.len() != 1 {
            return Err(Error::Other(
                "rich table has ambiguous physical content".to_owned(),
            ));
        }
        match parsed.body.content.remove(0) {
            BodyContent::Table(table) => Ok(*table),
            _ => Err(Error::Other(
                "rich table source is not a qualified table".to_owned(),
            )),
        }
    }

    /// Read owned facts from one source-qualified table.
    #[doc(hidden)]
    pub fn read_story_table<T>(
        &self,
        location: &ContentLocation,
        read: impl FnOnce(TableRef<'_>) -> T,
    ) -> Result<T> {
        let table = self.story_table_model(location)?;
        Ok(read(TableRef { inner: &table }))
    }

    /// Edit one source-qualified table and publish it atomically.
    #[doc(hidden)]
    pub fn edit_story_table<T>(
        &mut self,
        location: &ContentLocation,
        edit: impl FnOnce(&mut Table<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut table = self.story_table_model(location)?;
        let result = edit(&mut Table { inner: &mut table })?;
        // `Paragraph`'s adders name built-in styles by their English id; a
        // localized file names them otherwise.
        let mut content = [BodyContent::Table(Box::new(table))];
        walkers::visit_body_paragraphs_mut(&mut content, &mut |paragraph| {
            self.localize_run_styles(paragraph)
        });
        let [BodyContent::Table(table)] = content else {
            unreachable!("the table was wrapped above")
        };
        self.replace_story_content(location, ContentFragment::table(*table)?)?;
        Ok(result)
    }

    /// Snapshot all checked Word owners within one immutable source inventory.
    ///
    /// The four families have the same order, payload and admission as story
    /// items, physical table cells, range paragraphs and hyperlinks respectively.
    /// Source qualification is shared only within this call, never across edits.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // One cross-crate projection carries four existing owned families.
    pub fn story_owner_snapshots(
        &self,
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        self.owner_snapshots_of(self.story_sources()?, false)
    }

    /// [`Self::story_owner_snapshots`] of a main part whose body holds only
    /// `table`, placed as a direct body table of this document, with the
    /// table's namespace-complete XML. Body routes are those of that body,
    /// and cell story owners count from zero.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The table XML and the four owned families.
    pub fn detached_table_owner_snapshots(
        &self,
        table: &CT_Tbl,
    ) -> Result<(
        Vec<u8>,
        (
            Vec<StoryItemSnapshot>,
            Vec<(ContentLocation, usize, usize, StoryId)>,
            Vec<(ContentLocation, Vec<u8>)>,
            Vec<(ContentLocation, LinkInfo)>,
        ),
    )> {
        let (part, xml) =
            self.standalone_body_item(&BodyContent::Table(Box::new(table.clone())))?;
        let snapshots = self.owner_snapshots_of(
            vec![StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Owned(part),
            }],
            false,
        )?;
        Ok((xml, snapshots))
    }

    /// The checked Word owners of a main part whose body holds only `item`,
    /// placed as a direct body child of this document.
    ///
    /// Body story items count from the item, and table cell and text box
    /// stories from its first of each, and each snapshot's XML is
    /// namespace-complete. Equal items
    /// give equal owners while [`Self::body_item_context`] is equal, up to
    /// the hyperlink relationships, which they do not check again.
    #[doc(hidden)]
    pub fn body_item_owners(&self, item: &BodyContent) -> Result<BodyItemOwners> {
        // The whole body admits each of its children alone.
        let mut alone = CT_Body {
            content: vec![item.clone()],
            sect_pr: None,
        };
        alone.validate_resource_limits()?;
        let part = self.document.standalone_body_item_xml(item)?;
        let (snapshots, cells, paragraphs, links) = self.owner_snapshots_of(
            vec![StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Borrowed(&part),
            }],
            true,
        )?;
        let owners = scan_story_owners(&part, StoryKind::Body)?;
        let body = owners
            .iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        let items = scan_story_items(&part, body, None)?;
        let count = |kind| owners.iter().filter(|owner| owner.kind == kind).count();
        Ok(BodyItemOwners {
            snapshots,
            cells,
            paragraphs,
            links,
            cell_stories: count(StoryKind::TableCell),
            text_boxes: count(StoryKind::TextBox),
            paragraph_routes: body_paragraph_routes(&alone, &items),
            tables: alone.tables().count(),
            cell_paragraphs: cell_paragraph_routes(&mut alone, &part, &owners, &self.doc_part_name)
                .ok(),
            holds_field: holds_field_words(&part),
        })
    }

    /// What [`Self::body_item_owners`] reads besides the item: the main
    /// part's name and root, and the namespace facts of the opened package.
    #[doc(hidden)]
    pub fn body_item_context(&self) -> BodyItemContext {
        BodyItemContext {
            part_name: self.doc_part_name.clone(),
            root: [
                self.document.extra_namespaces.clone(),
                self.document.root_attributes.clone(),
                self.root_namespace_declarations.clone(),
                self.body_namespace_declarations.clone(),
                self.body_namespace_bindings.clone(),
            ],
            plain_local_edit_blocked: self.plain_local_edit_blocked,
        }
    }

    /// Where an element lies in its part: the paragraph at `paragraph` in
    /// `story`, or the story owner `story` itself (a table cell) when
    /// `paragraph` is `None`. The story is found by kind, part and ordinal,
    /// whatever its fingerprint. `None` when the part holds no such element,
    /// or when a hyperlink or a story item straddles its edge.
    ///
    /// The routes are those of [`Self::story_owner_snapshots`]: an edit of
    /// the element alone renumbers the story items after it by the change in
    /// its item count, the hyperlinks after it in an item that holds it (a
    /// block content control) by the change in its hyperlink count, and the
    /// text box and table cell stories after it by the change in theirs.
    /// The read covers the whole story.
    #[doc(hidden)]
    pub fn element_extent(
        &self,
        story: &StoryId,
        paragraph: Option<&[usize]>,
    ) -> Result<Option<ElementExtent>> {
        let (root_kind, xml) = if story.part_name == self.doc_part_name {
            (
                StoryKind::Body,
                Cow::Owned(self.main_part_with_body_spans()?.0),
            )
        } else {
            match self
                .related_story_sources()?
                .into_iter()
                .find(|source| source.part_name == story.part_name)
            {
                Some(source) => (source.root_kind, source.xml),
                None => return Ok(None),
            }
        };
        extent_in_source(
            &xml,
            root_kind,
            &story.part_name,
            (story.kind, story.owner_index),
            paragraph,
        )
    }

    /// [`Self::element_extent`] read from the direct body block at `slot`
    /// alone, as [`Self::body_item_owners`] reads it: `bases` are the story
    /// items of the body and the text box and table cell stories of the main
    /// part before that block. Item indexes and story ordinals are those of
    /// the whole part; the read covers the block's body story items only, so
    /// it lists the body story's hyperlinks in that block alone. `None` when
    /// the element is not in that block.
    #[doc(hidden)]
    pub fn block_element_extent(
        &self,
        slot: usize,
        (items, boxes, cells): (usize, usize, usize),
        story: &StoryId,
        paragraph: Option<&[usize]>,
    ) -> Result<Option<ElementExtent>> {
        if story.part_name != self.doc_part_name {
            return Ok(None);
        }
        let Some(item) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let local = match story.kind {
            StoryKind::Body => Some(story.owner_index),
            StoryKind::TextBox => story.owner_index.checked_sub(boxes),
            StoryKind::TableCell => story.owner_index.checked_sub(cells),
            _ => None,
        };
        let Some(local) = local else {
            return Ok(None);
        };
        let path = match paragraph {
            Some(path) if story.kind == StoryKind::Body => {
                let mut path = path.to_vec();
                match path.first_mut().map(|head| head.checked_sub(items)) {
                    Some(Some(head)) => path[0] = head,
                    _ => return Ok(None),
                }
                Some(path)
            }
            Some(path) => Some(path.to_vec()),
            None => None,
        };
        let xml = self.document.standalone_body_item_xml(item)?;
        let Some(mut extent) = extent_in_source(
            &xml,
            StoryKind::Body,
            &story.part_name,
            (story.kind, local),
            path.as_deref(),
        )?
        else {
            return Ok(None);
        };
        if story.kind == StoryKind::Body {
            extent.items.0 += items;
            extent.read = extent.read.start + items..extent.read.end + items;
            for (path, _, _) in &mut extent.links {
                path[0] += items;
            }
        }
        extent.text_boxes.0 += boxes;
        extent.cell_stories.0 += cells;
        Ok(Some(extent))
    }

    /// The bytes of every story owner, story item, paragraph and hyperlink of
    /// the document, by the routes of [`Self::story_owner_snapshots`], each
    /// closed over the namespaces it uses. Debug checks compare them across
    /// an edit with [`resolved_xml_events`].
    #[doc(hidden)]
    pub fn owner_element_xml(&self) -> Result<OwnerElementXml> {
        type Key = (StoryKind, String, usize, Vec<usize>, usize);
        let mut result = OwnerElementXml::default();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let xml = &source.xml;
            let owners = scan_story_owners(xml, source.root_kind)?;
            let owner_scopes =
                story_namespace_scopes_at(xml, owners.iter().map(|owner| owner.full.start))?;
            // Each element by family (owner, item, paragraph, link), key and span.
            let mut elements: Vec<(u8, Key, Range<usize>)> = Vec::new();
            for owner in &owners {
                if !seen.insert((owner.kind, source.part_name.clone(), owner.owner_index)) {
                    continue;
                }
                let key = |path: Vec<usize>, index: usize| -> Key {
                    (
                        owner.kind,
                        source.part_name.clone(),
                        owner.owner_index,
                        path,
                        index,
                    )
                };
                elements.push((0, key(Vec::new(), 0), owner.full.clone()));
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".into())
                })?;
                let items = scan_story_items_with_scope(xml, owner, scope, None)?;
                let item_scopes = story_namespace_scopes_at(
                    xml,
                    items
                        .iter()
                        .flat_map(|item| [item.scan.start, item.full.start]),
                )?;
                let id = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                for (index, item) in items.iter().enumerate() {
                    elements.push((1, key(vec![index], 0), item.full.clone()));
                }
                for (location, span) in story_range_paragraph_spans_from_inventory(
                    xml,
                    &[(id.clone(), owner.clone())],
                    std::slice::from_ref(&items),
                    &item_scopes,
                )?
                .into_iter()
                .flatten()
                {
                    elements.push((2, key(location.index_path, 0), span));
                }
                let mut local = HashMap::<Vec<usize>, usize>::new();
                for (location, link) in ordered_story_link_spans(xml, &id, items, &item_scopes)? {
                    let index = local.entry(location.index_path.clone()).or_default();
                    elements.push((3, key(location.index_path, *index), link.full));
                    *index += 1;
                }
            }
            let scopes =
                story_namespace_scopes_at(xml, elements.iter().map(|(_, _, span)| span.start))?;
            for (family, key, span) in elements {
                let scope = scopes.get(&span.start).ok_or_else(|| {
                    Error::Other("story element namespace scope was not inventoried".into())
                })?;
                let closed = close_content_fragment_namespaces(&xml[span], scope)?;
                let (kind, part, ordinal, path, index) = key;
                match family {
                    0 => {
                        result.stories.insert((kind, part, ordinal), closed);
                    }
                    1 => {
                        result.items.insert((kind, part, ordinal, path), closed);
                    }
                    2 => {
                        result
                            .paragraphs
                            .insert((kind, part, ordinal, path), closed);
                    }
                    _ => {
                        result
                            .links
                            .insert((kind, part, ordinal, path, index), closed);
                    }
                }
            }
        }
        Ok(result)
    }

    /// The direct body children, as [`Self::body_item_owners`] takes them.
    #[doc(hidden)]
    pub fn body_children(&self) -> &[BodyContent] {
        &self.document.body.content
    }

    /// The story parts after the main part, in the order of
    /// [`Self::story_owner_snapshots`]: the root story kind, the part name
    /// and the bytes of each, read without serializing the main part.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The kind, name and bytes of each part.
    pub fn story_part_sources(&self) -> Result<Vec<(StoryKind, String, Cow<'_, [u8]>)>> {
        Ok(self
            .related_story_sources()?
            .into_iter()
            .map(|source| (source.root_kind, source.part_name, source.xml))
            .collect())
    }

    /// [`Self::story_owner_snapshots`] of one part that
    /// [`Self::story_part_sources`] lists, each snapshot's XML
    /// namespace-complete.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The four owned families of one part.
    pub fn story_part_owner_snapshots(
        &self,
        kind: StoryKind,
        part_name: &str,
        xml: &[u8],
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        self.owner_snapshots_of(
            vec![StorySource {
                root_kind: kind,
                part_name: part_name.to_owned(),
                xml: Cow::Borrowed(xml),
            }],
            true,
        )
    }

    /// The relationship IDs of `part_name` that a hyperlink of a story in
    /// that part may name: the first relationship of each ID, when it is an
    /// external hyperlink, as the owner snapshots check a link's
    /// relationship.
    #[doc(hidden)]
    pub fn story_hyperlink_relationships(&self, part_name: &str) -> HashSet<&str> {
        let mut seen = HashSet::new();
        let mut valid = HashSet::new();
        for relationship in self
            .package
            .get_part_rels(part_name)
            .map_or(&[][..], |relationships| relationships.items.as_slice())
        {
            if seen.insert(relationship.id.as_str())
                && relationship.rel_type == rel_types::HYPERLINK
                && relationship.target_mode.as_deref() == Some("External")
            {
                valid.insert(relationship.id.as_str());
            }
        }
        valid
    }

    #[allow(clippy::type_complexity)] // One cross-crate projection carries four existing owned families.
    fn owner_snapshots_of(
        &self,
        sources: Vec<StorySource<'_>>,
        standalone: bool,
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        let mut snapshots = Vec::new();
        let mut cells = Vec::new();
        let mut paragraphs = Vec::new();
        let mut links = Vec::new();
        let mut seen = HashSet::new();
        let mut emitted_cells = HashSet::new();
        for source in sources {
            let physical_owners = scan_story_owners(&source.xml, source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                &source.xml,
                physical_owners.iter().map(|owner| owner.full.start),
            )?;
            let mut owners = Vec::with_capacity(physical_owners.len());
            let mut inventories = Vec::with_capacity(physical_owners.len());
            let mut admitted = Vec::with_capacity(physical_owners.len());
            for owner in &physical_owners {
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".into())
                })?;
                inventories.push(scan_story_items_with_scope(
                    &source.xml,
                    owner,
                    scope,
                    None,
                )?);
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                admitted.push(seen.insert((
                    owner.kind,
                    source.part_name.clone(),
                    owner.owner_index,
                )));
                owners.push((story, owner.clone()));
            }
            let scopes = story_namespace_scopes_at(
                &source.xml,
                inventories
                    .iter()
                    .flatten()
                    .flat_map(|item| [item.scan.start, item.full.start]),
            )?;
            cells.extend(Self::story_table_cells_from_inventory(
                &source,
                &physical_owners,
                &inventories,
                &scopes,
                &mut emitted_cells,
            )?);
            let paragraph_spans = story_range_paragraph_spans_from_inventory(
                &source.xml,
                &owners,
                &inventories,
                &scopes,
            )?;
            let mut link_spans = Vec::with_capacity(owners.len());
            for ((story, _), items) in owners.iter().zip(&inventories) {
                link_spans.push(ordered_story_link_spans(
                    &source.xml,
                    story,
                    items.clone(),
                    &scopes,
                )?);
            }
            let leaf_scopes = story_namespace_scopes_at(
                &source.xml,
                paragraph_spans
                    .iter()
                    .flatten()
                    .map(|(_, span)| span.start)
                    .chain(link_spans.iter().flatten().map(|(_, link)| link.full.start)),
            )?;
            for (owner_index, ((story, _), items)) in owners.iter().zip(&inventories).enumerate() {
                if !admitted[owner_index] {
                    continue;
                }
                let mut direct_items = Vec::new();
                if story.kind == StoryKind::Body {
                    for item in items.iter().filter(|item| item.direct_owner_child) {
                        if item.kind != StoryItemKind::PreservedNode
                            || !content_fragment_root_is_section_properties(&source.xml, item)?
                        {
                            direct_items.push(item);
                        }
                    }
                }
                let mut direct_cursor = 0;
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
                    let scope = scopes.get(&item.scan.start).ok_or_else(|| {
                        Error::Other("story item namespace scope was not inventoried".into())
                    })?;
                    let text = story_item_text_with_scope(&source.xml, item, scope, items)?;
                    let xml = if item.complex_field {
                        complex_story_field_xml(&source.xml, item)?
                    } else if standalone {
                        let scope = scopes.get(&item.full.start).ok_or_else(|| {
                            Error::Other("story item namespace scope was not inventoried".into())
                        })?;
                        close_content_fragment_namespaces(&source.xml[item.full.clone()], scope)?
                    } else {
                        source.xml[item.full.clone()].to_vec()
                    };
                    snapshots.push(StoryItemSnapshot {
                        location: ContentLocation::new(story.clone(), item.kind, vec![index]),
                        direct_body_index,
                        direct_child: item.direct_owner_child,
                        text,
                        xml,
                    });
                }
            }
            for (index, mut spans) in paragraph_spans.into_iter().enumerate() {
                if !admitted[index] {
                    continue;
                }
                spans.sort_by_key(|(_, span)| span.start);
                for (location, span) in spans {
                    let scope = leaf_scopes.get(&span.start).ok_or_else(|| {
                        Error::Other("story paragraph namespace scope was not inventoried".into())
                    })?;
                    paragraphs.push((
                        location,
                        close_content_fragment_namespaces(&source.xml[span], scope)?,
                    ));
                }
            }
            for (index, spans) in link_spans.into_iter().enumerate() {
                if !admitted[index] {
                    continue;
                }
                for (location, span) in spans {
                    let scope = leaf_scopes.get(&span.full.start).ok_or_else(|| {
                        Error::Other("story hyperlink namespace scope was not inventoried".into())
                    })?;
                    let info = self.story_link_info(location.story(), &source.xml, span, scope)?;
                    links.push((location, info));
                }
            }
        }
        Ok((snapshots, cells, paragraphs, links))
    }

    #[allow(clippy::type_complexity)]
    fn story_table_cells_from_inventory(
        source: &StorySource<'_>,
        owners: &[StoryOwnerSpan],
        inventories: &[Vec<StoryItemSpan>],
        scopes: &HashMap<usize, BTreeMap<String, String>>,
        emitted: &mut HashSet<(ContentLocation, usize, usize)>,
    ) -> Result<Vec<(ContentLocation, usize, usize, StoryId)>> {
        let mut result = Vec::new();
        let mut tables = Vec::new();
        for (owner, items) in owners.iter().zip(inventories) {
            let story = StoryId {
                kind: owner.kind,
                part_name: source.part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            };
            for (index, item) in items.iter().enumerate() {
                if item.kind == StoryItemKind::Table {
                    tables.push((
                        item.full.clone(),
                        ContentLocation::new(story.clone(), item.kind, vec![index]),
                    ));
                }
            }
        }
        // Recover physical order with one lexical pass instead of sorting
        // a growing table inventory. Qualification came from the owner scan.
        let mut by_start = tables
            .into_iter()
            .map(|item| (item.0.start, item))
            .collect::<HashMap<_, _>>();
        let mut tables = Vec::with_capacity(by_start.len());
        let mut reader = quick_xml::Reader::from_reader(source.xml.as_ref());
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            match reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("table source order scan failed: {error}")))?
            {
                Event::Start(_) | Event::Empty(_) => {
                    if let Some(item) = by_start.remove(&before) {
                        tables.push(item);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        if !by_start.is_empty() {
            return Err(Error::Other(
                "table physical source order is incomplete".into(),
            ));
        }
        let cells = owners
            .iter()
            .filter(|owner| owner.kind == StoryKind::TableCell)
            .collect::<Vec<_>>();
        let mut next = 0;
        let mut cell_cursor = 0;
        while next < tables.len() {
            let root = next;
            let span = tables[root].0.clone();
            next += 1;
            while next < tables.len() && tables[next].0.end < span.end {
                next += 1;
            }
            let scope = scopes
                .get(&span.start)
                .ok_or_else(|| Error::Other("table item scope missing".into()))?;
            let closed = close_content_fragment_namespaces(&source.xml[span.clone()], scope)?;
            let mut wrapped =
                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
            wrapped.extend(closed);
            wrapped.extend_from_slice(b"</w:body></w:document>");
            let parsed = CT_Document::from_xml(&wrapped)?;
            let [BodyContent::Table(table)] = parsed.body.content.as_slice() else {
                return Err(Error::Other(
                    "rich table has ambiguous physical content".into(),
                ));
            };
            let (count, modeled) = physical_table_cells(table);
            while cells
                .get(cell_cursor)
                .is_some_and(|owner| owner.full.start < span.start)
            {
                cell_cursor += 1;
            }
            let first = cell_cursor;
            while cells
                .get(cell_cursor)
                .is_some_and(|owner| owner.full.start < span.end)
            {
                cell_cursor += 1;
            }
            let physical = &cells[first..cell_cursor];
            if modeled.len() != physical.len() {
                return Err(Error::Other(
                    "rich table cell source correspondence is incomplete".into(),
                ));
            }
            let mut table_routes = vec![None; count];
            let mut containing = Vec::<usize>::new();
            let mut cursor = root;
            for ((table, coordinate), owner) in modeled.into_iter().zip(physical) {
                while cursor < next && tables[cursor].0.start < owner.full.start {
                    while containing
                        .last()
                        .is_some_and(|&index| tables[index].0.end < tables[cursor].0.end)
                    {
                        containing.pop();
                    }
                    containing.push(cursor);
                    cursor += 1;
                }
                while containing
                    .last()
                    .is_some_and(|&index| tables[index].0.end <= owner.full.start)
                {
                    containing.pop();
                }
                let index = *containing
                    .last()
                    .ok_or_else(|| Error::Other("cell has no physical table".into()))?;
                if owner.full.end >= tables[index].0.end
                    || table_routes[table].is_some_and(|before| before != index)
                {
                    return Err(Error::Other(
                        "rich table cell has ambiguous physical owner".into(),
                    ));
                }
                table_routes[table] = Some(index);
                if let Some((row, cell)) = coordinate
                    && emitted.insert((tables[index].1.clone(), row, cell))
                {
                    result.push((
                        tables[index].1.clone(),
                        row,
                        cell,
                        StoryId {
                            kind: owner.kind,
                            part_name: source.part_name.clone(),
                            owner_index: owner.owner_index,
                            fingerprint: owner.fingerprint,
                        },
                    ));
                }
            }
        }
        Ok(result)
    }

    /// Resolve one related table cell through its actual enclosing physical table.
    #[doc(hidden)]
    pub fn story_table_cell(
        &self,
        location: &ContentLocation,
        row: usize,
        cell: usize,
    ) -> Result<StoryId> {
        if location.item_kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "rich table needs an existing table location".to_owned(),
            ));
        }
        let (source, item) = self.story_item_source(location)?;
        let owner = physical_table_cell_owner(&source.xml, source.root_kind, &item, row, cell)?;
        let story = StoryId {
            kind: owner.kind,
            part_name: source.part_name.clone(),
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        };
        Ok(story)
    }

    /// Resolve one paragraph through its actual checked related table cell owner.
    #[doc(hidden)]
    pub fn story_table_cell_paragraph_location(
        &self,
        location: &ContentLocation,
        row: usize,
        cell: usize,
        paragraph: usize,
    ) -> Result<ContentLocation> {
        let story = self.story_table_cell(location, row, cell)?;
        let (source, owner) = self.story_source_and_owner(&story)?;
        let spans = story_range_paragraph_spans(&source.xml, &[(story.clone(), owner.clone())])?
            .into_iter()
            .flatten();
        let (location, _) = spans.into_iter().nth(paragraph).ok_or_else(|| {
            Error::Other("rich table cell paragraph index is out of range".to_owned())
        })?;
        self.story_range_paragraph_snapshot(&location)?;
        Ok(location)
    }
}

/// The events of the XML fragment `xml`, each element and attribute name
/// resolved to its namespace and every namespace declaration left out: two
/// fragments that hold the same elements give the same events wherever their
/// declarations sit, and a prefix bound to another namespace gives other
/// events. The fragment declares every prefix it uses, as the elements of
/// [`Document::owner_element_xml`] do. Debug checks compare elements across
/// an edit with them.
#[doc(hidden)]
pub fn resolved_xml_events(xml: &[u8]) -> Result<Vec<Vec<u8>>> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    let namespace = |result: ResolveResult<'_>| match result {
        ResolveResult::Bound(namespace) => Ok(namespace.as_ref().to_vec()),
        ResolveResult::Unbound => Ok(Vec::new()),
        ResolveResult::Unknown(prefix) => Err(Error::Other(format!(
            "XML fragment uses the undeclared prefix {}",
            String::from_utf8_lossy(&prefix)
        ))),
    };
    let mut reader = quick_xml::NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut events = Vec::new();
    loop {
        let (resolved, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("malformed XML fragment: {error}")))?;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let mut record = b"<".to_vec();
                record.extend(namespace(resolved)?);
                record.push(b'}');
                record.extend_from_slice(element.local_name().as_ref());
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("malformed XML attribute: {error}"))
                    })?;
                    let key = attribute.key.as_ref();
                    if key == b"xmlns" || key.starts_with(b"xmlns:") {
                        continue;
                    }
                    let (attribute_namespace, local) =
                        reader.resolver().resolve_attribute(attribute.key);
                    record.push(b' ');
                    record.extend(namespace(attribute_namespace)?);
                    record.push(b'}');
                    record.extend_from_slice(local.as_ref());
                    record.push(b'=');
                    record.extend_from_slice(&attribute.value);
                }
                events.push(record);
                if matches!(event, Event::Empty(_)) {
                    events.push(b"</".to_vec());
                }
            }
            Event::End(_) => events.push(b"</".to_vec()),
            Event::Eof => break,
            other => events.push(format!("{other:?}").into_bytes()),
        }
        buffer.clear();
    }
    Ok(events)
}

/// The extent of the element of [`Document::element_extent`] in one story
/// source: the paragraph at `paragraph` in the story of `kind` and ordinal
/// `owner_index`, or that story itself.
fn extent_in_source(
    xml: &[u8],
    root_kind: StoryKind,
    part_name: &str,
    (kind, owner_index): (StoryKind, usize),
    paragraph: Option<&[usize]>,
) -> Result<Option<ElementExtent>> {
    let owners = scan_story_owners(xml, root_kind)?;
    let Some(owner) = owners
        .iter()
        .find(|owner| owner.kind == kind && owner.owner_index == owner_index)
    else {
        return Ok(None);
    };
    let owner_scopes = story_namespace_scopes_at(xml, [owner.full.start])?;
    let scope = owner_scopes
        .get(&owner.full.start)
        .ok_or_else(|| Error::Other("story owner namespace scope was not inventoried".into()))?;
    let items = scan_story_items_with_scope(xml, owner, scope, None)?;
    let item_scopes = story_namespace_scopes_at(
        xml,
        items
            .iter()
            .flat_map(|item| [item.scan.start, item.full.start]),
    )?;
    let id = StoryId {
        kind: owner.kind,
        part_name: part_name.to_owned(),
        owner_index: owner.owner_index,
        fingerprint: owner.fingerprint,
    };
    let span = match paragraph {
        None => owner.full.clone(),
        Some(path) => {
            let spans = story_range_paragraph_spans_from_inventory(
                xml,
                &[(id.clone(), owner.clone())],
                std::slice::from_ref(&items),
                &item_scopes,
            )?;
            match spans
                .into_iter()
                .flatten()
                .find(|(location, _)| location.index_path == path)
            {
                Some((_, span)) => span,
                None => return Ok(None),
            }
        }
    };
    let placement = |range: &Range<usize>| {
        if range.end <= span.start {
            Some(std::cmp::Ordering::Less)
        } else if span.end <= range.start {
            Some(std::cmp::Ordering::Greater)
        } else if span.start <= range.start && range.end <= span.end {
            Some(std::cmp::Ordering::Equal)
        } else {
            None
        }
    };
    let mut first = 0;
    let mut count = 0;
    for (index, item) in items.iter().enumerate() {
        match placement(&item.full) {
            Some(std::cmp::Ordering::Less) => first = index + 1,
            Some(std::cmp::Ordering::Equal) if first + count == index => count += 1,
            Some(std::cmp::Ordering::Greater) => {}
            // A block content control holds the paragraph: it starts before.
            None if item.full.start <= span.start && span.end <= item.full.end => {
                first = index + 1;
            }
            _ => return Ok(None),
        }
    }
    let read = 0..items.len();
    let mut local = HashMap::<Vec<usize>, usize>::new();
    let mut links = Vec::new();
    for (location, link) in ordered_story_link_spans(xml, &id, items, &item_scopes)? {
        let Some(place) = placement(&link.full) else {
            return Ok(None);
        };
        let index = local.entry(location.index_path.clone()).or_default();
        links.push((location.index_path, *index, place));
        *index += 1;
    }
    let stories = |kind: StoryKind| {
        let mut before = 0;
        let mut inside = 0;
        for other in owners.iter().filter(|other| other.kind == kind) {
            // A cell is its own story; a paragraph's story starts before it.
            if paragraph.is_none() && std::ptr::eq(other, owner) {
                continue;
            }
            match placement(&other.full) {
                Some(std::cmp::Ordering::Equal) => inside += 1,
                _ if other.full.start < span.start => before += 1,
                _ => {}
            }
        }
        (before, inside)
    };
    let element_scope = story_namespace_scopes_at(xml, [span.start])?;
    let element_scope = element_scope
        .get(&span.start)
        .ok_or_else(|| Error::Other("story element namespace scope was not inventoried".into()))?;
    Ok(Some(ElementExtent {
        xml: close_content_fragment_namespaces(&xml[span.clone()], element_scope)?,
        items: (first, count),
        read,
        links,
        text_boxes: stories(StoryKind::TextBox),
        cell_stories: stories(StoryKind::TableCell),
    }))
}
