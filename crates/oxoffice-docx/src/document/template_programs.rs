//! Template source programs: direct table children, copied and retained ranges, paired edits.

use super::*;

// The same modeled event admission used by the existing paired projection.
// This finite projection adds only direct table children, not another owner map.
#[allow(clippy::type_complexity)] // One direct-child range plus its row flag, keyed by table.
fn template_direct_table_children(
    xml: &[u8],
) -> Result<BTreeMap<usize, Vec<(Range<usize>, bool)>>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut buffer = Vec::new();
    let mut tables = BTreeMap::<usize, Vec<(Range<usize>, bool)>>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let after = reader.buffer_position() as usize;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let namespace = story_namespace(&namespace);
                let local = local.as_ref();
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes =
                    story_word_prefixes_at(&element, inherited, namespace == StoryNamespace::Word)?;
                let modeled = !stack.last().is_some_and(|frame| frame.opaque)
                    && modeled_story_event(
                        &mut stack, namespace, local, &element, empty, xml, before, after,
                    )?;
                if modeled && namespace == StoryNamespace::Word && local == b"tbl" {
                    tables.entry(before).or_default();
                }
                if let Some(parent) = stack.last().filter(|parent| {
                    !parent.opaque
                        && parent.namespace == StoryNamespace::Word
                        && parent.local_name.as_ref() == b"tbl"
                }) && !(namespace == StoryNamespace::Word
                    && matches!(local, b"tblPr" | b"tblGrid"))
                {
                    let end = if empty {
                        after
                    } else {
                        story_element_end(xml, before)?
                    };
                    tables.entry(parent.full_start).or_default().push((
                        before..end,
                        modeled && namespace == StoryNamespace::Word && local == b"tr",
                    ));
                }
                if !empty {
                    let sdt_context = story_element_sdt_context(&stack, namespace, local);
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: story_frame_local_name(local),
                        is_word: namespace == StoryNamespace::Word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque: !modeled,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(tables)
}

pub(super) fn prepare_template_source_programs(
    source: &StorySource<'_>,
) -> Result<Vec<crate::template::TemplateProgram>> {
    prepare_template_source_programs_in(source, None)
}

