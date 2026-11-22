//! Copying the result of a REF field: the bookmarked runs, with their notes and comments.

use super::*;
use oxoffice_docx_oxml::text::same_bookmark_name;

/// Preflight before copying a current Field. The boolean identifies unchanged
/// imported facts whose established raw preservation bypasses semantic editing.
pub(crate) fn preflight_current_field(field: &Field) -> Result<bool> {
    if let Err(error) = field.validate_for_attachment() {
        if field
            .source_replacement()?
            .is_some_and(|(original, current)| original == current)
        {
            return Ok(true);
        }
        return Err(error.into());
    }
    Ok(false)
}

/// Freeze one complete current Field tree for this read operation. Recursive
/// consumers borrow its already-normalized descendants instead of restaging each
/// subtree. Only required retained result projections get separate owned CT_Ps.
pub(crate) fn prepared_current_field(field: &Field) -> Result<Field> {
    fn normalize(field: &mut Field) -> Result<()> {
        for argument in field.instruction.arguments.iter_mut().chain(
            field
                .instruction
                .switches
                .iter_mut()
                .filter_map(|switch| switch.argument.as_mut()),
        ) {
            for child in argument.nested_fields_mut() {
                normalize(child)?;
            }
        }
        let count = field.cached_fields_in_source_order().len();
        for index in 0..count {
            let child = field.cached_field_mut(index).ok_or_else(|| {
                Error::Other("current field preparation lost its actual cached slot".into())
            })?;
            normalize(child)?;
        }
        field.normalize_cached_result_owner()?;
        Ok(())
    }
    if preflight_current_field(field)? {
        return Ok(field.clone());
    }
    let mut current = field.clone();
    normalize(&mut current)?;
    Ok(current)
}

