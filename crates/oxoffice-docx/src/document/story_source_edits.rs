//! Writing story sources: note fragments, appended fragments and namespace scopes.

use super::*;

pub(crate) fn set_story_source_xml(
    document: &mut Document,
    part_name: &str,
    xml: Vec<u8>,
) -> Result<()> {
    set_story_source_xml_in(document, part_name, xml, None)
}

pub(super) fn set_story_source_xml_in(
    document: &mut Document,
    part_name: &str,
    xml: Vec<u8>,
    mut scoped: Option<(
        &mut TemplateMainWriter<'_, '_>,
        oxoffice_docx_oxml::document::ScopedWordSource,
    )>,
) -> Result<()> {
    if part_name == document.doc_part_name {
        // The main-part story source is canonical XML. It drops a root
        // default namespace and every body declaration, and binds the root
        // `w`, `r` and `mc` prefixes to their canonical URIs. The save path's
        // check on those declarations keeps retained content of the replaced
        // part in its namespace. A checked paragraph write emits canonical
        // `w14:paraId` without a local declaration, so the part root binds
        // that prefix before this source is parsed again.
        let rewritten_root_declarations: Vec<_> = document
            .root_namespace_declarations
            .iter()
            .filter(|(name, _)| {
                matches!(name.as_str(), "xmlns" | "xmlns:w" | "xmlns:r" | "xmlns:mc")
            })
            .cloned()
            .collect();
        if let Some(prefix) = unsafe_serializer_namespace_prefix(
            &rewritten_root_declarations,
            &document.body_namespace_declarations,
            document.package.get_part(part_name),
        ) {
            return Err(Error::Other(format!(
                "cannot serialize a modified document with a shadowed `{prefix}` namespace"
            )));
        }
        let (xml, mut main_scoped) = if let Some((template, source)) = scoped.take() {
            let source = template.operation.declare_source_w14_on_root(source)?;
            let retained = template.operation.repeat_source(&source)?;
            (
                template.operation.finish_source(retained)?,
                Some((template, source)),
            )
        } else {
            let mut xml = xml;
            declare_w14_on_part_root(&mut xml)?;
            (xml, None)
        };
        let namespace_scopes = document_namespace_scopes(&xml)?;
        let xml = Arc::new(xml);
        let rebuilt = if main_scoped.is_none() {
            document.source_blocks(&xml)
        } else {
            None
        };
        let (parsed, blocks) = match rebuilt {
            Some((parsed, blocks)) => (parsed, Some(blocks)),
            None => {
                let parsed = if let Some((template, source)) = main_scoped.take() {
                    template.sources.invalidate_model();
                    let parsed = template
                        .sources
                        .read_document(template.operation, Arc::new(source))?;
                    template.sources.published_main = Some((
                        Arc::clone(&xml),
                        Arc::clone(
                            template
                                .sources
                                .reader_source
                                .as_ref()
                                .expect("actual main reader input"),
                        ),
                    ));
                    parsed
                } else {
                    CT_Document::from_xml(&xml)?
                };
                (parsed, None)
            }
        };
        #[cfg(debug_assertions)]
        if blocks.is_some() {
            {
                let whole = CT_Document::from_xml(&xml).expect("proved source parses whole");
                assert_eq!(parsed, whole, "source pieces changed the parsed model");
                assert_eq!(
                    parsed.to_xml().ok(),
                    whole.to_xml().ok(),
                    "source normalization changed"
                );
            }
        }
        let nested_namespace_owners = OnceLock::new();
        if let Some(owners) = blocks
            .as_ref()
            .and_then(|blocks| blocks.retained_nested_owners(&xml))
        {
            let _ = nested_namespace_owners.set(Some(owners));
        }
        let (blocked, unsavable) = plain_local_edit_facts(
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            &xml,
            &nested_namespace_owners,
        );
        document.plain_local_edit_blocked = blocked;
        document.unsavable_body_edit_cache = unsavable;
        document.document = crate::document::DocumentModel::unproved(parsed);
        document.main_part_blocks = blocks;
        document.package.set_part(part_name, xml);
        document.root_namespace_declarations = namespace_scopes.root_declarations;
        document.body_namespace_declarations = namespace_scopes.body_declarations;
        document.body_namespace_bindings = namespace_scopes.body_bindings;
        document.local_cell_source = LocalCellSource::classified(
            &document.package,
            &document.doc_part_name,
            &document.root_namespace_declarations,
            &document.body_namespace_declarations,
            !blocked,
        );
    } else if document.comments_part_name.as_deref() == Some(part_name) {
        let mut comments = if let Some((writer, source)) = scoped.as_mut() {
            let comments = writer.operation.source_comments(source)?;
            // The operation reader and package publication below are the same producer handback.
            comments
        } else {
            oxoffice_docx_oxml::comments::CT_Comments::from_xml(&xml)?
        };
        let changed = document.reconcile_comment_paragraph_publication(&mut comments)?;
        document.comments = Some(comments);
        document.package.set_part(part_name, xml);
        // Existing ordinary/scoped model writer emits the generated attributes.
        document.comments_dirty = changed;
    } else if document.footnotes_part_name.as_deref() == Some(part_name) {
        document.footnotes = if let Some((template, source)) = scoped.as_mut() {
            let notes = template.operation.source_notes(source)?;
            template.sources.notes_model_read = true;
            template.sources.read_note_count = notes.footnotes.len();
            notes
        } else {
            oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&xml)?
        };
        document.package.set_part(part_name, xml);
        document.footnotes_dirty = false;
    } else if document.glossary_part_name.as_deref() == Some(part_name) {
        document.glossary = Some(oxoffice_docx_oxml::glossary::CT_GlossaryDocument::from_xml(
            &xml,
        )?);
        document.package.set_part(part_name, xml);
        document.glossary_dirty = false;
    } else {
        document.package.set_part(part_name, xml);
    }
    if let Some((template, source)) = scoped {
        let bytes = document
            .package
            .get_part_shared(part_name)
            .ok_or_else(|| Error::Other("published template part disappeared".into()))?;
        let source = Arc::new(source);
        if document.comments_part_name.as_deref() == Some(part_name) {
            template.sources.comments_reader_source = Some(Arc::clone(&source));
        }
        template
            .sources
            .put_related_source(part_name, bytes, source);
    }
    Ok(())
}