fn copy_template_source_range(
    source: &StorySource<'_>,
    range: Range<usize>,
    scoped: &mut Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Vec<u8>> {
    if let Some((operation, parent)) = scoped {
        let copied = operation.compose_sources(&[(Some(*parent), range, &[])])?;
        Ok(operation.finish_source(copied)?)
    } else {
        Ok(source.xml[range].to_vec())
    }
}

fn retain_template_source_range(
    range: Range<usize>,
    scoped: &mut Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>> {
    if let Some((operation, parent)) = scoped {
        Ok(Some(Arc::new(operation.compose_sources(&[(
            Some(*parent),
            range,
            &[],
        )])?)))
    } else {
        Ok(None)
    }
}

pub(super) fn prepare_template_source_programs_in(
    source: &StorySource<'_>,
    mut scoped: Option<(
        &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        &oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<Vec<crate::template::TemplateProgram>> {
    use crate::template::{TemplateItemKind, TemplateProgram, TemplateProgramItem};
    let mut roots = Vec::new();
    let owners = scan_story_owners(&source.xml, source.root_kind)?;
    let scopes =
        story_namespace_scopes_at(&source.xml, owners.iter().map(|owner| owner.full.start))?;
    let mut inventories = Vec::new();
    let mut template_fields = Vec::new();
    let mut qualified = Vec::new();
    for owner in &owners {
        let scope = scopes
            .get(&owner.full.start)
            .ok_or_else(|| Error::Other("template owner scope missing".into()))?;
        let mut ranges = Vec::new();
        inventories.push(scan_story_items_with_scope(
            &source.xml,
            owner,
            scope,
            Some(&mut ranges),
        )?);
        ranges.extend(
            inventories
                .last()
                .unwrap()
                .iter()
                .filter(|item| item.kind == StoryItemKind::Field)
                .map(|item| item.full.clone()),
        );
        ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        let mut merged = Vec::<Range<usize>>::new();
        for range in ranges {
            if let Some(last) = merged.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        template_fields.push(merged);
        qualified.push((
            StoryId {
                kind: owner.kind,
                part_name: source.part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            },
            owner.clone(),
        ));
    }
    let paragraphs = story_range_paragraph_spans(&source.xml, &qualified)?;
    let paragraph_locations = paragraphs
        .into_iter()
        .flatten()
        .map(|(location, range)| (range.start, location))
        .collect::<HashMap<_, _>>();
    let mut programs = Vec::<TemplateProgram>::new();
    let mut suppressed_aliases = BTreeMap::<usize, usize>::new();
    let mut active = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        if suppressed_aliases
            .range(..=owner.full.start)
            .next_back()
            .is_some_and(|(_, end)| owner.full.end <= *end)
        {
            continue;
        }
        let paired = if owner.kind == StoryKind::TextBox {
            paired_scoped_ranges(&source.xml, owner, &owner.full, None)?
        } else {
            None
        };
        let mut ranges = vec![owner.full.clone()];
        for range in paired
            .into_iter()
            .flatten()
            .filter(|range| *range != owner.full)
        {
            suppressed_aliases.insert(range.start, range.end);
            ranges.push(range);
        }
        active.push(index);
        programs.push(TemplateProgram {
            story: qualified[index].0.clone(),
            part: source.part_name.clone(),
            ranges,
            raw: copy_template_source_range(source, owner.full.clone(), &mut scoped)?,
            source: retain_template_source_range(owner.full.clone(), &mut scoped)?,
            scope: scopes[&owner.full.start].clone(),
            required_paragraph: owner.kind == StoryKind::TableCell,
            table_rows: false,
            items: Vec::new(),
        });
    }
    let table_children = template_direct_table_children(&source.xml)?;
    let mut item_scopes = story_namespace_scopes_at(
        &source.xml,
        inventories
            .iter()
            .flatten()
            .map(|item| item.full.start)
            .chain(
                table_children
                    .values()
                    .flatten()
                    .map(|(range, _)| range.start),
            ),
    )?;
    // Only the existing admitted block-control slots introduce another
    // sibling grammar container. Inline wrappers retain mapper semantics.
    for &index in &active {
        for item in &inventories[index] {
            if item.kind == StoryItemKind::ContentControl
                && item.sdt_context == Some(StorySdtContext::Block)
            {
                programs.push(TemplateProgram {
                    story: qualified[index].0.clone(),
                    part: source.part_name.clone(),
                    ranges: vec![item.full.clone()],
                    raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                    source: retain_template_source_range(item.full.clone(), &mut scoped)?,
                    scope: item_scopes[&item.full.start].clone(),
                    required_paragraph: false,
                    table_rows: false,
                    items: Vec::new(),
                });
            }
        }
    }
    let mut prepared = Vec::new();
    for &owner_index in &active {
        let story = &qualified[owner_index].0;
        // A block control is its own grammar container. Its direct paragraphs
        // are not story items of the enclosing owner, so the container would
        // otherwise publish with no scalar occurrences.
        let mut items = inventories[owner_index].clone();
        let mut extra_starts = Vec::new();
        for item in &inventories[owner_index] {
            if item.kind != StoryItemKind::ContentControl
                || item.sdt_context != Some(StorySdtContext::Block)
            {
                continue;
            }
            let scope = item_scopes
                .get(&item.full.start)
                .ok_or_else(|| Error::Other("template block control scope missing".to_owned()))?;
            for paragraph in scan_story_control_paragraphs_with_scope(&source.xml, item, scope)? {
                if items.iter().any(|existing| existing.full == paragraph) {
                    continue;
                }
                extra_starts.push(paragraph.start);
                items.push(StoryItemSpan {
                    kind: StoryItemKind::Paragraph,
                    full: paragraph.clone(),
                    scan: paragraph,
                    direct_owner_child: false,
                    complex_field: false,
                    complex_ancestors: Vec::new(),
                    sdt_context: None,
                });
            }
        }
        if !extra_starts.is_empty() {
            item_scopes.extend(story_namespace_scopes_at(&source.xml, extra_starts)?);
        }
        items.sort_by_key(|item| (item.full.start, item.full.end));
        let fields = &template_fields[owner_index];
        let mut blocked_until = 0;
        for (index, item) in items.iter().enumerate() {
            if item.full.start < blocked_until {
                continue;
            }
            if !matches!(
                item.kind,
                StoryItemKind::Paragraph
                    | StoryItemKind::Table
                    | StoryItemKind::ContentControl
                    | StoryItemKind::PreservedNode
            ) {
                continue;
            }
            if item.kind == StoryItemKind::ContentControl
                && item.sdt_context != Some(StorySdtContext::Block)
            {
                continue;
            }
            if matches!(item.kind, StoryItemKind::Paragraph | StoryItemKind::Table) {
                blocked_until = item.full.end;
            }
            let location = paragraph_locations
                .get(&item.full.start)
                .cloned()
                .unwrap_or_else(|| ContentLocation::new(story.clone(), item.kind, vec![index]));
            let field_owned = item.complex_field
                || !item.complex_ancestors.is_empty()
                || fields
                    .get(fields.partition_point(|range| range.end <= item.full.start))
                    .is_some_and(|range| range.start < item.full.end);
            if item.kind == StoryItemKind::Table {
                let rows = table_children.get(&item.full.start).ok_or_else(|| {
                    Error::Other("template table lacks its admitted physical projection".into())
                })?;
                let closed = if let Some((operation, parent)) = &mut scoped {
                    let selected =
                        operation.compose_sources(&[(Some(*parent), item.full.clone(), &[])])?;
                    let closed = operation
                        .close_source_namespaces(selected, &item_scopes[&item.full.start])?;
                    operation.finish_source(closed)?
                } else {
                    close_content_fragment_namespaces(
                        &source.xml[item.full.clone()],
                        &item_scopes[&item.full.start].clone(),
                    )?
                };
                let table = CT_Tbl::from_xml_fragment_checked(&closed)?;
                if rows.iter().filter(|(_, row)| *row).count() != table.rows.len() {
                    return Err(Error::Other(
                        "template direct row source does not match the existing table model".into(),
                    ));
                }
                let mut row_index = 0;
                let mut row_items = Vec::new();
                for (range, is_row) in rows {
                    let marker_unsafe = !*is_row
                        || table.extra_xml.iter().any(|(at, _)| *at == row_index)
                        || table
                            .content_controls
                            .iter()
                            .any(|(at, _, _)| *at == row_index);
                    row_items.push(TemplateProgramItem {
                        location: location.clone(),
                        range: range.clone(),
                        kind: if *is_row {
                            TemplateItemKind::TableRow
                        } else {
                            TemplateItemKind::Source(StoryItemKind::PreservedNode)
                        },
                        field_owned,
                        marker_unsafe,
                        raw: copy_template_source_range(source, range.clone(), &mut scoped)?,
                        scope: item_scopes[&range.start].clone(),
                        children: Vec::new(),
                    });
                    row_index += usize::from(*is_row);
                }
                programs.push(TemplateProgram {
                    story: story.clone(),
                    part: source.part_name.clone(),
                    ranges: vec![item.full.clone()],
                    raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                    source: retain_template_source_range(item.full.clone(), &mut scoped)?,
                    scope: item_scopes[&item.full.start].clone(),
                    required_paragraph: false,
                    table_rows: true,
                    items: Vec::new(),
                });
                prepared.extend(row_items);
            }
            prepared.push(TemplateProgramItem {
                location,
                range: item.full.clone(),
                kind: TemplateItemKind::Source(item.kind),
                field_owned,
                marker_unsafe: false,
                raw: copy_template_source_range(source, item.full.clone(), &mut scoped)?,
                scope: item_scopes[&item.full.start].clone(),
                children: Vec::new(),
            });
        }
    }
    // The existing physical spans are laminar. Two ordered containment
    // sweeps join items to containers and containers to enclosing items,
    // without searching every owner for every paragraph.
    let mut order = (0..programs.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| {
        (
            programs[*index].ranges[0].start,
            std::cmp::Reverse(programs[*index].ranges[0].end),
        )
    });
    prepared.sort_by_key(|item| (item.range.start, std::cmp::Reverse(item.range.end)));
    let mut stack: Vec<usize> = Vec::new();
    let mut cursor = 0;
    let mut item_slots = Vec::new();
    for item in prepared {
        while let Some(&index) = order
            .get(cursor)
            .filter(|index| programs[**index].ranges[0].start <= item.range.start)
        {
            let span = &programs[index].ranges[0];
            while stack.last().is_some_and(|parent| {
                programs[*parent].ranges[0].end < span.end
                    || programs[*parent].ranges[0].end <= span.start
            }) {
                stack.pop();
            }
            stack.push(index);
            cursor += 1;
        }
        while stack.last().is_some_and(|index| {
            programs[*index].ranges[0].end < item.range.end
                || programs[*index].ranges[0].end <= item.range.start
        }) {
            stack.pop();
        }
        let container = stack
            .iter()
            .rev()
            .find(|index| programs[**index].ranges[0] != item.range)
            .copied()
            .ok_or_else(|| {
                Error::Other("template item has no qualified original container".into())
            })?;
        let slot = programs[container].items.len();
        item_slots.push((item.range.clone(), container, slot));
        programs[container].items.push(item);
    }
    item_slots.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut parents = vec![None; programs.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut cursor = 0;
    for &index in &order {
        let span = &programs[index].ranges[0];
        while let Some((range, _, _)) = item_slots
            .get(cursor)
            .filter(|(range, _, _)| range.start <= span.start)
        {
            while stack.last().is_some_and(|parent| {
                item_slots[*parent].0.end < range.end || item_slots[*parent].0.end <= range.start
            }) {
                stack.pop();
            }
            stack.push(cursor);
            cursor += 1;
        }
        while stack.last().is_some_and(|item| {
            item_slots[*item].0.end < span.end || item_slots[*item].0.end <= span.start
        }) {
            stack.pop();
        }
        if let Some(&item) = stack.last() {
            parents[index] = Some((item_slots[item].1, item_slots[item].2));
        }
    }
    order.sort_by_key(|index| programs[*index].ranges[0].len());
    let mut programs = programs.into_iter().map(Some).collect::<Vec<_>>();
    let mut source_roots = Vec::new();
    for index in order {
        let mut program = programs[index]
            .take()
            .ok_or_else(|| Error::Other("template grammar program was consumed twice".into()))?;
        for item in &mut program.items {
            item.children.sort_by_key(|child| child.ranges[0].start);
        }
        if let Some((parent, slot)) = parents[index] {
            programs[parent]
                .as_mut()
                .ok_or_else(|| {
                    Error::Other("template parent was consumed before its child".into())
                })?
                .items[slot]
                .children
                .push(program);
        } else {
            source_roots.push(program);
        }
    }
    source_roots.sort_by_key(|program| program.ranges[0].start);
    roots.extend(source_roots);
    Ok(roots)
}

pub(super) fn paired_scoped_ranges(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    span: &Range<usize>,
    mut operation: Option<&mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>>,
) -> Result<Option<Vec<Range<usize>>>> {
    let copies = scoped_alternate_text_box_copies(xml)?;
    let selected = copies
        .iter()
        .filter(|copy| copy.range.start <= owner.full.start && owner.full.end <= copy.range.end)
        .max_by_key(|copy| copy.alternate);
    let Some(selected) = selected else {
        return Ok(None);
    };
    let mut branches = BTreeMap::<usize, Vec<&PairedTextBoxCopy>>::new();
    for copy in &copies {
        if copy.alternate == selected.alternate {
            branches.entry(copy.branch).or_default().push(copy);
        }
    }
    if branches.len() < 2 {
        return Ok(None);
    }
    let selected_branch = &branches[&selected.branch];
    let selected_slot = selected_branch
        .iter()
        .position(|copy| copy.range == selected.range)
        .ok_or_else(|| Error::Other("paired selected box disappeared".into()))?;
    let baseline = selected_branch
        .iter()
        .map(|copy| paired_text_box_projection(xml, copy.range.clone(), operation.as_deref_mut()))
        .collect::<Result<Vec<_>>>()?;
    let selected_signature = &baseline[selected_slot].signature;
    if selected_branch.len() > 1
        && baseline
            .iter()
            .filter(|projection| &projection.signature == selected_signature)
            .count()
            != 1
    {
        return Err(Error::Other(
            "indistinguishable sibling text boxes cannot be paired".into(),
        ));
    }
    let path = baseline[selected_slot]
        .slots
        .iter()
        .find(|(_, range)| range == span)
        .map(|(path, _)| path.clone())
        .ok_or_else(|| Error::Other("selected paired content has no structural slot".into()))?;
    let mut ranges = Vec::new();
    for branch in branches.values() {
        if branch.len() != baseline.len() {
            return Err(Error::Other(
                "paired text-box branches have unequal cardinality".into(),
            ));
        }
        let projections = branch
            .iter()
            .map(|copy| {
                paired_text_box_projection(xml, copy.range.clone(), operation.as_deref_mut())
            })
            .collect::<Result<Vec<_>>>()?;
        if projections
            .iter()
            .zip(&baseline)
            .any(|(actual, expected)| actual.signature != expected.signature)
            || (branch.len() > 1
                && projections
                    .iter()
                    .filter(|p| &p.signature == selected_signature)
                    .count()
                    != 1)
        {
            return Err(Error::Other(
                "paired alternate text-box topology or accepted text is asymmetric".into(),
            ));
        }
        let range = projections[selected_slot]
            .slots
            .iter()
            .find(|(actual_path, _)| actual_path == &path)
            .map(|(_, range)| range.clone())
            .ok_or_else(|| Error::Other("paired content structural slot is absent".into()))?;
        ranges.push(range);
    }
    ranges.sort_by_key(|range| range.start);
    if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other("paired replacement sources overlap".into()));
    }
    Ok(Some(ranges))
}

pub(super) fn paired_scoped_edits(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    replacement: &PairedReplacement<'_>,
    selected_raw: &mut Vec<u8>,
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
) -> Result<Option<Vec<PairedTextEdit>>> {
    let PairedReplacement {
        span,
        paragraph,
        placeholder,
        replacement,
        literal_input,
        count,
    } = *replacement;
    let Some(ranges) = paired_scoped_ranges(xml, owner, span, Some(operation))? else {
        return Ok(None);
    };
    let mut edits = Vec::new();
    for range in ranges {
        if range == *span {
            edits.push(PairedTextEdit {
                range,
                raw: std::mem::take(selected_raw),
            });
            continue;
        }
        let scope = story_namespace_scope_at(xml, range.start)?;
        let counterpart = operation.replace_block_with_literal_input(
            range.clone(),
            &scope,
            paragraph,
            placeholder,
            replacement,
            literal_input,
        );
        let (raw, actual_count) = counterpart?;
        let raw = without_inherited_root_declarations(&xml[range.clone()], raw, &scope);
        if actual_count != count {
            return Err(Error::Other(
                "paired replacement physical counts are unequal".into(),
            ));
        }
        edits.push(PairedTextEdit { range, raw });
    }
    Ok(Some(edits))
}