pub(super) fn collect_ref_cached_note_references(
    field: &Field,
    output: &mut Vec<(StoryKind, i32)>,
) -> Result<()> {
    fn collect(field: &Field, output: &mut Vec<(StoryKind, i32)>) -> Result<()> {
        let prepared = field.cached_result_owner_projection()?;
        let owned_runs = prepared.as_ref().map(|paragraph| {
            paragraph
                .accepted_run_paths()
                .into_iter()
                .filter_map(|path| paragraph.accepted_run(&path))
                .collect::<Vec<_>>()
        });
        let runs = owned_runs
            .as_deref()
            .map(|runs| runs.to_vec())
            .or_else(|| field.cached_result_runs().map(|runs| runs.iter().collect()));
        if let Some(runs) = runs {
            for content in runs.into_iter().flat_map(|run| &run.content) {
                match content {
                    RunContent::FootnoteRef { id, .. } => output.push((StoryKind::Footnote, *id)),
                    RunContent::EndnoteRef { id, .. } => output.push((StoryKind::Endnote, *id)),
                    RunContent::Field(field) => collect(field, output)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
    let current = prepared_current_field(field)?;
    collect(&current, output)
}

pub(super) fn ref_note_reference_counts(
    package: &OpcPackage,
) -> Result<HashMap<(StoryKind, i32), usize>> {
    let mut counts = HashMap::new();
    for (name, xml) in &package.parts {
        if !name.ends_with(".xml")
            || ![
                b"footnoteReference".as_slice(),
                b"endnoteReference".as_slice(),
            ]
            .iter()
            .any(|needle| xml.windows(needle.len()).any(|window| window == *needle))
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!(
                    "REF f source reference graph is malformed: {error}"
                ))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if namespace_is_word(&namespace) {
                        let kind = match local.as_ref() {
                            b"footnoteReference" => Some(StoryKind::Footnote),
                            b"endnoteReference" => Some(StoryKind::Endnote),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            let (_, id) = resolved_element_attribute(
                                &element,
                                reader.resolver(),
                                b"id",
                                AttributeNamespace::Word,
                            )?
                            .ok_or_else(|| {
                                Error::Other("REF f source reference has no ID".into())
                            })?;
                            let id = id.parse::<i32>().map_err(|_| {
                                Error::Other("REF f source reference ID is invalid".into())
                            })?;
                            *counts.entry((kind, id)).or_insert(0) += 1;
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

#[derive(Clone, Copy)]
enum RefCommentCompanionKind {
    Extended,
    Ids,
}

impl RefCommentCompanionKind {
    fn namespace(self) -> &'static str {
        match self {
            Self::Extended => oxoffice_docx_oxml::comments_extended::W15_NS,
            Self::Ids => "http://schemas.microsoft.com/office/word/2016/wordml/cid",
        }
    }

    fn root(self) -> &'static [u8] {
        match self {
            Self::Extended => b"commentsEx",
            Self::Ids => b"commentsIds",
        }
    }

    fn item(self) -> &'static [u8] {
        match self {
            Self::Extended => b"commentEx",
            Self::Ids => b"commentId",
        }
    }

    fn relationship(self) -> &'static str {
        match self {
            Self::Extended => crate::comments::COMMENTS_EXTENDED_REL_TYPE,
            Self::Ids => "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds",
        }
    }
}

struct RefCommentCompanionEntry {
    para_id: String,
    parent: Option<String>,
    durable_id: Option<String>,
    span: std::ops::Range<usize>,
    xml: Vec<u8>,
}

fn ref_comment_companion_entries(
    xml: &[u8],
    kind: RefCommentCompanionKind,
) -> Result<(Vec<RefCommentCompanionEntry>, usize)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut root_end = None;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut durable_seen = HashSet::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation companion XML is malformed: {error}"
            ))
        })?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let empty = xml.get(end.saturating_sub(2)..end) == Some(b"/>");
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let owned = namespace_matches(&namespace, kind.namespace());
                if depth == 0 {
                    if !owned || local.as_ref() != kind.root() || saw_root || empty {
                        return Err(Error::Other(
                            "REF f annotation companion has no unique qualified root".into(),
                        ));
                    }
                    saw_root = true;
                } else if depth == 1 && owned && local.as_ref() == kind.item() {
                    let attribute = |name| {
                        resolved_element_attribute(
                            &element,
                            reader.resolver(),
                            name,
                            AttributeNamespace::Bound(kind.namespace()),
                        )
                        .map(|value| value.map(|(_, value)| value))
                    };
                    let para_id = attribute(b"paraId")?.ok_or_else(|| {
                        Error::Other("REF f annotation companion entry has no paragraph ID".into())
                    })?;
                    if para_id.len() != 8
                        || u32::from_str_radix(&para_id, 16).is_err()
                        || !seen.insert(para_id.to_ascii_uppercase())
                    {
                        return Err(Error::Other(
                            "REF f annotation companion paragraph ID is invalid or ambiguous"
                                .into(),
                        ));
                    }
                    let parent = attribute(b"paraIdParent")?;
                    let durable_id = attribute(b"durableId")?;
                    if matches!(kind, RefCommentCompanionKind::Ids)
                        && !durable_id.as_ref().is_some_and(|id| {
                            id.len() == 8
                                && u32::from_str_radix(id, 16).is_ok()
                                && durable_seen.insert(id.to_ascii_uppercase())
                        })
                    {
                        return Err(Error::Other(
                            "REF f annotation companion durable ID is absent or invalid".into(),
                        ));
                    }
                    if !empty {
                        reader
                            .read_to_end_into(element.name(), &mut Vec::new())
                            .map_err(|error| {
                                Error::Other(format!(
                                    "REF f annotation companion entry is unclosed: {error}"
                                ))
                            })?;
                    }
                    let span = start..reader.buffer_position() as usize;
                    let scope = crate::document::story_namespace_scope_at(xml, start)?;
                    entries.push(RefCommentCompanionEntry {
                        para_id,
                        parent,
                        durable_id,
                        xml: crate::document::close_content_fragment_namespaces(
                            &xml[span.clone()],
                            &scope,
                        )?,
                        span,
                    });
                    buffer.clear();
                    continue;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(element) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("REF f annotation companion XML is unbalanced".into())
                })?;
                if depth == 0 {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if !namespace_matches(&namespace, kind.namespace())
                        || local.as_ref() != kind.root()
                        || root_end.replace(start).is_some()
                    {
                        return Err(Error::Other(
                            "REF f annotation companion root is ambiguous".into(),
                        ));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 || !saw_root {
        return Err(Error::Other(
            "REF f annotation companion XML is unclosed".into(),
        ));
    }
    Ok((
        entries,
        root_end
            .ok_or_else(|| Error::Other("REF f annotation companion root end is missing".into()))?,
    ))
}

fn ref_comment_paragraph_ids(xml: &[u8]) -> Result<Vec<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("REF f annotation payload is malformed: {error}"))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && matches!(
                        local.as_ref(),
                        b"commentReference" | b"commentRangeStart" | b"commentRangeEnd"
                    )
                {
                    return Err(Error::Other("REF f annotation payload contains an unqualified recursive annotation graph".into()));
                }
                if namespace_is_word(&namespace) && local.as_ref() == b"p" {
                    let (_, id) = resolved_element_attribute(&element, reader.resolver(), b"paraId", AttributeNamespace::Bound(W14_NS))?
                        .ok_or_else(|| Error::Other("REF f annotation paragraph lacks qualified identity for companion closure".into()))?;
                    if id.len() != 8
                        || u32::from_str_radix(&id, 16).is_err()
                        || !seen.insert(id.to_ascii_uppercase())
                    {
                        return Err(Error::Other(
                            "REF f annotation paragraph identity is invalid or duplicated".into(),
                        ));
                    }
                    ids.push(id);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(ids)
}

fn ref_comment_paragraph_identity_counts(xml: &[u8]) -> Result<HashMap<String, usize>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut counts = HashMap::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation owner identity inventory is malformed: {error}"
            ))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && local.as_ref() == b"p"
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"paraId",
                        AttributeNamespace::Bound(W14_NS),
                    )?
                {
                    *counts.entry(id.to_ascii_uppercase()).or_insert(0) += 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(counts)
}

fn ref_comment_companion_part(
    document: &Document,
    kind: RefCommentCompanionKind,
) -> Result<Option<String>> {
    let relationships = document.package.get_part_rels(&document.doc_part_name);
    let mut matches = relationships
        .into_iter()
        .flat_map(|relationships| &relationships.items)
        .filter(|relationship| relationship.rel_type == kind.relationship());
    let first = matches.next();
    if matches.next().is_some()
        || first
            .is_some_and(|relationship| !crate::document::relationship_is_internal(relationship))
    {
        return Err(Error::Other(
            "REF f annotation companion relationship owner is ambiguous or external".into(),
        ));
    }
    first
        .map(|relationship| {
            let part =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if document.package.get_part(&part).is_none() {
                return Err(Error::Other(
                    "REF f annotation companion relationship target is missing".into(),
                ));
            }
            Ok(part)
        })
        .transpose()
}

struct RefCommentCompanionCopy {
    kind: RefCommentCompanionKind,
    part: String,
    xml: Vec<u8>,
    replace_para_id: Option<String>,
}

pub(super) struct RefCommentCopy {
    part: String,
    xml: Vec<u8>,
    replace_id: Option<i32>,
    companions: Vec<RefCommentCompanionCopy>,
}