// Source-qualified types, rather than the legacy typed unknown-type fallback,
// decide whether an imported part already owns an authored special record.
pub(super) fn missing_note_specials(
    existing: &oxoffice_docx_oxml::footnotes::CT_Footnotes,
    source: Option<&[u8]>,
    kind: StoryKind,
    pending_normal_id: Option<i32>,
) -> Result<Vec<oxoffice_docx_oxml::footnotes::CT_Footnote>> {
    use crate::comparison::{RawNoteType, raw_note_attribute_in_scope, raw_note_type_in_scope};
    use oxoffice_docx_oxml::footnotes::{CT_Footnote, NoteType};
    let mut separator = false;
    let mut continuation = false;
    let mut notes = existing
        .footnotes
        .iter()
        .any(|note| note.note_type == NoteType::Normal);
    let mut occupied = existing
        .footnotes
        .iter()
        .map(|note| note.id)
        .collect::<HashSet<_>>();
    if let Some(id) = pending_normal_id {
        occupied.insert(id);
    }
    if let Some(source) = source {
        for owner in scan_story_owners_with_special(source, kind, true)? {
            if owner.kind != kind {
                continue;
            }
            let scope = story_namespace_scope_at(source, owner.full.start)?;
            let xml = std::str::from_utf8(&source[owner.full])
                .map_err(|error| Error::Other(format!("note source is not UTF-8: {error}")))?;
            let bindings = scope
                .into_iter()
                .map(|(prefix, uri)| {
                    (
                        if prefix.is_empty() {
                            "xmlns".into()
                        } else {
                            format!("xmlns:{prefix}")
                        },
                        uri,
                    )
                })
                .collect::<Vec<_>>();
            if let Some(id) = raw_note_attribute_in_scope(xml, &bindings, "id")?
                .and_then(|id| id.parse::<i32>().ok())
            {
                occupied.insert(id);
            }
            match raw_note_type_in_scope(xml, &bindings)? {
                RawNoteType::Separator => separator = true,
                RawNoteType::ContinuationSeparator => continuation = true,
                RawNoteType::Omitted | RawNoteType::ExplicitNormal => notes = true,
                _ => {}
            }
        }
    }
    // A part that already holds notes and no separator comes from a producer
    // that writes none (Google Docs numbers its notes from 0): Word draws its
    // own separators there, and adding ours would change the file's form for
    // nothing, and every comparison with the file it came from.
    if notes && !separator && !continuation {
        return Ok(Vec::new());
    }
    let mut additions = Vec::new();
    for (id, note_type, present, child) in [
        (
            -1,
            NoteType::Separator,
            separator,
            b"<w:separator/>".as_slice(),
        ),
        (
            0,
            NoteType::ContinuationSeparator,
            continuation,
            b"<w:continuationSeparator/>".as_slice(),
        ),
    ] {
        if present || occupied.contains(&id) {
            continue;
        }
        let mut run = CT_R::new("");
        run.content.clear();
        run.extra_xml.push(child.to_vec());
        run.extra_xml_positions.push(0);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(run);
        additions.push(CT_Footnote {
            id,
            note_type,
            paragraphs: vec![paragraph],
        });
    }
    Ok(additions)
}

