//! Content fragments inserted into a story: checking their block content, controls, identities and relationships, then serializing and inserting them.

use super::*;
use oxoffice_core::xml::is_xml_1_0_character;

pub(crate) fn validate_fragment_block_content(content: &[u8]) -> Result<()> {
    let mut xml = format!(
        r#"<w:document xmlns:w="{}"><w:body>"#,
        oxoffice_docx_oxml::namespace::W_NS
    )
    .into_bytes();
    xml.extend_from_slice(content);
    xml.extend_from_slice(b"</w:body></w:document>");
    let owner = scan_story_owners(&xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("fragment content has no body owner".to_owned()))?;
    for item in direct_story_content_items(&xml, &owner)? {
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let item_xml = close_content_fragment_namespaces(&xml[item.full], &scope)?;
        validate_fragment_block_item(&item_xml)?;
    }
    Ok(())
}

pub(super) fn validate_fragment_block_item(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    if let (namespace, Event::Start(element) | Event::Empty(element)) = reader
        .read_resolved_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid document fragment item: {error}")))?
        && word_element(&namespace)
    {
        match element.local_name().as_ref() {
            b"sdt" => validate_serialized_block_content_control(xml)?,
            b"r" | b"hyperlink" | b"fldSimple" | b"drawing" | b"pict" => {
                return Err(Error::Other(
                    "document fragment boundaries admit block content only".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn story_owner_content_end(xml: &[u8], owner: &StoryOwnerSpan) -> Result<usize> {
    let items = if owner.kind == StoryKind::Body {
        scan_story_items(xml, owner, None)?
    } else {
        Vec::new()
    };
    story_owner_content_end_in(xml, owner, &items)
}

pub(super) fn story_owner_content_end_in(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    items: &[StoryItemSpan],
) -> Result<usize> {
    if owner.kind == StoryKind::Body {
        for item in items {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, item)?
            {
                return Ok(item.full.start);
            }
        }
    }
    let mut reader = quick_xml::Reader::from_reader(&xml[owner.full.clone()]);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story owner boundary scan failed: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return (owner.full.end >= owner.full.start + 2)
                    .then_some(owner.full.end - 2)
                    .ok_or_else(|| Error::Other("empty story owner is incomplete".to_owned()));
            }
            Event::End(_) if depth == 1 => return Ok(owner.full.start + before),
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => {
                return Err(Error::Other(
                    "story owner has no closing boundary".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn package_authoritative_body_fragment(
    xml: &[u8],
    include_final_section_properties: bool,
    destination_scope: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let owner = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("document fragment has no main body".to_owned()))?;
    let mut output = Vec::new();
    let items = direct_story_content_items(xml, &owner)?;
    let mut cursor = items
        .first()
        .map(|item| item.full.start)
        .ok_or_else(|| Error::Other("document fragment main body is empty".to_owned()))?;
    for item in items {
        output.extend_from_slice(&xml[cursor..item.full.start]);
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        output.extend(close_content_fragment_namespaces(
            &xml[item.full.clone()],
            &required,
        )?);
        cursor = item.full.end;
    }
    output.extend_from_slice(&xml[cursor..story_owner_content_end(xml, &owner)?]);
    if include_final_section_properties {
        let mut section = None;
        for item in scan_story_items(xml, &owner, None)? {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, &item)?
            {
                section = Some(item);
                break;
            }
        }
        let section = section.ok_or_else(|| {
            Error::Other("section-inclusive document fragment has no final section".to_owned())
        })?;
        let scope = story_namespace_scope_at(xml, section.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let section = close_content_fragment_namespaces(&xml[section.full], &required)?;
        output.extend_from_slice(format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:pPr>"#).as_bytes());
        output.extend(section);
        output.extend_from_slice(b"</w:pPr></w:p>");
    }
    Ok(output)
}

pub(crate) fn package_authoritative_body_source(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: &oxoffice_docx_oxml::document::ScopedWordSource,
    include_final_section_properties: bool,
    destination_scope: &BTreeMap<String, String>,
) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
    // The same package-authoritative selectors and namespace policy as the
    // ordinary byte producer above, retaining their actual selected ranges.
    let xml = source.as_bytes();
    let owner = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("document fragment has no main body".into()))?;
    let items = direct_story_content_items(xml, &owner)?;
    let mut cursor = items
        .first()
        .map(|item| item.full.start)
        .ok_or_else(|| Error::Other("document fragment main body is empty".into()))?;
    let mut output = Vec::new();
    for item in items {
        output.push(operation.compose_sources(&[(Some(source), cursor..item.full.start, &[])])?);
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let selected = operation.compose_sources(&[(Some(source), item.full.clone(), &[])])?;
        output.push(operation.close_source_namespaces(selected, &required)?);
        cursor = item.full.end;
    }
    output.push(operation.compose_sources(&[(
        Some(source),
        cursor..story_owner_content_end(xml, &owner)?,
        &[],
    )])?);
    if include_final_section_properties {
        let mut section = None;
        for item in scan_story_items(xml, &owner, None)? {
            if item.direct_owner_child
                && item.kind == StoryItemKind::PreservedNode
                && content_fragment_root_is_section_properties(xml, &item)?
            {
                section = Some(item);
                break;
            }
        }
        let section = section.ok_or_else(|| {
            Error::Other("section-inclusive document fragment has no final section".into())
        })?;
        let scope = story_namespace_scope_at(xml, section.full.start)?;
        let required = scope
            .into_iter()
            .filter(|(prefix, namespace)| destination_scope.get(prefix) != Some(namespace))
            .collect();
        let selected = operation.compose_sources(&[(Some(source), section.full, &[])])?;
        let selected = operation.close_source_namespaces(selected, &required)?;
        let open = format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:pPr>"#);
        output.push(operation.compose_sources(&[
            (None, 0..0, open.as_bytes()),
            (Some(&selected), 0..selected.as_bytes().len(), &[]),
            (None, 0..0, b"</w:pPr></w:p>"),
        ])?);
    }
    let pieces = output
        .iter()
        .map(|part| (Some(part), 0..part.as_bytes().len(), &[][..]))
        .collect::<Vec<_>>();
    Ok(operation.compose_sources(&pieces)?)
}

/// Whether a fragment uses the `w14` prefix: in a qualified name, as the
/// part root declaration finds it, or in a markup compatibility attribute
/// that lists prefixes or qualified names.
pub(super) fn fragment_uses_w14(xml: &[u8]) -> Result<bool> {
    let named = xml.windows(5).any(|window| {
        matches!(window[0], b'<' | b'/' | b' ' | b'\t' | b'\r' | b'\n') && &window[1..] == b"w14:"
    });
    if named {
        return Ok(true);
    }
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("fragment scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("fragment attribute scan failed: {error}"))
                    })?;
                    let compatibility = matches!(
                        attribute.key.local_name().as_ref(),
                        b"Requires"
                            | b"Ignorable"
                            | b"ProcessContent"
                            | b"MustUnderstand"
                            | b"PreserveElements"
                            | b"PreserveAttributes"
                    );
                    if compatibility
                        && attribute
                            .value
                            .split(|byte| byte.is_ascii_whitespace())
                            .any(|token| token == b"w14" || token.starts_with(b"w14:"))
                    {
                        return Ok(true);
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

/// The body item span of a part written by
/// `CT_Document::standalone_body_item_xml`.
pub(super) fn standalone_body_item_span(part: &[u8]) -> Result<(usize, usize)> {
    // Attribute values escape `<`, so the first body tag follows the root.
    let start = part
        .windows(b"<w:body>".len())
        .position(|window| window == b"<w:body>")
        .map(|body| body + b"<w:body>".len())
        .ok_or_else(|| Error::Other("standalone body item has no body".into()))?;
    let end = part
        .len()
        .checked_sub(b"</w:body></w:document>".len())
        .filter(|end| start <= *end)
        .ok_or_else(|| Error::Other("standalone body item is incomplete".into()))?;
    Ok((start, end))
}

/// The physical cell owner of `(row, cell)` in the table `item` of `xml`.
pub(super) fn physical_table_cell_owner(
    xml: &[u8],
    root_kind: StoryKind,
    item: &StoryItemSpan,
    row: usize,
    cell: usize,
) -> Result<StoryOwnerSpan> {
    let table = Document::story_table_from_source(xml, item)?;
    TableRef { inner: &table }
        .cell(row, cell)
        .ok_or_else(|| Error::Other("rich table cell index is out of range".to_owned()))?;
    let mut owners = scan_story_owners(xml, root_kind)?
        .into_iter()
        .filter(|owner| {
            owner.kind == StoryKind::TableCell
                && item.full.start < owner.full.start
                && owner.full.end < item.full.end
        })
        .collect::<Vec<_>>();
    let (_, modeled) = physical_table_cells(&table);
    if modeled.len() < owners.len() {
        return Err(Error::Other(
            "rich table cell source correspondence is incomplete".into(),
        ));
    }
    if modeled.len() > owners.len() {
        return Err(Error::Other(
            "rich table has unqualified modeled cells".into(),
        ));
    }
    let ordinal = modeled
        .iter()
        .position(|(table, coordinate)| *table == 0 && *coordinate == Some((row, cell)))
        .ok_or_else(|| Error::Other("rich table cell has no unique physical owner".into()))?;
    Ok(owners.swap_remove(ordinal))
}

pub(crate) fn close_content_fragment_namespaces(
    xml: &[u8],
    scope: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let closed = oxoffice_docx_oxml::document::close_content_fragment_namespaces(xml, scope)
        .map_err(|error| match error {
            oxoffice_docx_oxml::OxmlError::InvalidValue(message) => Error::Other(message),
            other => Error::from(other),
        })?;
    Ok(closed)
}

pub(super) fn reject_section_owning_content_fragment(fragment: &ContentFragment) -> Result<()> {
    let xml = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
    let mut wrapped = format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
    wrapped.extend_from_slice(&xml);
    wrapped.extend_from_slice(b"</w:body></w:document>");
    let document = CT_Document::from_xml(&wrapped)?;
    let mut owns_section = false;
    visit_body_paragraphs(&document.body.content, false, &mut |paragraph| {
        owns_section |= paragraph
            .properties
            .as_ref()
            .is_some_and(|properties| properties.sect_pr.is_some());
    });
    if owns_section {
        return Err(Error::Other(
            "generic content operations cannot mutate a section-owning paragraph".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn serialize_content_fragment(content: BodyContent) -> Result<Vec<u8>> {
    let mut document = CT_Document::new();
    document.body.content = vec![content];
    document.body.sect_pr = None;
    let xml = document.to_xml()?;
    let owner = scan_story_owners(&xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("serialized content fragment has no body owner".to_owned()))?;
    let items = direct_story_content_items(&xml, &owner)?;
    let [item] = items.as_slice() else {
        return Err(Error::Other(
            "serialized content fragment must contain exactly one direct item".to_owned(),
        ));
    };
    let scope = story_namespace_scope_at(&xml, item.full.start)?;
    close_content_fragment_namespaces(&xml[item.full.clone()], &scope)
}

const BLOCK_SDT_PROPERTIES_SLOT: u8 = 1;
const BLOCK_SDT_END_PROPERTIES_SLOT: u8 = 2;
const BLOCK_SDT_CONTENT_SLOT: u8 = 4;

pub(super) fn validate_serialized_block_content_control(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut controls: Vec<(usize, Option<usize>, u8, u8)> = Vec::new();

    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(element) => {
                validate_block_control_element(
                    &namespace,
                    &element,
                    depth,
                    false,
                    &mut root_seen,
                    &mut controls,
                )?;
                depth += 1;
            }
            Event::Empty(element) => validate_block_control_element(
                &namespace,
                &element,
                depth,
                true,
                &mut root_seen,
                &mut controls,
            )?,
            Event::End(element) => {
                if depth == 0 {
                    return Err(Error::Other(
                        "block content control has an unmatched end element".to_owned(),
                    ));
                }
                if controls.last().is_some_and(|control| {
                    control.0 == depth
                        && word_element(&namespace)
                        && matches_local_name(element.name().as_ref(), b"sdt")
                }) {
                    controls.pop();
                }
                depth -= 1;
            }
            Event::Text(text) => {
                let bytes: &[u8] = text.as_ref();
                if block_control_text_is_direct(depth, &controls)
                    && !block_control_bytes_are_xml_whitespace(bytes)?
                {
                    return Err(Error::Other(
                        "block content control has direct non-whitespace text".to_owned(),
                    ));
                }
            }
            Event::CData(text) => {
                let bytes: &[u8] = text.as_ref();
                if block_control_text_is_direct(depth, &controls)
                    && !block_control_bytes_are_xml_whitespace(bytes)?
                {
                    return Err(Error::Other(
                        "block content control has direct non-whitespace CDATA".to_owned(),
                    ));
                }
            }
            Event::GeneralRef(reference) if block_control_text_is_direct(depth, &controls) => {
                if !block_control_reference_is_whitespace(&reference)? {
                    return Err(Error::Other(
                        "block content control has a direct non-whitespace reference".to_owned(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    if !root_seen || depth != 0 || !controls.is_empty() {
        return Err(Error::Other(
            "block content control is structurally incomplete".to_owned(),
        ));
    }
    Ok(())
}

fn block_control_reference_is_whitespace(
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<bool> {
    if let Some(character) = reference.resolve_char_ref().map_err(|error| {
        Error::Other(format!(
            "block content control has an invalid character reference: {error}"
        ))
    })? {
        if !is_xml_1_0_character(character) {
            return Err(Error::Other(
                "block content control has an XML 1.0-forbidden character reference".to_owned(),
            ));
        }
        return Ok(xml_1_0_whitespace(character));
    }
    let name = reference.decode().map_err(|error| {
        Error::Other(format!(
            "block content control has an invalid entity reference: {error}"
        ))
    })?;
    let value = resolve_xml_entity(&name).ok_or_else(|| {
        Error::Other(format!(
            "block content control has an unresolved entity reference: &{name};"
        ))
    })?;
    if !value.chars().all(is_xml_1_0_character) {
        return Err(Error::Other(format!(
            "block content control entity reference contains an XML 1.0-forbidden character: &{name};"
        )));
    }
    Ok(value.chars().all(xml_1_0_whitespace))
}

fn block_control_bytes_are_xml_whitespace(bytes: &[u8]) -> Result<bool> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        Error::Other(format!(
            "block content control has invalid UTF-8 text: {error}"
        ))
    })?;
    Ok(text.chars().all(xml_1_0_whitespace))
}

pub(super) fn xml_1_0_whitespace(character: char) -> bool {
    matches!(character, '\u{20}' | '\u{9}' | '\u{A}' | '\u{D}')
}

fn validate_block_control_element(
    namespace: &ResolveResult<'_>,
    element: &BytesStart<'_>,
    parent_depth: usize,
    empty: bool,
    root_seen: &mut bool,
    controls: &mut Vec<(usize, Option<usize>, u8, u8)>,
) -> Result<()> {
    let is_word = word_element(namespace);
    let name = element.name();
    let local = name.as_ref();

    if controls.is_empty() {
        if *root_seen || !is_word || !matches_local_name(local, b"sdt") {
            return Err(Error::Other(
                "block content control must contain exactly one Word sdt root".to_owned(),
            ));
        }
        *root_seen = true;
        if !empty {
            controls.push((parent_depth + 1, None, 0, 0));
        }
        return Ok(());
    }

    let (root_depth, content_depth) = {
        let control = controls
            .last()
            .expect("active block content control was checked above");
        (control.0, control.1)
    };
    if parent_depth == root_depth {
        if !is_word {
            return Ok(());
        }
        let (slot, order) = if matches_local_name(local, b"sdtPr") {
            (BLOCK_SDT_PROPERTIES_SLOT, 0)
        } else if matches_local_name(local, b"sdtEndPr") {
            (BLOCK_SDT_END_PROPERTIES_SLOT, 1)
        } else if matches_local_name(local, b"sdtContent") {
            (BLOCK_SDT_CONTENT_SLOT, 2)
        } else {
            return Err(Error::Other(
                "block content control has an invalid Word root child".to_owned(),
            ));
        };
        let control = controls
            .last_mut()
            .expect("active block content control was checked above");
        if control.3 & slot != 0 || control.2 > order {
            return Err(Error::Other(
                "block content control root children are duplicated or out of order".to_owned(),
            ));
        }
        control.2 = order;
        control.3 |= slot;
        if slot == BLOCK_SDT_CONTENT_SLOT && !empty {
            control.1 = Some(parent_depth + 1);
        }
        return Ok(());
    }

    if content_depth != Some(parent_depth) {
        return Ok(());
    }
    if !is_word {
        return Ok(());
    }
    if matches_local_name(local, b"p") || matches_local_name(local, b"tbl") {
        return Ok(());
    }
    if matches_local_name(local, b"sdt") {
        if !empty {
            controls.push((parent_depth + 1, None, 0, 0));
        }
        return Ok(());
    }
    Err(Error::Other(
        "block content control has a non-block Word content child".to_owned(),
    ))
}

fn block_control_text_is_direct(depth: usize, controls: &[(usize, Option<usize>, u8, u8)]) -> bool {
    controls
        .last()
        .is_some_and(|control| depth == control.0 || Some(depth) == control.1)
}

pub(super) fn validated_direct_content_item(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<StoryItemSpan> {
    if location.index_path.len() != 1 {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let items = scan_story_items(xml, owner, None)?;
    let index = location.index_path[0];
    let item = items.get(index).cloned().ok_or(StoryError::OutOfBounds {
        index,
        len: items.len(),
    })?;
    if item.kind != location.item_kind {
        return Err(StoryError::KindMismatch {
            expected: location.item_kind,
            actual: item.kind,
        }
        .into());
    }
    if !item.direct_owner_child
        || owner.kind == StoryKind::Body && content_fragment_root_is_section_properties(xml, &item)?
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    Ok(item)
}

// Nested paragraph paths use the enclosing block control's content as the
// effective block owner while keeping the underlying part's relationship scope.
pub(super) fn fragment_content_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(StoryOwnerSpan, usize, usize, usize)> {
    if location.is_end || location.index_path.len() == 1 {
        let (boundary, index, count) = validated_content_boundary(xml, owner, location)?;
        return Ok((owner.clone(), boundary, index, count));
    }
    let [control_index, paragraph_index] = location.index_path.as_slice() else {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    };
    if location.item_kind != StoryItemKind::Paragraph {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let items = scan_story_items(xml, owner, None)?;
    let control = items.get(*control_index).ok_or(StoryError::OutOfBounds {
        index: *control_index,
        len: items.len(),
    })?;
    if control.kind != StoryItemKind::ContentControl
        || control.sdt_context != Some(StorySdtContext::Block)
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let paragraphs = scan_story_control_paragraphs(xml, control)?;
    let paragraph = paragraphs
        .get(*paragraph_index)
        .ok_or(StoryError::OutOfBounds {
            index: *paragraph_index,
            len: paragraphs.len(),
        })?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut enclosing = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("fragment control owner scan failed: {error}")))?
        {
            Event::Start(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                stack.push((
                    word_element(&namespace) && local.as_ref() == b"sdtContent",
                    before,
                ));
            }
            Event::End(_) => {
                if let Some((true, start)) = stack.pop() {
                    let end = reader.buffer_position() as usize;
                    if start >= control.full.start
                        && end <= control.full.end
                        && start < paragraph.start
                        && end > paragraph.end
                    {
                        enclosing.push(start..end);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let full = enclosing
        .into_iter()
        .min_by_key(|span| span.end - span.start)
        .ok_or_else(|| Error::Other("fragment paragraph has no block control owner".to_owned()))?;
    let effective = StoryOwnerSpan {
        full,
        ..owner.clone()
    };
    let direct = direct_story_content_items(xml, &effective)?;
    let index = direct
        .iter()
        .position(|item| item.kind == StoryItemKind::Paragraph && item.full == *paragraph)
        .ok_or_else(|| {
            Error::Other("fragment paragraph is not a direct block control child".to_owned())
        })?;
    Ok((effective, paragraph.start, index, direct.len()))
}

/// [`fragment_content_boundary`] for an insertion: before the bookmark
/// starts that lead the block at `location` (see `leading_bookmarks`).
pub(super) fn fragment_insertion_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(StoryOwnerSpan, usize, usize, usize)> {
    let (owner, boundary, index, count) = fragment_content_boundary(xml, owner, location)?;
    let (boundary, index) = widened_insertion_boundary(xml, &owner, boundary, index)?;
    Ok((owner, boundary, index, count))
}

/// [`validated_content_boundary`] for an insertion: before the bookmark
/// starts that lead the block at `location` (see `leading_bookmarks`).
pub(super) fn validated_insertion_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(usize, usize, usize)> {
    let (boundary, index, count) = validated_content_boundary(xml, owner, location)?;
    let (boundary, index) = widened_insertion_boundary(xml, owner, boundary, index)?;
    Ok((boundary, index, count))
}

fn widened_insertion_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    boundary: usize,
    index: usize,
) -> Result<(usize, usize)> {
    let items = direct_story_content_items(xml, owner)?;
    let first = leading_bookmark_start_item(xml, &items, index)?;
    Ok(if first == index {
        (boundary, index)
    } else {
        (items[first].full.start, first)
    })
}

pub(super) fn validated_content_boundary(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<(usize, usize, usize)> {
    let items = direct_story_content_items(xml, owner)?;
    if location.is_end {
        return Ok((
            story_owner_content_end(xml, owner)?,
            items.len(),
            items.len(),
        ));
    }
    if location.index_path.len() != 1 {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let projected = scan_story_items(xml, owner, None)?;
    let index = location.index_path[0];
    let item = projected.get(index).ok_or(StoryError::OutOfBounds {
        index,
        len: projected.len(),
    })?;
    if item.kind != location.item_kind {
        return Err(StoryError::KindMismatch {
            expected: location.item_kind,
            actual: item.kind,
        }
        .into());
    }
    if !item.direct_owner_child
        || owner.kind == StoryKind::Body && content_fragment_root_is_section_properties(xml, item)?
    {
        return Err(StoryError::InvalidPath {
            path: location.index_path.clone(),
        }
        .into());
    }
    let direct_index = items
        .iter()
        .position(|candidate| candidate.full == item.full)
        .ok_or_else(|| Error::Other("direct content destination was not found".to_owned()))?;
    Ok((item.full.start, direct_index, items.len()))
}

pub(super) fn insert_story_fragment(
    source: &mut Vec<u8>,
    owner: &StoryOwnerSpan,
    boundary: usize,
    fragment: Vec<u8>,
) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(&source[owner.full.clone()]);
    let mut buffer = Vec::new();
    let empty_name = loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story owner insertion scan failed: {error}")))?
        {
            Event::Start(_) => break None,
            Event::Empty(element) => break Some(element.name().as_ref().to_vec()),
            Event::Eof => {
                return Err(Error::Other(
                    "story owner has no insertion boundary".to_owned(),
                ));
            }
            _ => buffer.clear(),
        }
    };
    if let Some(name) = empty_name {
        if boundary + 2 != owner.full.end {
            return Err(Error::Other(
                "empty story owner insertion boundary is invalid".to_owned(),
            ));
        }
        let mut replacement = Vec::with_capacity(fragment.len() + name.len() + 4);
        replacement.push(b'>');
        replacement.extend(fragment);
        replacement.extend_from_slice(b"</");
        replacement.extend(name);
        replacement.push(b'>');
        source.splice(boundary..owner.full.end, replacement);
    } else {
        source.splice(boundary..boundary, fragment);
    }
    Ok(())
}

fn story_scope_matches_fragment(destination: &StoryId, fragment: &ContentFragment) -> bool {
    fragment.source_part_name.as_deref() == Some(destination.part_name.as_str())
        && fragment.source_story_kind == Some(destination.kind)
        && fragment.source_owner_index == Some(destination.owner_index)
}

/// Concrete typed owners used only while qualifying replacement XML.
/// Raw preservation payloads never acquire relationship type authority.
pub(super) enum WordXmlOwner<'a> {
    Paragraph(&'a CT_P),
    Run(&'a CT_R),
    Table(&'a CT_Tbl),
    Row(&'a CT_Row),
    Cell(&'a CT_Tc),
    Control(&'a CT_Sdt),
}

/// Qualify changed bookmark and comment identities through existing physical
/// source inventories. Unchanged selected identities retain their owner proof.
pub(super) fn validate_element_xml_identity_change(
    document: &Document,
    story: &StoryId,
    previous: &[u8],
    incoming: &[u8],
) -> Result<()> {
    let old_comments = crate::comments::comment_source_markers(previous)?;
    let new_comments = crate::comments::comment_source_markers(incoming)?;
    if !old_comments
        .iter()
        .map(|marker| (marker.id, marker.family))
        .eq(new_comments.iter().map(|marker| (marker.id, marker.family)))
    {
        let counts = |markers: &[crate::comments::CommentSourceMarker]| -> Result<BTreeMap<i32, [usize; 3]>> {
            let mut counts = BTreeMap::new();
            for marker in markers {
                let entry = counts.entry(marker.id).or_insert([0usize; 3]);
                entry[marker.family] = entry[marker.family].checked_add(1).ok_or_else(|| Error::Other("element XML comment count overflow".into()))?;
            }
            Ok(counts)
        };
        let before = counts(&old_comments)?;
        let after = counts(&new_comments)?;
        let mut old_order = BTreeMap::<i32, Vec<usize>>::new();
        let mut new_order = BTreeMap::<i32, Vec<usize>>::new();
        for marker in &old_comments {
            old_order.entry(marker.id).or_default().push(marker.family);
        }
        for marker in &new_comments {
            new_order.entry(marker.id).or_default().push(marker.family);
        }
        let changed = old_order
            .keys()
            .chain(new_order.keys())
            .copied()
            .filter(|id| old_order.get(id) != new_order.get(id))
            .collect::<HashSet<_>>();
        let mut prepared = document.clone_for_staging();
        prepared.flush_to_package()?;
        let inventory = prepared.comment_source_inventory_at(&prepared.doc_part_name)?;
        if !inventory.contains_key(&story.part_name) {
            return Err(Error::Other(
                "element XML comment relationship scope is unsupported".into(),
            ));
        }
        let mut total = BTreeMap::<i32, [usize; 3]>::new();
        for markers in inventory.values() {
            for (id, counts) in counts(markers)? {
                let entry = total.entry(id).or_default();
                for family in 0..3 {
                    entry[family] = entry[family].checked_add(counts[family]).ok_or_else(|| {
                        Error::Other("element XML comment inventory overflow".into())
                    })?;
                }
            }
        }
        for id in &changed {
            let id = *id;
            let old = before.get(&id).copied().unwrap_or_default();
            let new = after.get(&id).copied().unwrap_or_default();
            if new.iter().any(|count| *count > 1) || new[0] != new[1] {
                return Err(Error::Other(
                    "element XML changed comment span outside the selected owner is unsupported"
                        .into(),
                ));
            }
            if new != [0; 3] {
                prepared.fragment_comment_dependency(id)?;
            }
            let all = total.get(&id).copied().unwrap_or_default();
            let mut combined = [0usize; 3];
            for family in 0..3 {
                combined[family] = all[family]
                    .checked_sub(old[family])
                    .and_then(|outside| outside.checked_add(new[family]))
                    .ok_or_else(|| {
                        Error::Other("element XML comment owner correspondence is invalid".into())
                    })?;
            }
            if combined.iter().any(|count| *count > 1) || combined[0] != combined[1] {
                return Err(Error::Other(
                    "element XML comment ID collides or leaves an unmatched physical owner".into(),
                ));
            }
        }
        let mut started = HashSet::new();
        for marker in &new_comments {
            if !changed.contains(&marker.id) {
                continue;
            }
            if marker.family == 0 {
                started.insert(marker.id);
            } else if marker.family == 1 && !started.contains(&marker.id) {
                return Err(Error::Other(
                    "element XML comment end precedes its start".into(),
                ));
            }
        }
    }
    let counts = |markers: &[BookmarkMarkerSpan]| -> Result<BTreeMap<i32, [usize; 2]>> {
        let mut counts = BTreeMap::new();
        for marker in markers {
            let id = marker
                .id
                .parse::<i32>()
                .map_err(|_| Error::Other("element XML bookmark has an invalid ID".into()))?;
            let count = counts.entry(id).or_insert([0usize; 2]);
            let family = usize::from(!marker.start);
            count[family] = count[family]
                .checked_add(1)
                .ok_or_else(|| Error::Other("element XML bookmark count overflow".into()))?;
        }
        Ok(counts)
    };
    let previous_markers = bookmark_marker_spans(previous)?;
    let incoming_markers = bookmark_marker_spans(incoming)?;
    let before = counts(&previous_markers)?;
    let after = counts(&incoming_markers)?;
    let mut old_order = BTreeMap::<i32, Vec<bool>>::new();
    let mut new_order = BTreeMap::<i32, Vec<bool>>::new();
    for marker in &previous_markers {
        old_order
            .entry(marker.id.parse().unwrap())
            .or_default()
            .push(marker.start);
    }
    for marker in &incoming_markers {
        new_order
            .entry(marker.id.parse().unwrap())
            .or_default()
            .push(marker.start);
    }
    let changed = old_order
        .keys()
        .chain(new_order.keys())
        .copied()
        .filter(|id| old_order.get(id) != new_order.get(id))
        .collect::<HashSet<_>>();
    if changed.is_empty() {
        return Ok(());
    }
    let (source, _) = document.story_source_and_owner(story)?;
    let total = counts(&bookmark_marker_spans(&source.xml)?)?;
    for id in &changed {
        let id = *id;
        let old = before.get(&id).copied().unwrap_or_default();
        let new = after.get(&id).copied().unwrap_or_default();
        if new != [0, 0] && new != [1, 1] {
            return Err(Error::Other(
                "element XML changed bookmark span outside the selected owner is unsupported"
                    .into(),
            ));
        }
        let total = total.get(&id).copied().unwrap_or_default();
        let mut combined = [0usize; 2];
        for family in 0..2 {
            combined[family] = total[family]
                .checked_sub(old[family])
                .and_then(|outside| outside.checked_add(new[family]))
                .ok_or_else(|| {
                    Error::Other("element XML bookmark owner correspondence is invalid".into())
                })?;
        }
        if combined != [0, 0] && combined != [1, 1] {
            return Err(Error::Other(
                "element XML bookmark ID collides or leaves an unmatched physical owner".into(),
            ));
        }
    }
    let mut started = HashSet::new();
    for marker in &incoming_markers {
        let id = marker
            .id
            .parse::<i32>()
            .map_err(|_| Error::Other("element XML bookmark has an invalid ID".into()))?;
        if !changed.contains(&id) {
            continue;
        }
        if marker.start {
            started.insert(id);
        } else if !started.contains(&id) {
            return Err(Error::Other(
                "element XML bookmark end precedes its start".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_element_owner_relationships(
    document: &Document,
    story: &StoryId,
    root: WordXmlOwner<'_>,
) -> Result<()> {
    let mut pending = vec![root];
    while let Some(owner) = pending.pop() {
        match owner {
            WordXmlOwner::Paragraph(paragraph) => {
                for hyperlink in &paragraph.hyperlinks {
                    if let Some(id) = &hyperlink.rel_id {
                        document.hyperlink_url_for_story(story, id)?;
                    }
                }
                pending.extend(paragraph.runs.iter().map(WordXmlOwner::Run));
                pending.extend(
                    paragraph
                        .content_controls
                        .iter()
                        .map(|(_, _, _, control)| WordXmlOwner::Control(control)),
                );
                for (_, _, revision) in &paragraph.revisions {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordXmlOwner::Paragraph(paragraph));
                    }
                }
                for ruby in &paragraph.rubies {
                    pending.extend(ruby.ruby_text.iter().map(WordXmlOwner::Run));
                }
            }
            WordXmlOwner::Run(run) => {
                for content in &run.content {
                    match content {
                        RunContent::Field(field) => {
                            field.validate_for_attachment()?;
                            // A new whole raw owner has no retained-program authority.
                            oxoffice_docx_oxml::text::Field::from_raw(
                                &field.effective_instruction_text(),
                                field.form(),
                                Vec::new(),
                            )?;
                            let instruction = field.effective_instruction();
                            oxoffice_docx_oxml::text::FieldInstruction::new(
                                &instruction.name,
                                instruction.arguments,
                                instruction.switches,
                            )?;
                            if let Some(owner) = field.cached_result_owner_projection()? {
                                validate_element_owner_relationships(
                                    document,
                                    story,
                                    WordXmlOwner::Paragraph(&owner),
                                )?;
                            } else if let Some(runs) = field.cached_result_runs() {
                                pending.extend(runs.iter().map(WordXmlOwner::Run));
                            }
                        }
                        RunContent::FootnoteRef { id, .. } | RunContent::EndnoteRef { id, .. } => {
                            if document.glossary_part_name.as_deref()
                                == Some(story.part_name.as_str())
                            {
                                return Err(Error::Other(
                                    "element XML glossary note reference scope is unsupported"
                                        .into(),
                                ));
                            }
                            let kind = if matches!(content, RunContent::FootnoteRef { .. }) {
                                StoryKind::Footnote
                            } else {
                                StoryKind::Endnote
                            };
                            document.fragment_note_dependency(kind, *id)?;
                        }
                        RunContent::Drawing(drawing) => {
                            if drawing.inline.is_some() && drawing.anchor.is_some() {
                                return Err(Error::Other(
                                    "element XML drawing has two modeled owners".into(),
                                ));
                            }
                            let relationships = drawing
                                .inline
                                .as_ref()
                                .map(|inline| {
                                    (
                                        &inline.embed_id,
                                        inline.link_id.as_deref(),
                                        inline.chart_rel_id.as_deref(),
                                    )
                                })
                                .or_else(|| {
                                    drawing.anchor.as_ref().map(|anchor| {
                                        (
                                            &anchor.embed_id,
                                            anchor.link_id.as_deref(),
                                            anchor.chart_rel_id.as_deref(),
                                        )
                                    })
                                });
                            if let Some((embed, link, chart)) = relationships {
                                if !embed.is_empty() {
                                    let target = document
                                        .validate_internal_relationship_for_story(
                                            story,
                                            embed,
                                            rel_types::IMAGE,
                                        )?;
                                    if !document
                                        .package
                                        .content_types
                                        .content_type_for(&target)
                                        .is_some_and(|kind| kind.starts_with("image/"))
                                    {
                                        return Err(Error::Other("element XML image relationship targets a non-image part".into()));
                                    }
                                }
                                if let Some(id) = link {
                                    let relationship = document
                                        .package
                                        .get_part_rels(&story.part_name)
                                        .and_then(|relationships| relationships.get_by_id(id))
                                        .ok_or_else(|| {
                                            Error::Other(
                                                "element XML linked image relationship is missing"
                                                    .into(),
                                            )
                                        })?;
                                    if relationship.rel_type != rel_types::IMAGE
                                        || relationship.target_mode.as_deref() != Some("External")
                                    {
                                        return Err(Error::Other("element XML linked image requires an external image relationship".into()));
                                    }
                                }
                                if let Some(id) = chart {
                                    let target = document
                                        .validate_internal_relationship_for_story(
                                            story,
                                            id,
                                            rel_types::CHART,
                                        )?;
                                    let source =
                                        document.package.get_part(&target).ok_or_else(|| {
                                            Error::Other(
                                                "element XML chart target is missing".into(),
                                            )
                                        })?;
                                    if document.package.content_types.content_type_for(&target)
                                        != Some(content_types::CHART)
                                    {
                                        return Err(Error::Other("element XML chart relationship targets a non-chart part".into()));
                                    }
                                    CT_ChartSpace::from_xml(source).map_err(|error| {
                                        Error::Other(format!("element XML chart target: {error}"))
                                    })?;
                                }
                            }
                            if let Some(shape) = drawing
                                .anchor
                                .as_ref()
                                .and_then(|anchor| anchor.shape.as_ref())
                            {
                                if let Some(body) = &shape.text_body {
                                    for content in &body.content {
                                        match content {
                                            BodyContent::Paragraph(paragraph) => {
                                                pending.push(WordXmlOwner::Paragraph(paragraph))
                                            }
                                            BodyContent::Table(table) => {
                                                pending.push(WordXmlOwner::Table(table))
                                            }
                                            BodyContent::ContentControl(control) => {
                                                pending.push(WordXmlOwner::Control(control))
                                            }
                                            BodyContent::RawXml(_) => {}
                                        }
                                    }
                                } else {
                                    pending.extend(shape.text.iter().map(WordXmlOwner::Paragraph));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WordXmlOwner::Table(table) => {
                validate_table_topology(table)?;
                pending.extend(table.rows.iter().map(WordXmlOwner::Row));
                pending.extend(
                    table
                        .content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Row(row) => {
                pending.extend(row.cells.iter().map(WordXmlOwner::Cell));
                pending.extend(
                    row.content_controls
                        .iter()
                        .map(|(_, _, control)| WordXmlOwner::Control(control)),
                );
            }
            WordXmlOwner::Cell(cell) => {
                for content in &cell.content {
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            pending.push(WordXmlOwner::Paragraph(paragraph))
                        }
                        CellContent::Table(table) => pending.push(WordXmlOwner::Table(table)),
                        CellContent::ContentControl(control) => {
                            pending.push(WordXmlOwner::Control(control))
                        }
                    }
                }
            }
            WordXmlOwner::Control(control) => {
                for content in &control.content {
                    match content {
                        SdtContent::Paragraph(paragraph) => {
                            pending.push(WordXmlOwner::Paragraph(paragraph))
                        }
                        SdtContent::Run(run) => pending.push(WordXmlOwner::Run(run)),
                        SdtContent::Table(table) => pending.push(WordXmlOwner::Table(table)),
                        SdtContent::Row(row) => pending.push(WordXmlOwner::Row(row)),
                        SdtContent::Cell(cell) => pending.push(WordXmlOwner::Cell(cell)),
                        SdtContent::ContentControl(control) => {
                            pending.push(WordXmlOwner::Control(control))
                        }
                        SdtContent::RawXml(_) => {}
                    }
                }
                for (_, revision) in control.revisions() {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordXmlOwner::Paragraph(paragraph));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The relationship ids the fragment's XML names, in order.
fn fragment_relationship_ids(fragment: &ContentFragment) -> Result<Vec<String>> {
    let inherited = fragment
        .namespace_scope
        .iter()
        .map(|(prefix, namespace)| {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            (name, namespace.clone())
        })
        .collect::<Vec<_>>();
    xml_relationship_ids_in_order_with_bindings(&fragment.xml, &inherited)
}

/// The relationships of `story`'s part that the fragment names, as they are
/// now, for a fragment taken from that story, each internal one with a digest
/// of its target part's bytes.
pub(super) fn referenced_relationships(
    document: &Document,
    story: &StoryId,
    fragment: &ContentFragment,
) -> Result<Vec<(oxoffice_opc::relationship::Relationship, Option<u64>)>> {
    let ids = fragment_relationship_ids(fragment)?;
    let Some(relationships) = document.package.get_part_rels(&story.part_name) else {
        return Ok(Vec::new());
    };
    Ok(ids
        .iter()
        .filter_map(|id| relationships.get_by_id(id))
        .map(|relationship| {
            let digest = relationship_target_digest(document, &story.part_name, relationship);
            (relationship.clone(), digest)
        })
        .collect())
}

/// A digest of an internal relationship's target part, which a save may
/// rename but does not change.
fn relationship_target_digest(
    document: &Document,
    owner: &str,
    relationship: &oxoffice_opc::relationship::Relationship,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    if !relationship_is_internal(relationship) {
        return None;
    }
    let part = OpcPackage::resolve_rel_target(owner, &relationship.target);
    let bytes = document.package.get_part(&part)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

pub(super) fn validate_fragment_relationships(
    document: &Document,
    destination: &StoryId,
    fragment: &ContentFragment,
) -> Result<()> {
    let relationship_ids = fragment_relationship_ids(fragment)?;
    if relationship_ids.is_empty() {
        return Ok(());
    }
    if !story_scope_matches_fragment(destination, fragment) {
        return Err(Error::Other(
            "content fragment relationship references require the unchanged story owner".to_owned(),
        ));
    }
    let relationships = document
        .package
        .get_part_rels(&destination.part_name)
        .ok_or_else(|| {
            Error::Other(format!(
                "content fragment owner {} has no relationship scope",
                destination.part_name
            ))
        })?;
    for id in relationship_ids {
        let Some(relationship) = relationships.get_by_id(&id) else {
            // A taken fragment may come from another document; hand-written XML
            // names a wrong id.
            let hint = if fragment.relationships.is_empty() {
                ""
            } else {
                "; move content between documents with DocumentFragment::from_range and \
                 import_fragment"
            };
            return Err(Error::Other(format!(
                "content fragment references missing relationship {id} in {}{hint}",
                destination.part_name
            )));
        };
        // A fragment taken from another document, or from another part,
        // names that part's relationships: the same id may name another link
        // or part here. An internal target can be renamed by a save, so its
        // bytes are compared; an external target is compared whole.
        if let Some((source, digest)) = fragment
            .relationships
            .iter()
            .find(|(source, _)| source.id == id)
            && (source.rel_type != relationship.rel_type
                || relationship_is_internal(source) != relationship_is_internal(relationship)
                || (!relationship_is_internal(source) && source.target != relationship.target)
                || (relationship_is_internal(source)
                    && *digest
                        != relationship_target_digest(
                            document,
                            &destination.part_name,
                            relationship,
                        )))
        {
            return Err(Error::Other(format!(
                "content fragment relationship {id} names {} where it was taken and {} in {}; \
                 move content between documents with DocumentFragment::from_range and \
                 import_fragment",
                source.target, relationship.target, destination.part_name
            )));
        }
    }
    Ok(())
}

pub(super) fn content_fragment_for_insertion(
    document: &Document,
    destination: &StoryId,
    fragment: &ContentFragment,
) -> Result<Vec<u8>> {
    validate_fragment_relationships(document, destination, fragment)?;
    let keep_scope =
        fragment.source_part_name.is_none() || story_scope_matches_fragment(destination, fragment);
    let mut xml = Vec::new();
    for piece in fragment
        .markers
        .before
        .iter()
        .chain(std::iter::once(&fragment.xml))
        .chain(&fragment.markers.after)
    {
        if keep_scope {
            xml.extend_from_slice(piece);
        } else {
            xml.extend(close_content_fragment_namespaces(
                piece,
                &fragment.namespace_scope,
            )?);
        }
    }
    Ok(xml)
}

/// `xml` without the `w14:paraId` and `w14:textId` of its paragraphs and
/// rows, whatever the prefix bound to the w14 namespace, in the fragment or in
/// `scope`. Only start tags of `p` and `tr` are edited.
pub(super) fn without_w14_identities(xml: &[u8], scope: &BTreeMap<String, String>) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(xml) else {
        return xml.to_vec();
    };
    if !text.contains("paraId") && !text.contains("textId") {
        return xml.to_vec();
    }
    let mut prefixes: Vec<String> = scope
        .iter()
        .filter(|(_, namespace)| namespace.as_str() == W14_NAMESPACE)
        .map(|(prefix, _)| prefix.clone())
        .collect();
    let declared = regex::Regex::new(r#"xmlns:([A-Za-z_][\w.-]*)\s*=\s*["']([^"']*)["']"#)
        .expect("a valid pattern");
    for capture in declared.captures_iter(text) {
        if &capture[2] == W14_NAMESPACE && !prefixes.contains(&capture[1].to_owned()) {
            prefixes.push(capture[1].to_owned());
        }
    }
    let mut result = text.to_owned();
    for prefix in prefixes.iter().filter(|prefix| !prefix.is_empty()) {
        let attribute = regex::Regex::new(&format!(
            r#"(<(?:[A-Za-z_][\w.-]*:)?(?:p|tr)\b[^>]*?)\s{}:(?:paraId|textId)\s*=\s*(?:"[^"]*"|'[^']*')"#,
            regex::escape(prefix)
        ))
        .expect("a valid pattern");
        // A tag holds both attributes: replace until none is left.
        while attribute.is_match(&result) {
            result = attribute.replace_all(&result, "$1").into_owned();
        }
    }
    result.into_bytes()
}

/// Give every document-wide identity of the fragment, its bookmark markers
/// included, a fresh value, as a copy needs.
pub(super) fn freshen_content_fragment_identities(
    document: &mut Document,
    fragment: &mut ContentFragment,
) -> Result<()> {
    let wrapped = wrapped_content_fragment(fragment)?;
    let updated = crate::field::freshen_content_fragment_identities(document, &wrapped)?;
    let updated = remove_comment_anchors_from_fragment(&updated)?;
    replace_wrapped_content_fragment(fragment, &updated)
}

/// Give a fresh value to each identity of the fragment, its bookmark markers
/// included, that `document` already holds, as an insertion needs: what is
/// free keeps its value, so a cut and paste changes nothing.
pub(super) fn freshen_taken_content_fragment_identities(
    document: &mut Document,
    fragment: &mut ContentFragment,
) -> Result<()> {
    let wrapped = wrapped_content_fragment(fragment)?;
    let updated = crate::field::freshen_taken_content_fragment_identities(document, &wrapped)?;
    replace_wrapped_content_fragment(fragment, &updated)
}

/// Whether the fragment holds a note reference, which
/// [`copy_shown_notes`] reads the main part for.
pub(super) fn fragment_holds_note_references(fragment: &ContentFragment) -> Result<bool> {
    let wrapped = wrapped_content_fragment(fragment)?;
    Ok(!crate::field::fragment_note_references(&wrapped)?.is_empty())
}

/// Give each note reference of the fragment whose note a reference of
/// `document` already shows a copy of that note, with fresh identities, as
/// Word gives a pasted note mark: two references never share one note. A
/// note no reference shows (its reference was cut) keeps its reference.
pub(super) fn copy_shown_notes(
    document: &mut Document,
    fragment: &mut ContentFragment,
) -> Result<()> {
    let wrapped = wrapped_content_fragment(fragment)?;
    let references = crate::field::fragment_note_references(&wrapped)?;
    if references.is_empty() {
        return Ok(());
    }
    document.flush_to_package()?;
    let main = document
        .package
        .get_part(&document.doc_part_name)
        .ok_or_else(|| Error::Other("main document part is missing".to_owned()))?;
    let shown = super::content::note_reference_ids_in(main)?;
    let mut maps = HashMap::<StoryKind, BTreeMap<String, String>>::new();
    let mut used = HashMap::<StoryKind, HashSet<i32>>::new();
    for (kind, id) in references {
        let is_shown = match kind {
            StoryKind::Footnote => shown.footnotes.contains(&id),
            StoryKind::Endnote => shown.endnotes.contains(&id),
            _ => false,
        };
        if !is_shown
            || maps
                .get(&kind)
                .is_some_and(|map| map.contains_key(&id.to_string()))
        {
            continue;
        }
        let (part, note) = document.fragment_note_dependency(kind, id)?;
        let taken = match used.entry(kind) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let part_xml = document
                    .package
                    .get_part(&part)
                    .ok_or_else(|| Error::Other(format!("note part {part} is missing")))?;
                let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(part_xml)?;
                entry.insert(notes.footnotes.iter().map(|note| note.id).collect())
            }
        };
        let mut fresh = 1i32;
        while taken.contains(&fresh) {
            fresh = fresh
                .checked_add(1)
                .ok_or_else(|| Error::Other("note ids are exhausted".to_owned()))?;
        }
        taken.insert(fresh);
        let map = HashMap::from([(kind, BTreeMap::from([(id.to_string(), fresh.to_string())]))]);
        let note = crate::field::patch_fragment_note_ids(&note, &map)?;
        let open = format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#);
        let close = "</w:body></w:document>";
        let mut copy = open.clone().into_bytes();
        copy.extend_from_slice(&note);
        copy.extend_from_slice(close.as_bytes());
        let copy = crate::field::freshen_taken_content_fragment_identities(document, &copy)?;
        let note = copy[open.len()..copy.len() - close.len()].to_vec();
        document.publish_fragment_note_staged(kind, &part, note, None)?;
        maps.entry(kind)
            .or_default()
            .insert(id.to_string(), fresh.to_string());
    }
    if maps.is_empty() {
        return Ok(());
    }
    let updated = crate::field::patch_fragment_note_ids(&wrapped, &maps)?;
    replace_wrapped_content_fragment(fragment, &updated)
}

/// The fragment's markers and item, in order, as the body of a document
/// that declares the fragment's namespace scope.
fn wrapped_content_fragment(fragment: &ContentFragment) -> Result<Vec<u8>> {
    let mut wrapper_prefix = "rdocxw".to_owned();
    let mut suffix = 1usize;
    while fragment.namespace_scope.contains_key(&wrapper_prefix) {
        wrapper_prefix = format!("rdocxw{suffix}");
        suffix = suffix
            .checked_add(1)
            .ok_or_else(|| Error::Other("content wrapper prefix space exhausted".to_owned()))?;
    }
    let mut prefix =
        format!(r#"<{wrapper_prefix}:document xmlns:{wrapper_prefix}="{WORD_NAMESPACE}""#)
            .into_bytes();
    for (namespace_prefix, namespace) in &fragment.namespace_scope {
        if namespace_prefix == "xml" || namespace_prefix == &wrapper_prefix {
            continue;
        }
        prefix.extend_from_slice(b" xmlns");
        if !namespace_prefix.is_empty() {
            prefix.push(b':');
            prefix.extend_from_slice(namespace_prefix.as_bytes());
        }
        prefix.extend_from_slice(b"=\"");
        prefix.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
        prefix.push(b'"');
    }
    prefix.extend_from_slice(format!("><{wrapper_prefix}:body>").as_bytes());
    let suffix = format!("</{wrapper_prefix}:body></{wrapper_prefix}:document>");
    let mut wrapped = prefix;
    for marker in &fragment.markers.before {
        wrapped.extend_from_slice(marker);
    }
    wrapped.extend_from_slice(&fragment.xml);
    for marker in &fragment.markers.after {
        wrapped.extend_from_slice(marker);
    }
    wrapped.extend_from_slice(suffix.as_bytes());
    Ok(wrapped)
}

/// Put the markers and item of a rewritten [`wrapped_content_fragment`]
/// back in the fragment.
fn replace_wrapped_content_fragment(fragment: &mut ContentFragment, updated: &[u8]) -> Result<()> {
    let owner = scan_story_owners(updated, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("rewritten content wrapper has no body".to_owned()))?;
    let items = direct_story_content_items(updated, &owner)?;
    let before = fragment.markers.before.len();
    if items.len() != before + 1 + fragment.markers.after.len() {
        return Err(Error::Other(
            "rewritten content wrapper must keep its item and markers".to_owned(),
        ));
    }
    let piece = |index: usize| updated[items[index].full.clone()].to_vec();
    fragment.markers.before = (0..before).map(piece).collect();
    fragment.xml = piece(before);
    fragment.markers.after = (before + 1..items.len()).map(piece).collect();
    Ok(())
}

fn remove_comment_anchors_from_fragment(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut ranges = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comment anchor scan failed: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Empty(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
                    ) =>
            {
                ranges.push(before..after);
            }
            Event::Start(element)
                if is_word
                    && matches!(
                        element.local_name().as_ref(),
                        b"commentRangeStart" | b"commentRangeEnd" | b"commentReference"
                    ) =>
            {
                ranges.push(before..story_element_end(xml, before)?);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = Vec::with_capacity(xml.len());
    let mut cursor = 0usize;
    for range in ranges {
        result.extend_from_slice(&xml[cursor..range.start]);
        cursor = range.end;
    }
    result.extend_from_slice(&xml[cursor..]);
    Ok(result)
}