pub(super) fn ref_comment_occupied_identity_values(package: &OpcPackage) -> Result<HashSet<u32>> {
    let mut occupied = HashSet::new();
    for (part, xml) in &package.parts {
        if !part.ends_with(".xml") {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!(
                    "REF f annotation identity inventory is malformed: {error}"
                ))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!(
                                "REF f annotation identity attribute is malformed: {error}"
                            ))
                        })?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        let namespace_owned = namespace_matches(&namespace, W14_NS)
                            || namespace_matches(
                                &namespace,
                                RefCommentCompanionKind::Extended.namespace(),
                            )
                            || namespace_matches(
                                &namespace,
                                RefCommentCompanionKind::Ids.namespace(),
                            );
                        if namespace_owned
                            && matches!(local.as_ref(), b"paraId" | b"paraIdParent" | b"durableId")
                        {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!(
                                        "REF f annotation identity value is malformed: {error}"
                                    ))
                                })?;
                            if value.len() != 8 {
                                return Err(Error::Other("REF f annotation identity is not an eight-digit hexadecimal value".into()));
                            }
                            occupied.insert(u32::from_str_radix(&value, 16).map_err(|_| {
                                Error::Other("REF f annotation identity is invalid".into())
                            })?);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(occupied)
}

pub(crate) fn patch_ref_comment_identity_attributes(
    xml: &[u8],
    comment_ids: &BTreeMap<String, String>,
    paragraph_ids: &BTreeMap<String, String>,
    durable_ids: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation identity patch XML is malformed: {error}"
            ))
        })?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace) && local.as_ref() == b"comment" {
                    add_identity_attribute_edit(
                        xml,
                        start,
                        end,
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                        comment_ids,
                        &mut edits,
                    )?;
                }
                if namespace_is_word(&namespace) && local.as_ref() == b"p" {
                    add_identity_attribute_edit(
                        xml,
                        start,
                        end,
                        &element,
                        reader.resolver(),
                        b"paraId",
                        AttributeNamespace::Bound(W14_NS),
                        paragraph_ids,
                        &mut edits,
                    )?;
                }
                for kind in [
                    RefCommentCompanionKind::Extended,
                    RefCommentCompanionKind::Ids,
                ] {
                    if namespace_matches(&namespace, kind.namespace())
                        && local.as_ref() == kind.item()
                    {
                        add_identity_attribute_edit(
                            xml,
                            start,
                            end,
                            &element,
                            reader.resolver(),
                            b"paraId",
                            AttributeNamespace::Bound(kind.namespace()),
                            paragraph_ids,
                            &mut edits,
                        )?;
                        if matches!(kind, RefCommentCompanionKind::Extended) {
                            add_identity_attribute_edit(
                                xml,
                                start,
                                end,
                                &element,
                                reader.resolver(),
                                b"paraIdParent",
                                AttributeNamespace::Bound(kind.namespace()),
                                paragraph_ids,
                                &mut edits,
                            )?;
                        }
                        add_identity_attribute_edit(
                            xml,
                            start,
                            end,
                            &element,
                            reader.resolver(),
                            b"durableId",
                            AttributeNamespace::Bound(kind.namespace()),
                            durable_ids,
                            &mut edits,
                        )?;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = xml.to_vec();
    for edit in edits.into_iter().rev() {
        result.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(result)
}

pub(super) fn prepare_ref_comment_copy(
    document: &mut Document,
    original: &Document,
    source_id: i32,
    replace_id: Option<i32>,
    occupied_comments: &mut HashSet<i32>,
    occupied_identities: &mut HashSet<u32>,
) -> Result<(i32, RefCommentCopy)> {
    let (part, source) = original.fragment_comment_dependency(source_id)?;
    if document.comments_part_name.as_deref() != Some(&part)
        || !fragment_note_references(&source)?.is_empty()
    {
        return Err(Error::Other(
            "REF f annotation has an unqualified relationship owner or recursive note payload"
                .into(),
        ));
    }
    let paragraph_ids = ref_comment_paragraph_ids(&source)?;
    let paragraph_owners = ref_comment_paragraph_identity_counts(
        original
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("REF f annotation owner part is absent".into()))?,
    )?;
    if paragraph_ids
        .iter()
        .any(|id| paragraph_owners.get(&id.to_ascii_uppercase()) != Some(&1))
    {
        return Err(Error::Other(
            "REF f annotation paragraph identity is shared with another physical owner".into(),
        ));
    }
    let source_last = paragraph_ids.last().ok_or_else(|| {
        Error::Other("REF f annotation source has no qualified last paragraph identity".into())
    })?;
    let old_last = replace_id
        .map(|id| {
            original
                .fragment_comment_dependency(id)
                .and_then(|(old_part, xml)| {
                    if old_part != part || id == source_id {
                        return Err(Error::Other(
                            "REF f cached annotation is borrowed or differently owned".into(),
                        ));
                    }
                    let old_paragraphs = ref_comment_paragraph_ids(&xml)?;
                    if old_paragraphs.iter().any(|id|paragraph_owners.get(&id.to_ascii_uppercase())!=Some(&1)) {
                        return Err(Error::Other("REF f cached annotation paragraph identity is shared with another physical owner".into()));
                    }
                    old_paragraphs
                        .last()
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other(
                                "REF f cached annotation has no qualified last paragraph identity"
                                    .into(),
                            )
                        })
                })
        })
        .transpose()?;
    let mut para_map = BTreeMap::new();
    for old in &paragraph_ids {
        para_map.insert(
            old.clone(),
            crate::comments::allocate_para_id_from_occupied(occupied_identities)?,
        );
    }
    let id = if let Some(id) = replace_id {
        if !occupied_comments.contains(&id) {
            return Err(Error::Other(
                "REF f cached annotation owner is absent".into(),
            ));
        }
        id
    } else {
        let mut id = 0i32;
        while occupied_comments.contains(&id) {
            id = id
                .checked_add(1)
                .ok_or_else(|| Error::Other("REF f annotation IDs are exhausted".into()))?;
        }
        occupied_comments.insert(id);
        id
    };
    let mut companions = Vec::new();
    for kind in [
        RefCommentCompanionKind::Extended,
        RefCommentCompanionKind::Ids,
    ] {
        let Some(companion_part) = ref_comment_companion_part(original, kind)? else {
            continue;
        };
        let xml = original.package.get_part(&companion_part).unwrap();
        let (entries, _) = ref_comment_companion_entries(xml, kind)?;
        let selected = entries
            .iter()
            .filter(|entry| entry.para_id.eq_ignore_ascii_case(source_last))
            .collect::<Vec<_>>();
        if selected.len() != 1
            || selected[0].parent.is_some()
            || entries.iter().any(|entry| {
                entry
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent.eq_ignore_ascii_case(source_last))
            })
        {
            return Err(Error::Other("REF f annotation companion source is missing, ambiguous or part of an unqualified reply graph".into()));
        }
        let replace_para_id = if let Some(old_last) = &old_last {
            let old = entries
                .iter()
                .filter(|entry| entry.para_id.eq_ignore_ascii_case(old_last))
                .collect::<Vec<_>>();
            if old.len() != 1
                || old[0].parent.is_some()
                || entries.iter().any(|entry| {
                    entry
                        .parent
                        .as_ref()
                        .is_some_and(|parent| parent.eq_ignore_ascii_case(old_last))
                })
            {
                return Err(Error::Other(
                    "REF f cached annotation companion closure is ambiguous".into(),
                ));
            }
            Some(old[0].para_id.clone())
        } else {
            None
        };
        let mut durable_map = BTreeMap::new();
        if let Some(old) = &selected[0].durable_id {
            durable_map.insert(
                old.clone(),
                crate::comments::allocate_para_id_from_occupied(occupied_identities)?,
            );
        }
        let mut companion_para_map = para_map.clone();
        // Companion producers may use a different hexadecimal spelling of the same ID.
        companion_para_map.insert(selected[0].para_id.clone(), para_map[source_last].clone());
        companions.push(RefCommentCompanionCopy {
            kind,
            part: companion_part,
            xml: patch_ref_comment_identity_attributes(
                &selected[0].xml,
                &BTreeMap::new(),
                &companion_para_map,
                &durable_map,
            )?,
            replace_para_id,
        });
    }
    for relationship_id in relationship_ids_in_xml(&source)? {
        let relationship = original
            .package
            .get_part_rels(&part)
            .and_then(|relationships| relationships.get_by_id(&relationship_id))
            .ok_or_else(|| {
                Error::Other("REF f annotation payload relationship is absent".into())
            })?;
        if !crate::document::relationship_is_internal(relationship) {
            return Err(Error::Other(
                "REF f annotation external relationship payload lacks authenticated controls"
                    .into(),
            ));
        }
        discover_fragment_part_closure(
            &original.package,
            &OpcPackage::resolve_rel_target(&part, &relationship.target),
            &mut HashSet::new(),
        )?;
    }
    let mut xml = patch_ref_comment_identity_attributes(
        &source,
        &BTreeMap::from([(source_id.to_string(), id.to_string())]),
        &para_map,
        &BTreeMap::new(),
    )?;
    let wrapper = format!("<w:body xmlns:w=\"{W_NS}\">");
    let mut body = wrapper.as_bytes().to_vec();
    body.extend_from_slice(&xml);
    body.extend_from_slice(b"</w:body>");
    let mut remap = BodyIdentityRemap::default();
    for old in body_identity_values(&body)?.drawing_ids {
        if remap
            .drawing_ids
            .insert(old, document.identifiers.reserve_drawing_id()?.to_string())
            .is_some()
        {
            return Err(Error::Other(
                "REF f annotation source has duplicate drawing identities".into(),
            ));
        }
    }
    let mapped = patch_body_identity_attributes(&body, &remap)?;
    xml = mapped[wrapper.len()..mapped.len() - b"</w:body>".len()].to_vec();
    Ok((
        id,
        RefCommentCopy {
            part,
            xml,
            replace_id,
            companions,
        },
    ))
}