/// Marks in `found` each character style of `wanted` that a run of these
/// owners, or of the revisions they hold, uses; stops once every one is.
pub(super) fn owners_reference_styles(
    mut pending: Vec<WordXmlOwner<'_>>,
    wanted: &[&str],
    found: &[Cell<bool>],
) {
    let mark = |run: &CT_R| {
        if let Some(style) = run
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref())
            && let Some(index) = wanted.iter().position(|id| *id == style)
        {
            found[index].set(true);
        }
    };
    fn revision_runs(revision: &CT_Revision, mark: &dyn Fn(&CT_R)) {
        if let Some(paragraph) = revision.content_paragraph() {
            paragraph.source_runs().into_iter().for_each(mark);
            return;
        }
        if let oxoffice_docx_oxml::revision::RevisionContent::Runs(runs) = revision.content() {
            runs.iter().for_each(mark);
        }
        for (_, nested) in revision.nested_revisions().iter() {
            revision_runs(nested, mark);
        }
    }
    while let Some(owner) = pending.pop() {
        if found.iter().all(Cell::get) {
            return;
        }
        match owner {
            WordXmlOwner::Paragraph(paragraph) => {
                paragraph.source_runs().into_iter().for_each(&mark);
            }
            WordXmlOwner::Run(run) => mark(run),
            WordXmlOwner::Table(table) => {
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
                pending.extend(cell.content.iter().map(|item| match item {
                    CellContent::Paragraph(paragraph) => WordXmlOwner::Paragraph(paragraph),
                    CellContent::Table(table) => WordXmlOwner::Table(table),
                    CellContent::ContentControl(control) => WordXmlOwner::Control(control),
                }));
            }
            WordXmlOwner::Control(control) => {
                for (_, revision) in control.revisions().iter() {
                    revision_runs(revision, &mark);
                }
                for item in &control.content {
                    match item {
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
            }
        }
    }
}

/// The superscript mark style of a footnote or an endnote reference, as
/// Word defines it: based on the default paragraph font when the document
/// has one, priority 99, hidden until used.
pub(super) fn note_reference_style(kind: StoryKind, styles: &CT_Styles) -> StyleBuilder {
    let (id, name) = if kind == StoryKind::Endnote {
        ("EndnoteReference", "endnote reference")
    } else {
        ("FootnoteReference", "footnote reference")
    };
    let builder = StyleBuilder::character(id, name)
        .priority(99)
        .semi_hidden(true)
        .unhide_when_used(true)
        .run_properties(CT_RPr {
            vert_align: Some("superscript".into()),
            ..Default::default()
        });
    match default_character_style_id(styles) {
        Some(base) => builder.based_on(&base),
        None => builder,
    }
}

/// The document's default character font style, by its default flag.
fn default_character_style_id(styles: &CT_Styles) -> Option<String> {
    styles
        .styles
        .iter()
        .find(|style| style.style_type == StyleType::Character && style.is_default)
        .map(|style| style.style_id.clone())
}

/// The id of a built-in character style: the character style carrying its
/// built-in name (a localized Word file names `FootnoteReference`
/// `Appelnotedebasdep`), else the character style with its id, else `None`.
pub(super) fn built_in_character_style_id(
    styles: &CT_Styles,
    id: &str,
    name: &str,
) -> Option<String> {
    let character =
        |style: &&oxoffice_docx_oxml::styles::CT_Style| style.style_type == StyleType::Character;
    styles
        .styles
        .iter()
        .filter(character)
        .find(|style| {
            style
                .name
                .as_deref()
                .is_some_and(|known| known.eq_ignore_ascii_case(name))
        })
        .or_else(|| {
            styles
                .styles
                .iter()
                .filter(character)
                .find(|style| style.style_id == id)
        })
        .map(|style| style.style_id.clone())
}

pub(super) fn serialized_note_fragment(
    note: &oxoffice_docx_oxml::footnotes::CT_Footnote,
    kind: StoryKind,
) -> Result<Vec<u8>> {
    let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes {
        footnotes: vec![note.clone()],
    };
    let serialized = match kind {
        StoryKind::Footnote => notes.to_xml_footnotes()?,
        StoryKind::Endnote => notes.to_xml_endnotes()?,
        _ => return Err(Error::Other("invalid note story kind".to_owned())),
    };
    let owner = scan_story_owners_with_special(&serialized, kind, true)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("serialized note has no story owner".to_owned()))?;
    let scope = story_namespace_scope_at(&serialized, owner.full.start)?;
    close_content_fragment_namespaces(&serialized[owner.full], &scope)
}