pub(super) fn publish_ref_comment_copy(
    document: &mut Document,
    copy: RefCommentCopy,
) -> Result<()> {
    document.publish_fragment_comment_staged(&copy.part, copy.xml, copy.replace_id)?;
    for companion in copy.companions {
        let source = document.package.get_part(&companion.part).ok_or_else(|| {
            Error::Other("REF f annotation companion publication target is missing".into())
        })?;
        let (entries, end) = ref_comment_companion_entries(source, companion.kind)?;
        let mut updated = source.to_vec();
        if let Some(id) = companion.replace_para_id {
            let entries = entries
                .iter()
                .filter(|entry| entry.para_id == id)
                .collect::<Vec<_>>();
            if entries.len() != 1 {
                return Err(Error::Other(
                    "REF f cached annotation companion publication owner is ambiguous".into(),
                ));
            }
            updated.splice(entries[0].span.clone(), companion.xml);
        } else {
            updated.splice(end..end, companion.xml);
        }
        ref_comment_companion_entries(&updated, companion.kind)?;
        if matches!(companion.kind, RefCommentCompanionKind::Extended) {
            document.comments_extended =
                Some(oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(&updated)?);
        }
        document.package.set_part(&companion.part, updated);
    }
    Ok(())
}

pub(super) fn ref_comment_reference_ids(cache: &RefBookmarkCache) -> Result<Vec<i32>> {
    let ids = cache
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| match content {
            RunContent::CommentReference { id, .. } => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut unique = HashSet::new();
    if cache.comment_ranges.len() != ids.len() * 2 {
        return Err(Error::Other(
            "REF f annotation target lacks closed owned range endpoints".into(),
        ));
    }
    for id in &ids {
        let starts = cache.comment_ranges.iter().filter(|marker| matches!(marker,CommentRangeMarker::Start{id:marker_id,..} if marker_id==id)).count();
        let ends = cache
            .comment_ranges
            .iter()
            .filter(
                |marker| matches!(marker,CommentRangeMarker::End{id:marker_id,..} if marker_id==id),
            )
            .count();
        if !unique.insert(*id) || (starts, ends) != (1, 1) {
            return Err(Error::Other(
                "REF f annotation target range ownership is ambiguous".into(),
            ));
        }
    }
    Ok(ids)
}

fn ref_comment_graph_counts(package: &OpcPackage) -> Result<HashMap<i32, [usize; 3]>> {
    let mut counts = HashMap::new();
    for (part, xml) in &package.parts {
        if !part.ends_with(".xml")
            || !xml
                .windows(b"comment".len())
                .any(|value| value == b"comment")
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!("REF f annotation graph is malformed: {error}"))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if namespace_is_word(&namespace) {
                        let index = match local.as_ref() {
                            b"commentRangeStart" => Some(0),
                            b"commentRangeEnd" => Some(1),
                            b"commentReference" => Some(2),
                            _ => None,
                        };
                        if let Some(index) = index {
                            let (_, value) = resolved_element_attribute(
                                &element,
                                reader.resolver(),
                                b"id",
                                AttributeNamespace::Word,
                            )?
                            .ok_or_else(|| {
                                Error::Other("REF f annotation graph edge has no ID".into())
                            })?;
                            let id = value.parse::<i32>().map_err(|_| {
                                Error::Other("REF f annotation graph edge has an invalid ID".into())
                            })?;
                            counts.entry(id).or_insert([0; 3])[index] += 1;
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

pub(super) fn ref_comment_replacement_ids(
    document: &Document,
    field: &Field,
    source: &RefBookmarkCache,
) -> Result<Vec<Option<i32>>> {
    let mut old = RefBookmarkCache {
        runs: Vec::new(),
        comment_ranges: Vec::new(),
        rows: Vec::new(),
    };
    let mut run = CT_R::new("");
    run.content = vec![RunContent::Field(field.clone())];
    append_ref_cached_run(&run, None, false, &mut old)?;
    let source_ids = ref_comment_reference_ids(source)?;
    let old_ids = ref_comment_reference_ids(&old)?;
    if old_ids.is_empty() {
        return Ok(vec![None; source_ids.len()]);
    }
    if old_ids.len() != source_ids.len() {
        return Err(Error::Other(
            "REF f cached annotation reference inventory differs from its current source".into(),
        ));
    }
    let counts = ref_comment_graph_counts(&document.package)?;
    let mut unique = HashSet::new();
    let mut replacements = Vec::new();
    for (id, source_id) in old_ids.iter().zip(&source_ids) {
        let start = old.comment_ranges.iter().filter(|marker| matches!(marker, CommentRangeMarker::Start {id: marker_id,..} if marker_id==id)).count();
        let end = old.comment_ranges.iter().filter(|marker| matches!(marker, CommentRangeMarker::End {id: marker_id,..} if marker_id==id)).count();
        if id == source_id
            || !unique.insert(*id)
            || (start, end) != (1, 1)
            || counts.get(id) != Some(&[1, 1, 1])
        {
            return Err(Error::Other("REF f cached annotation owner is borrowed, shared or has ambiguous range endpoints".into()));
        }
        document.fragment_comment_dependency(*id)?;
        replacements.push(Some(*id));
    }
    Ok(replacements)
}

pub(super) fn prepare_ref_note_copy(
    document: &mut Document,
    original: &Document,
    kind: StoryKind,
    old_id: i32,
    replace_id: Option<i32>,
    occupied: &mut HashMap<StoryKind, HashSet<i32>>,
) -> Result<(i32, FragmentNoteCopy)> {
    let (source_part, source) = original.fragment_note_dependency(kind, old_id)?;
    if !fragment_note_references(&source)?.is_empty() {
        return Err(Error::Other(
            "REF f note payload contains recursive note references".into(),
        ));
    }
    let part = document.ensure_fragment_note_part_staged(kind)?;
    if part != source_part {
        return Err(Error::Other(
            "REF f note copy has no same-owner relationship scope".into(),
        ));
    }
    let used = if let Some(used) = occupied.get_mut(&kind) {
        used
    } else {
        let notes = CT_Footnotes::from_xml(
            document
                .package
                .get_part(&part)
                .ok_or_else(|| Error::Other("REF f note part is missing".into()))?,
        )?;
        occupied
            .entry(kind)
            .or_insert_with(|| notes.footnotes.iter().map(|note| note.id).collect())
    };
    let id = if let Some(id) = replace_id {
        let (old_part, _) = original.fragment_note_dependency(kind, id)?;
        if old_part != part || !used.contains(&id) {
            return Err(Error::Other(
                "REF f cached note replacement is not a unique existing owner".into(),
            ));
        }
        id
    } else {
        let mut id = 1i32;
        while used.contains(&id) {
            id = id
                .checked_add(1)
                .ok_or_else(|| Error::Other("REF f note IDs are exhausted".into()))?;
        }
        used.insert(id);
        id
    };
    let mut xml = transform_ref_note_copy(original, &source_part, &source)?;
    for relationship_id in relationship_ids_in_xml(&xml)? {
        let relationships = original
            .package
            .get_part_rels(&source_part)
            .ok_or_else(|| Error::Other("REF f note relationship owner is missing".into()))?;
        let relationship = relationships.get_by_id(&relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "REF f note relationship {relationship_id} is missing"
            ))
        })?;
        if !crate::document::relationship_is_internal(relationship) {
            return Err(Error::Other(
                "REF f retained note payload has an external relationship".into(),
            ));
        }
        let target = OpcPackage::resolve_rel_target(&source_part, &relationship.target);
        discover_fragment_part_closure(&original.package, &target, &mut HashSet::new())?;
    }
    let wrapper = format!("<w:body xmlns:w=\"{W_NS}\">").into_bytes();
    let mut body = wrapper.clone();
    body.extend_from_slice(&xml);
    body.extend_from_slice(b"</w:body>");
    let values = body_identity_values(&body)?;
    let mut remap = BodyIdentityRemap::default();
    for old in values.drawing_ids {
        if remap.drawing_ids.contains_key(&old) {
            return Err(Error::Other(
                "REF f source has duplicate drawing IDs".into(),
            ));
        }
        remap
            .drawing_ids
            .insert(old, document.identifiers.reserve_drawing_id()?.to_string());
    }
    let mapped = patch_body_identity_attributes(&body, &remap)?;
    xml = mapped[wrapper.len()..mapped.len() - b"</w:body>".len()].to_vec();
    let maps = HashMap::from([(kind, BTreeMap::from([(old_id.to_string(), id.to_string())]))]);
    xml = patch_fragment_note_ids(&xml, &maps)?;
    Ok((
        id,
        FragmentNoteCopy {
            kind,
            destination_part: part,
            xml,
            source: None,
            replace_id,
        },
    ))
}

fn transform_ref_note_copy(document: &Document, part: &str, source: &[u8]) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<bool>::new();
    let mut edits = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("REF f note XML is malformed: {error}")))?;
        let word = namespace_is_word(&namespace);
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = element.local_name();
                if word && matches!(local.as_ref(), b"bookmarkStart" | b"bookmarkEnd") {
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("REF f note bookmark is malformed: {error}"))
                        })?;
                    edits.push(start..reader.buffer_position() as usize);
                } else {
                    let flatten = if word && local.as_ref() == b"hyperlink" {
                        let relationship_id = resolved_element_attribute(&element, reader.resolver(), b"id", AttributeNamespace::Relationship)?.map(|(_, value)| value).ok_or_else(|| Error::Other("REF f internal note hyperlink is unsupported without native controls".into()))?;
                        let relationship = document
                            .package
                            .get_part_rels(part)
                            .and_then(|relationships| relationships.get_by_id(&relationship_id))
                            .ok_or_else(|| {
                                Error::Other("REF f note hyperlink relationship is missing".into())
                            })?;
                        if crate::document::relationship_is_internal(relationship) {
                            return Err(Error::Other("REF f internal note hyperlink is unsupported without native controls".into()));
                        }
                        edits.push(start..end);
                        true
                    } else {
                        false
                    };
                    stack.push(flatten);
                }
            }
            Event::Empty(element) => {
                if word
                    && matches!(
                        element.local_name().as_ref(),
                        b"bookmarkStart" | b"bookmarkEnd"
                    )
                {
                    edits.push(start..end);
                }
            }
            Event::End(_) => {
                if stack
                    .pop()
                    .ok_or_else(|| Error::Other("REF f note XML is unbalanced".into()))?
                {
                    edits.push(start..end);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(Error::Other("REF f note XML is unclosed".into()));
    }
    let mut result = source.to_vec();
    for range in edits.into_iter().rev() {
        result.drain(range);
    }
    Ok(result)
}

fn collect_ref_copy_fields(
    field: &Field,
    inherited_lock: bool,
    generated: bool,
    suppress_copied_references: bool,
    fields: &mut Vec<RefCopyField>,
) {
    let locked = inherited_lock || field.locked() == Some(true);
    fields.push(RefCopyField {
        field: field.clone(),
        locked,
        generated,
        suppress_copied_references,
    });
    for child in field.nested_fields_in_source_order() {
        collect_ref_copy_fields(child, locked, generated, suppress_copied_references, fields);
    }
    for child in field.cached_fields_in_source_order() {
        collect_ref_copy_fields(child, locked, true, suppress_copied_references, fields);
    }
}

pub(super) fn append_ref_copy_paragraph(
    paragraph: &CT_P,
    suppress_copied_references: bool,
    fields: &mut Vec<RefCopyField>,
) {
    for content in accepted_toc_runs(paragraph)
        .into_iter()
        .flat_map(|run| &run.run.content)
    {
        if let RunContent::Field(field) = content {
            collect_ref_copy_fields(field, false, false, suppress_copied_references, fields);
        }
    }
}

fn ref_cache_marker_at(marker: &CommentRangeMarker, position: usize) -> Result<CommentRangeMarker> {
    match marker {
        CommentRangeMarker::Start {
            id,
            has_child_content: false,
            ..
        } => Ok(CommentRangeMarker::Start {
            id: *id,
            run_index: position,
            raw_before: 0,
            has_child_content: false,
        }),
        CommentRangeMarker::End {
            id,
            has_child_content: false,
            ..
        } => Ok(CommentRangeMarker::End {
            id: *id,
            run_index: position,
            raw_before: 0,
            has_child_content: false,
        }),
        _ => Err(Error::Other(
            "REF f annotation boundary has unmodeled child content".into(),
        )),
    }
}