pub(super) fn serialized_note_fragment_in(
    note: &oxoffice_docx_oxml::footnotes::CT_Footnote,
    kind: StoryKind,
    note_index: usize,
    read_source: Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
    template: &mut TemplateMainWriter<'_, '_>,
) -> Result<Option<oxoffice_docx_oxml::document::ScopedWordSource>> {
    let endnotes = match kind {
        StoryKind::Footnote => false,
        StoryKind::Endnote => true,
        _ => return Err(Error::Other("invalid note story kind".into())),
    };
    let emitted = if let Some(source) = read_source {
        template
            .operation
            .source_model_note_from_read(note, endnotes, source, note_index)?
    } else {
        if template.sources.notes_model_read {
            return Ok(None);
        }
        // Preserve the existing one-note clone producer. Only actual initial
        // clone slots can be paired, never a note ID or matching text value.
        for paragraph in &note.paragraphs {
            paragraph.validate_resource_limits()?;
        }
        let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes {
            footnotes: vec![note.clone()],
        };
        let Some(inputs) = template
            .lineage
            .note_writer_inputs(note_index, &notes.footnotes[0])
        else {
            return Ok(None);
        };
        template.operation.source_model_notes(
            &notes,
            endnotes,
            &inputs.texts,
            &inputs.fields,
            &inputs.raw,
            false,
        )?
    };
    let owner = scan_story_owners_with_special(emitted.as_bytes(), kind, true)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("serialized note has no story owner".into()))?;
    let scope = story_namespace_scope_at(emitted.as_bytes(), owner.full.start)?;
    let selected = template
        .operation
        .compose_sources(&[(Some(&emitted), owner.full, &[])])?;
    Ok(Some(
        template
            .operation
            .close_source_namespaces(selected, &scope)?,
    ))
}

pub(super) fn serialized_footnote_fragment(
    footnote: &oxoffice_docx_oxml::footnotes::CT_Footnote,
) -> Result<Vec<u8>> {
    serialized_note_fragment(footnote, StoryKind::Footnote)
}