fn append_ref_cached_run(
    run: &CT_R,
    inherited: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    already_prepared: bool,
    cache: &mut RefBookmarkCache,
) -> Result<()> {
    if let [RunContent::Field(field)] = run.content.as_slice() {
        // Normalize a private current owner before deriving both its runs and
        // annotation graph. Pending caller state remains unpublished.
        let current = if already_prepared {
            None
        } else {
            Some(prepared_current_field(field)?)
        };
        let field = current.as_ref().unwrap_or(field);
        let prepared = field.cached_result_owner_projection()?;
        let owned_runs = prepared.as_ref().map(|paragraph| {
            paragraph
                .accepted_run_paths()
                .into_iter()
                .filter_map(|path| paragraph.accepted_run(&path))
                .collect::<Vec<_>>()
        });
        let runs =
            owned_runs.or_else(|| field.cached_result_runs().map(|runs| runs.iter().collect()));
        if let Some(runs) = runs {
            let base = cache.runs.len();
            let mut boundaries = Vec::with_capacity(runs.len() + 1);
            for child in runs {
                boundaries.push(cache.runs.len());
                append_ref_cached_run(child, run.properties.as_ref().or(inherited), true, cache)?;
            }
            boundaries.push(cache.runs.len());
            for marker in field.cached_result_comment_ranges() {
                let position = *boundaries
                    .get(match marker {
                        CommentRangeMarker::Start { run_index, .. }
                        | CommentRangeMarker::End { run_index, .. } => *run_index,
                    })
                    .ok_or_else(|| {
                        Error::Other("REF f cached annotation boundary is stale".into())
                    })?;
                cache
                    .comment_ranges
                    .push(ref_cache_marker_at(marker, position)?);
            }
            if cache.runs.len() == base {
                cache.runs.push(CT_R::new(""));
            }
        } else {
            let segments = field.cached_display_segments();
            if segments.is_empty() {
                let mut stored = CT_R::new(&field.cached_result);
                stored.properties = run.properties.clone().or_else(|| inherited.cloned());
                cache.runs.push(stored);
            } else {
                for (text, properties) in segments {
                    let mut stored = CT_R::new(text);
                    stored.properties = properties
                        .cloned()
                        .or_else(|| run.properties.clone())
                        .or_else(|| inherited.cloned());
                    cache.runs.push(stored);
                }
            }
        }
    } else if run
        .content
        .iter()
        .any(|content| matches!(content, RunContent::Field(_)))
    {
        return Err(Error::Other(
            "REF f mixed field target lacks a source-qualified cache boundary".into(),
        ));
    } else {
        let mut run = run.clone();
        if run.properties.is_none() {
            run.properties = inherited.cloned();
        }
        cache.runs.push(run);
    }
    Ok(())
}