pub(super) fn remove_note_references(xml: &[u8], id: i32, kind: StoryKind) -> Result<Vec<u8>> {
    let local_name = match kind {
        StoryKind::Footnote => b"footnoteReference".as_slice(),
        StoryKind::Endnote => b"endnoteReference".as_slice(),
        _ => return Err(Error::Other("invalid note story kind".to_owned())),
    };
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut ranges = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("footnote reference scan failed: {error}")))?;
        let start = match &event {
            Event::Start(start) | Event::Empty(start)
                if word_element(&namespace) && start.local_name().as_ref() == local_name =>
            {
                start
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let mut reference_id = None;
        let mut custom_mark_follows = false;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| {
                Error::Other(format!("footnote reference attribute scan failed: {error}"))
            })?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if !word_element(&namespace) {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!("footnote reference attribute is invalid: {error}"))
                })?;
            match local.as_ref() {
                b"id" => reference_id = value.parse::<i32>().ok(),
                b"customMarkFollows" => {
                    custom_mark_follows = matches!(value.as_ref(), "1" | "true" | "on");
                }
                _ => {}
            }
        }
        if reference_id == Some(id) {
            let end = if matches!(event, Event::Empty(_)) {
                reader.buffer_position() as usize
            } else {
                story_element_end(xml, before)?
            };
            let event = event.into_owned();
            let mut mark_range = None;
            if custom_mark_follows {
                // The custom mark is the text, deleted text or symbol that
                // follows the reference in its run, as Word reads it: it
                // goes with the note. Whitespace, XML comments, processing
                // instructions and a rendered page break between them are
                // passed over and kept.
                if let Event::Start(reference) = &event {
                    let name = reference.name().as_ref().to_vec();
                    reader
                        .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("custom mark scan failed: {error}"))
                        })?;
                }
                let mut next_buffer = Vec::new();
                loop {
                    let offset = reader.buffer_position() as usize;
                    let (namespace, next) = reader
                        .read_resolved_event_into(&mut next_buffer)
                        .map_err(|error| {
                            Error::Other(format!("custom mark scan failed: {error}"))
                        })?;
                    match next {
                        Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
                        Event::Comment(_) | Event::PI(_) => {}
                        Event::Empty(hint)
                            if word_element(&namespace)
                                && hint.local_name().as_ref() == b"lastRenderedPageBreak" => {}
                        Event::Start(hint)
                            if word_element(&namespace)
                                && hint.local_name().as_ref() == b"lastRenderedPageBreak" =>
                        {
                            let name = hint.name().as_ref().to_vec();
                            reader
                                .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                                .map_err(|error| {
                                    Error::Other(format!("custom mark scan failed: {error}"))
                                })?;
                        }
                        Event::Start(mark)
                            if word_element(&namespace)
                                && matches!(
                                    mark.local_name().as_ref(),
                                    b"t" | b"delText" | b"sym"
                                ) =>
                        {
                            mark_range = Some(offset..story_element_end(xml, offset)?);
                            let name = mark.name().as_ref().to_vec();
                            reader
                                .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                                .map_err(|error| {
                                    Error::Other(format!("custom mark scan failed: {error}"))
                                })?;
                            break;
                        }
                        Event::Empty(mark)
                            if word_element(&namespace)
                                && matches!(
                                    mark.local_name().as_ref(),
                                    b"t" | b"delText" | b"sym"
                                ) =>
                        {
                            mark_range = Some(offset..reader.buffer_position() as usize);
                            break;
                        }
                        _ => break,
                    }
                    next_buffer.clear();
                }
            }
            ranges.push(before..end);
            ranges.extend(mark_range);
        }
        buffer.clear();
    }
    let mut updated = xml.to_vec();
    for range in ranges.into_iter().rev() {
        updated.drain(range);
    }
    Ok(updated)
}

enum StoryAppendPosition {
    At(usize),
    Empty {
        insertion: usize,
        after: usize,
        name: Vec<u8>,
    },
}

fn story_append_position(
    xml: &[u8],
    root_local_name: &[u8],
    item_local_name: &[u8],
) -> Result<StoryAppendPosition> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut after_last_item = None;
    let boundary = loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid footnotes XML: {error}")))?;
        let is_word = word_element(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                if !saw_root {
                    if is_word && element.local_name().as_ref() == root_local_name {
                        saw_root = true;
                        depth = 1;
                    }
                } else {
                    if depth == 1 && is_word && element.local_name().as_ref() == item_local_name {
                        after_last_item = Some(story_element_end(xml, before)?);
                    }
                    depth += 1;
                }
            }
            Event::Empty(element) => {
                if !saw_root && is_word && element.local_name().as_ref() == root_local_name {
                    let name = element.name().as_ref().to_vec();
                    let insertion = after.checked_sub(2).ok_or_else(|| {
                        Error::Other("empty story root opening tag is incomplete".to_owned())
                    })?;
                    return Ok(StoryAppendPosition::Empty {
                        insertion,
                        after,
                        name,
                    });
                }
                if saw_root
                    && depth == 1
                    && is_word
                    && element.local_name().as_ref() == item_local_name
                {
                    after_last_item = Some(after);
                }
            }
            Event::End(element) => {
                if saw_root
                    && depth == 1
                    && is_word
                    && element.local_name().as_ref() == root_local_name
                {
                    break after_last_item.unwrap_or(before);
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => {
                return Err(Error::Other(format!(
                    "story XML has no closing {} root element",
                    String::from_utf8_lossy(root_local_name)
                )));
            }
            _ => {}
        }
        buffer.clear();
    };
    Ok(StoryAppendPosition::At(boundary))
}

pub(super) fn append_story_fragments_to_root(
    xml: &[u8],
    root_local_name: &[u8],
    item_local_name: &[u8],
    fragments: &[Vec<u8>],
) -> Result<Vec<u8>> {
    if fragments.is_empty() {
        return Ok(xml.to_vec());
    }
    match story_append_position(xml, root_local_name, item_local_name)? {
        StoryAppendPosition::At(boundary) => {
            let mut updated = xml.to_vec();
            updated.splice(
                boundary..boundary,
                fragments
                    .iter()
                    .flat_map(|fragment| fragment.iter().copied()),
            );
            Ok(updated)
        }
        StoryAppendPosition::Empty {
            insertion,
            after,
            name,
        } => {
            let mut updated = Vec::new();
            updated.extend_from_slice(&xml[..insertion]);
            updated.push(b'>');
            updated.extend(
                fragments
                    .iter()
                    .flat_map(|fragment| fragment.iter().copied()),
            );
            updated.extend_from_slice(b"</");
            updated.extend_from_slice(&name);
            updated.push(b'>');
            updated.extend_from_slice(&xml[after..]);
            Ok(updated)
        }
    }
}

pub(super) fn append_story_sources_to_root(
    operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    source: impl Into<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    fragments: &[Arc<oxoffice_docx_oxml::document::ScopedWordSource>],
    root_local_name: &[u8],
    item_local_name: &[u8],
) -> Result<Arc<oxoffice_docx_oxml::document::ScopedWordSource>> {
    let source = source.into();
    if fragments.is_empty() {
        return Ok(source);
    }
    let position = story_append_position(source.as_bytes(), root_local_name, item_local_name)?;
    let mut chunks = Vec::new();
    let close;
    match position {
        StoryAppendPosition::At(boundary) => {
            chunks.push((Some(source.as_ref()), 0..boundary, &[][..]));
            for fragment in fragments {
                chunks.push((
                    Some(fragment.as_ref()),
                    0..fragment.as_bytes().len(),
                    &[][..],
                ));
            }
            chunks.push((
                Some(source.as_ref()),
                boundary..source.as_bytes().len(),
                &[][..],
            ));
        }
        StoryAppendPosition::Empty {
            insertion,
            after,
            name,
        } => {
            close = [b"</".as_slice(), name.as_slice(), b">".as_slice()].concat();
            chunks.push((Some(source.as_ref()), 0..insertion, &[][..]));
            chunks.push((None, 0..0, b">".as_slice()));
            for fragment in fragments {
                chunks.push((
                    Some(fragment.as_ref()),
                    0..fragment.as_bytes().len(),
                    &[][..],
                ));
            }
            chunks.push((None, 0..0, close.as_slice()));
            chunks.push((
                Some(source.as_ref()),
                after..source.as_bytes().len(),
                &[][..],
            ));
        }
    }
    Ok(Arc::new(operation.compose_sources(&chunks)?))
}

pub(super) fn update_story_namespace_scope(
    scope: &mut BTreeMap<String, String>,
    element: &BytesStart<'_>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace scope attribute failed: {error}"))
        })?;
        let Some((prefix, value)) = decoded_story_namespace_declaration(element, &attribute)?
        else {
            continue;
        };
        if value.is_empty() {
            scope.remove(&prefix);
        } else {
            scope.insert(prefix, value);
        }
    }
    Ok(())
}

pub(super) fn decoded_story_namespace_declaration(
    element: &BytesStart<'_>,
    attribute: &quick_xml::events::attributes::Attribute<'_>,
) -> Result<Option<(String, String)>> {
    let name = attribute.key.as_ref();
    let prefix = if name == b"xmlns" {
        ""
    } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
        std::str::from_utf8(prefix).map_err(|error| {
            Error::Other(format!("story namespace scope prefix is invalid: {error}"))
        })?
    } else {
        return Ok(None);
    };
    let value = attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
        .map_err(|error| Error::Other(format!("story namespace scope value is invalid: {error}")))?
        .into_owned();
    Ok(Some((prefix.to_owned(), value)))
}

pub(super) fn story_word_prefix_for_scope(scope: &BTreeMap<String, String>) -> String {
    scope
        .iter()
        .find_map(|(prefix, namespace)| (namespace == WORD_NAMESPACE).then_some(prefix.clone()))
        .unwrap_or_else(|| {
            let mut suffix = 0usize;
            loop {
                let candidate = if suffix == 0 {
                    "rdocxw".to_owned()
                } else {
                    format!("rdocxw{suffix}")
                };
                if !scope.contains_key(&candidate) {
                    break candidate;
                }
                suffix += 1;
            }
        })
}