fn append_ref_target_paragraph(
    paragraph: &CT_P,
    start: usize,
    end: usize,
    cache: &mut RefBookmarkCache,
) -> Result<()> {
    let projected = paragraph.accepted_bookmark_runs();
    let selected = projected
        .get(start..end)
        .ok_or_else(|| Error::Other("REF f target run range is stale".into()))?;
    cache
        .rows
        .push(selected.iter().map(|run| (*run).clone()).collect());
    let mut boundaries = Vec::with_capacity(selected.len() + 1);
    for run in selected {
        boundaries.push(cache.runs.len());
        append_ref_cached_run(run, None, false, cache)?;
    }
    boundaries.push(cache.runs.len());
    if !paragraph.comment_ranges.is_empty() {
        if !paragraph.content_controls.is_empty()
            || !paragraph.revisions.is_empty()
            || projected.len() != paragraph.runs.len()
        {
            return Err(Error::Other(
                "REF f annotation target lacks an exact accepted boundary projection".into(),
            ));
        }
        let mut starts = BTreeMap::new();
        let mut pairs = Vec::new();
        for marker in &paragraph.comment_ranges {
            match marker {
                CommentRangeMarker::Start {
                    id,
                    run_index,
                    has_child_content: false,
                    ..
                } => {
                    if starts.insert(*id, *run_index).is_some() {
                        return Err(Error::Other(
                            "REF f annotation source range is ambiguous".into(),
                        ));
                    }
                }
                CommentRangeMarker::End {
                    id,
                    run_index,
                    has_child_content: false,
                    ..
                } => {
                    let first = starts.remove(id).ok_or_else(|| {
                        Error::Other("REF f annotation source range is unpaired".into())
                    })?;
                    if first > *run_index {
                        return Err(Error::Other(
                            "REF f annotation source range is reversed".into(),
                        ));
                    }
                    pairs.push((*id, first, *run_index));
                }
                _ => {
                    return Err(Error::Other(
                        "REF f annotation source range contains unmodeled payload".into(),
                    ));
                }
            }
        }
        if !starts.is_empty() {
            return Err(Error::Other(
                "REF f annotation source range crosses an unqualified target paragraph".into(),
            ));
        }
        for (id, first, last) in pairs {
            let clipped_start = first.max(start);
            let clipped_end = last.min(end);
            if clipped_start >= clipped_end {
                continue;
            }
            let first = boundaries[clipped_start - start];
            let last = boundaries[clipped_end - start];
            cache.comment_ranges.push(CommentRangeMarker::Start {
                id,
                run_index: first,
                raw_before: 0,
                has_child_content: false,
            });
            cache.comment_ranges.push(CommentRangeMarker::End {
                id,
                run_index: last,
                raw_before: 0,
                has_child_content: false,
            });
            if !cache.runs.iter().flat_map(|run| &run.content).any(|content| matches!(content,RunContent::CommentReference{id:reference,..} if *reference==id)) {
                let mut reference=CT_R::new("");reference.content=vec![RunContent::CommentReference{id,raw_before:0}];cache.runs.push(reference);
            }
        }
    }
    cache.comment_ranges.sort_by_key(|marker| match marker {
        CommentRangeMarker::Start { run_index, .. } | CommentRangeMarker::End { run_index, .. } => {
            *run_index
        }
    });
    Ok(())
}

pub(super) fn ref_bookmark_runs(
    document: &Document,
    target: &str,
) -> Result<Option<RefBookmarkCache>> {
    let bookmarks = document.bookmarks();
    let matching = bookmarks
        .iter()
        .filter(|bookmark| {
            bookmark
                .name()
                .is_some_and(|name| same_bookmark_name(name, target))
        })
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        if matching.len() != 1 || matching[0].issue().is_some() {
            return Err(Error::Other(
                "REF f target bookmark is ambiguous or malformed".into(),
            ));
        }
        let range = matching[0]
            .range()
            .ok_or_else(|| Error::Other("REF f target lacks a range".into()))?;
        let mut paragraphs = Vec::new();
        collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
        let mut cache = RefBookmarkCache {
            runs: Vec::new(),
            comment_ranges: Vec::new(),
            rows: Vec::new(),
        };
        for index in range.start.body_index..=range.end.body_index {
            let paragraph = paragraphs
                .get(index)
                .ok_or_else(|| Error::Other("REF f target paragraph is absent".into()))?;
            let projected = paragraph.accepted_bookmark_runs();
            let start = if index == range.start.body_index {
                range.start.run_index
            } else {
                0
            };
            let end = if index == range.end.body_index {
                range.end.run_index
            } else {
                projected.len()
            };
            if index > range.start.body_index {
                let mut line = CT_R::new("");
                line.content = vec![RunContent::Break(
                    oxoffice_docx_oxml::text::BreakType::Line,
                    oxoffice_docx_oxml::text::BreakClear::None,
                )];
                cache.runs.push(line);
            }
            append_ref_target_paragraph(paragraph, start, end, &mut cache)?;
        }
        return Ok(Some(cache));
    }
    let (ranges, across) = document.story_range_inventory()?;
    if across
        .iter()
        .any(|kind| matches!(kind, crate::StoryRangeKind::Bookmark { name, .. } if same_bookmark_name(name, target)))
    {
        return Err(Error::Other(
            "REF f target bookmark runs across story owners".into(),
        ));
    }
    let range = ranges.into_iter().find(|range| matches!(range.kind(), crate::StoryRangeKind::Bookmark { name, .. } if same_bookmark_name(name, target)));
    let Some(range) = range else {
        return Ok(None);
    };
    let crate::StoryRangeKind::Bookmark { id, .. } = range.kind() else {
        unreachable!()
    };
    let mut selected = false;
    let mut cache = RefBookmarkCache {
        runs: Vec::new(),
        comment_ranges: Vec::new(),
        rows: Vec::new(),
    };
    let (start_side, end_side) = document.block_bookmark_sides(*id)?;
    for (location, xml) in document.story_range_paragraphs()? {
        if location == range.range().start.location {
            selected = true;
        }
        if !selected {
            continue;
        }
        if location.story() != range.range().start.location.story() {
            return Err(Error::Other("REF f target crosses story owners".into()));
        }
        let paragraph = CT_P::from_xml_fragment(&xml)?;
        let runs = paragraph.accepted_bookmark_runs();
        // A marker between blocks leads the paragraph it is read as (run 0)
        // or closes it; any other marker the paragraph does not hold
        // leaves the REF unresolved.
        let between_blocks = |side: Option<bool>, which: &str| match side {
            Some(true) => Ok(0),
            Some(false) => Ok(runs.len()),
            None => Err(Error::Other(format!(
                "REF f target {which} marker is absent from its physical projection"
            ))),
        };
        let first = if location == range.range().start.location {
            paragraph
                .bookmark_markers
                .iter()
                .find(|marker| {
                    marker.is_start()
                        && marker.id() == Some(*id)
                        && marker
                            .name()
                            .is_some_and(|name| same_bookmark_name(name, target))
                })
                .map_or_else(
                    || between_blocks(start_side, "start"),
                    |marker| Ok(marker.projected_run_index()),
                )?
        } else {
            0
        };
        let last = if location == range.range().end.location {
            paragraph
                .bookmark_markers
                .iter()
                .find(|marker| !marker.is_start() && marker.id() == Some(*id))
                .map_or_else(
                    || between_blocks(end_side, "end"),
                    |marker| Ok(marker.projected_run_index()),
                )?
        } else {
            runs.len()
        };
        if !cache.runs.is_empty() {
            let mut line = CT_R::new("");
            line.content = vec![RunContent::Break(
                oxoffice_docx_oxml::text::BreakType::Line,
                oxoffice_docx_oxml::text::BreakClear::None,
            )];
            cache.runs.push(line);
        }
        append_ref_target_paragraph(&paragraph, first, last, &mut cache)?;
        if location == range.range().end.location {
            return Ok(Some(cache));
        }
    }
    Err(Error::Other("REF f related target owner is absent".into()))
}
