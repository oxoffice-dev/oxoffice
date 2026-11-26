//! Fields in headers, footers, notes and text boxes: reading and patching their sources.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum PackageStoryKind {
    Document,
    Header,
    Footer,
    Footnotes,
    Endnotes,
    Comments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LegacyBlockOwner {
    Root,
    Table,
    Row,
    Cell,
}

#[derive(Clone, Copy)]
enum LegacyStoryElementKind {
    DocumentRoot,
    BodyRoot,
    HeaderFooterRoot,
    FootnotesRoot,
    EndnotesRoot,
    CommentsRoot,
    NormalComment,
    NormalNote,
    Table,
    Row,
    Cell,
    ContentControl(LegacyBlockOwner, bool),
    ContentControlContent(LegacyBlockOwner),
    Other,
}

pub(super) fn parsed_physical_field_paragraph(xml: &[u8]) -> Result<CT_P> {
    let mut body = CT_Body::default();
    body.content
        .push(BodyContent::Paragraph(Box::new(CT_P::from_xml_fragment(
            xml,
        )?)));
    prepare_physical_story_projection(&mut body, &mut [])?;
    let BodyContent::Paragraph(paragraph) = body.content.remove(0) else {
        unreachable!()
    };
    Ok(*paragraph)
}

pub(super) fn legacy_story_paragraphs(
    xml: &[u8],
    story_kind: PackageStoryKind,
) -> Result<Vec<LegacyStoryParagraph>> {
    validate_story_document_declarations_and_doctype(xml)?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<LegacyStoryElementKind>::new();
    let mut bindings = BTreeMap::<String, Vec<u8>>::new();
    let mut binding_scopes = Vec::new();
    let mut paragraphs = Vec::new();
    let mut path = Vec::new();
    let mut path_lengths = Vec::new();
    let mut child_counts = Vec::<usize>::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let general_context = |depth: usize| match story_kind {
        PackageStoryKind::Header | PackageStoryKind::Footer => depth == 1,
        PackageStoryKind::Footnotes | PackageStoryKind::Endnotes => depth == 2,
        PackageStoryKind::Comments | PackageStoryKind::Document => false,
    };
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package story XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if stack.is_empty() {
                    if root_seen || !package_story_root_matches(story_kind, word, local) {
                        return Err(Error::Other(
                            "package story must contain exactly one relationship-appropriate root"
                                .to_owned(),
                        ));
                    }
                    root_seen = true;
                }
                let kind = legacy_story_element_kind(
                    story_kind,
                    &stack,
                    word,
                    local,
                    &element,
                    reader.resolver(),
                );
                if matches!(kind, Some(LegacyStoryElementKind::Other))
                    && word
                    && (local == b"p"
                        || (local == b"sdtContent"
                            && matches!(
                                stack.last(),
                                Some(LegacyStoryElementKind::ContentControl(_, true))
                            )))
                {
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("invalid package story paragraph: {error}"))
                        })?;
                } else if kind.is_none() && word && local == b"p" {
                    let start_after = reader.buffer_position() as usize;
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("invalid package story paragraph: {error}"))
                        })?;
                    let end = reader.buffer_position() as usize;
                    let fragment = paragraph_fragment_with_bindings(
                        &xml[before..end],
                        start_after - before,
                        &bindings,
                    )?;
                    let paragraph = parsed_physical_field_paragraph(&fragment)?;
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field paragraph has no source owner".into())
                    })?;
                    let mut paragraph_path = path.clone();
                    paragraph_path.push(("p", *index));
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    paragraphs.push(LegacyStoryParagraph {
                        path: paragraph_path,
                        start: before,
                        end,
                        general_context: general_context(stack.len()),
                        original: paragraph.clone(),
                        paragraph,
                    });
                } else {
                    if matches!(kind, Some(LegacyStoryElementKind::ContentControlContent(_)))
                        && let Some(LegacyStoryElementKind::ContentControl(_, seen)) =
                            stack.last_mut()
                    {
                        *seen = true;
                    }
                    let mut local_bindings = bindings.clone();
                    apply_namespace_declarations(&element, &mut local_bindings)?;
                    binding_scopes.push(std::mem::replace(&mut bindings, local_bindings));
                    let kind = kind.unwrap_or(LegacyStoryElementKind::Other);
                    path_lengths.push(path.len());
                    let token = match kind {
                        LegacyStoryElementKind::Table => Some("tbl"),
                        LegacyStoryElementKind::Row => Some("tr"),
                        LegacyStoryElementKind::Cell => Some("tc"),
                        LegacyStoryElementKind::ContentControl(_, _) => Some("sdt"),
                        _ => None,
                    };
                    if let Some(token) = token {
                        let index = child_counts.last_mut().ok_or_else(|| {
                            Error::Other("field source container has no parent".into())
                        })?;
                        path.push((token, *index));
                        *index = index
                            .checked_add(1)
                            .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    }
                    child_counts.push(0);
                    stack.push(kind);
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err(Error::Other(
                        "package story XML has an unmatched end element".to_owned(),
                    ));
                }
                bindings = binding_scopes.pop().unwrap_or_default();
                path.truncate(
                    path_lengths
                        .pop()
                        .ok_or_else(|| Error::Other("field source path stack underflow".into()))?,
                );
                child_counts.pop();
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            // A self-closing paragraph is a story paragraph too, as the
            // layout source inventory counts it.
            Event::Empty(element) if !stack.is_empty() => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if word
                    && local == b"p"
                    && legacy_story_element_kind(
                        story_kind,
                        &stack,
                        word,
                        local,
                        &element,
                        reader.resolver(),
                    )
                    .is_none()
                {
                    let end = reader.buffer_position() as usize;
                    let fragment = paragraph_fragment_with_bindings(
                        &xml[before..end],
                        end - before,
                        &bindings,
                    )?;
                    let paragraph = parsed_physical_field_paragraph(&fragment)?;
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field paragraph has no source owner".into())
                    })?;
                    let mut paragraph_path = path.clone();
                    paragraph_path.push(("p", *index));
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    paragraphs.push(LegacyStoryParagraph {
                        path: paragraph_path,
                        start: before,
                        end,
                        general_context: general_context(stack.len()),
                        original: paragraph.clone(),
                        paragraph,
                    });
                } else if matches!(
                    legacy_story_element_kind(
                        story_kind,
                        &stack,
                        word,
                        local,
                        &element,
                        reader.resolver()
                    ),
                    Some(
                        LegacyStoryElementKind::Table
                            | LegacyStoryElementKind::Row
                            | LegacyStoryElementKind::Cell
                            | LegacyStoryElementKind::ContentControl(_, _)
                    )
                ) {
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field source empty container has no owner".into())
                    })?;
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                }
            }
            Event::Empty(element) if stack.is_empty() => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if root_seen || !package_story_root_matches(story_kind, word, local) {
                    return Err(Error::Other(
                        "package story must contain exactly one relationship-appropriate root"
                            .to_owned(),
                    ));
                }
                root_seen = true;
                root_closed = true;
            }
            Event::Text(text)
                if stack.is_empty() && text.iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Err(Error::Other(
                    "package story has non-whitespace text outside its root".to_owned(),
                ));
            }
            Event::CData(_) if stack.is_empty() => {
                return Err(Error::Other(
                    "package story has character data outside its root".to_owned(),
                ));
            }
            Event::GeneralRef(_) if stack.is_empty() => {
                return Err(Error::Other(
                    "package story has a character reference outside its root".to_owned(),
                ));
            }
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(Error::Other(
                        "package story XML has an unclosed element".to_owned(),
                    ));
                }
                if !root_seen || !root_closed {
                    return Err(Error::Other(
                        "package story must contain exactly one relationship-appropriate root"
                            .to_owned(),
                    ));
                }
                return Ok(paragraphs);
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn validate_story_document_declarations_and_doctype(xml: &[u8]) -> Result<()> {
    validate_strict_xml_1_0(xml).map_err(package_story_lexical_error)?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut declaration_allowed = true;
    let mut declaration_seen = false;
    let mut depth = 0usize;
    loop {
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package story XML: {error}")))?;
        let event = event.into_owned();
        match event {
            Event::Decl(_) => {
                if !declaration_allowed || declaration_seen {
                    return Err(Error::Other(
                        "misplaced or duplicate package story XML declaration".to_owned(),
                    ));
                }
                declaration_seen = true;
                declaration_allowed = false;
            }
            Event::DocType(_) => {
                return Err(Error::Other(
                    "package story XML cannot contain a document type".to_owned(),
                ));
            }
            Event::Start(_) => {
                declaration_allowed = false;
                depth += 1;
            }
            Event::Empty(_) => {
                declaration_allowed = false;
            }
            Event::End(_) => {
                declaration_allowed = false;
                depth = depth.saturating_sub(1);
            }
            Event::GeneralRef(_) => {
                declaration_allowed = false;
            }
            Event::Eof => return Ok(()),
            _ => declaration_allowed = false,
        }
        buffer.clear();
    }
}

fn package_story_lexical_error(error: XmlLexicalError) -> Error {
    let message = match error {
        XmlLexicalError::InvalidUtf8 => "invalid package story XML: input is not UTF-8".to_owned(),
        XmlLexicalError::InvalidDeclaration(reason) if reason == "must begin with version" => {
            "package story XML declaration must begin with version".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason)
            if reason == "attributes are invalid, duplicated, or out of order" =>
        {
            "invalid package story XML declaration".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason) => {
            format!("invalid package story XML declaration: {reason}")
        }
        XmlLexicalError::ForbiddenLiteralCharacter => {
            "package story XML contains a forbidden literal XML 1.0 character".to_owned()
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("qualified name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) => format!("invalid package story XML: {reason}"),
        XmlLexicalError::InvalidNamespace(reason)
            if reason == "element uses the reserved xmlns prefix" =>
        {
            "package story XML element uses the reserved xmlns prefix".to_owned()
        }
        XmlLexicalError::InvalidNamespace(reason) if reason == "invalid namespace declaration" => {
            "package story XML contains an invalid namespace declaration".to_owned()
        }
        XmlLexicalError::InvalidNamespace(_) => {
            "package story XML uses an unbound namespace prefix".to_owned()
        }
        XmlLexicalError::DuplicateExpandedAttribute => {
            "package story XML element has duplicate expanded-name attributes".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "attribute contains a literal less-than sign" =>
        {
            "package story XML attribute contains a literal less-than sign".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "character reference is not legal in XML 1.0" =>
        {
            "package story character reference is not legal in XML 1.0".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason.starts_with("undeclared entity reference ") =>
        {
            reason.replacen(
                "undeclared entity reference ",
                "undeclared package story XML entity reference ",
                1,
            )
        }
        XmlLexicalError::InvalidReference(reason) => {
            format!("invalid package story XML: {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason)
            if reason == "reserved XML target" =>
        {
            "reserved package story XML processing instruction".to_owned()
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) if reason.starts_with("name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) => {
            format!("invalid package story XML processing instruction: {reason}")
        }
        XmlLexicalError::InvalidComment(reason) => format!("invalid package story XML: {reason}"),
    };
    Error::Other(message)
}

fn legacy_story_element_kind(
    story_kind: PackageStoryKind,
    stack: &[LegacyStoryElementKind],
    word: bool,
    local: &[u8],
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
) -> Option<LegacyStoryElementKind> {
    if stack.is_empty() {
        return Some(match story_kind {
            PackageStoryKind::Document if word && local == b"document" => {
                LegacyStoryElementKind::DocumentRoot
            }
            PackageStoryKind::Header if word && local == b"hdr" => {
                LegacyStoryElementKind::HeaderFooterRoot
            }
            PackageStoryKind::Footer if word && local == b"ftr" => {
                LegacyStoryElementKind::HeaderFooterRoot
            }
            PackageStoryKind::Footnotes if word && local == b"footnotes" => {
                LegacyStoryElementKind::FootnotesRoot
            }
            PackageStoryKind::Endnotes if word && local == b"endnotes" => {
                LegacyStoryElementKind::EndnotesRoot
            }
            PackageStoryKind::Comments if word && local == b"comments" => {
                LegacyStoryElementKind::CommentsRoot
            }
            _ => LegacyStoryElementKind::Other,
        });
    }
    if word
        && matches!(
            (story_kind, stack.last()),
            (
                PackageStoryKind::Footnotes,
                Some(LegacyStoryElementKind::FootnotesRoot)
            ) | (
                PackageStoryKind::Endnotes,
                Some(LegacyStoryElementKind::EndnotesRoot)
            )
        )
        && matches!(local, b"footnote" | b"endnote")
        && note_is_normal(element, resolver)
    {
        return Some(LegacyStoryElementKind::NormalNote);
    }
    if matches!(story_kind, PackageStoryKind::Comments)
        && word
        && local == b"comment"
        && matches!(stack.last(), Some(LegacyStoryElementKind::CommentsRoot))
    {
        return Some(LegacyStoryElementKind::NormalComment);
    }
    if matches!(story_kind, PackageStoryKind::Document)
        && word
        && local == b"body"
        && matches!(stack.last(), Some(LegacyStoryElementKind::DocumentRoot))
    {
        return Some(LegacyStoryElementKind::BodyRoot);
    }
    if let Some(LegacyStoryElementKind::ContentControl(owner, false)) = stack.last()
        && word
        && local == b"sdtContent"
    {
        return Some(LegacyStoryElementKind::ContentControlContent(*owner));
    }
    let owner = match stack.last()? {
        LegacyStoryElementKind::BodyRoot
        | LegacyStoryElementKind::HeaderFooterRoot
        | LegacyStoryElementKind::NormalNote
        | LegacyStoryElementKind::NormalComment => LegacyBlockOwner::Root,
        LegacyStoryElementKind::Table => LegacyBlockOwner::Table,
        LegacyStoryElementKind::Row => LegacyBlockOwner::Row,
        LegacyStoryElementKind::Cell => LegacyBlockOwner::Cell,
        LegacyStoryElementKind::ContentControlContent(owner) => *owner,
        _ => return Some(LegacyStoryElementKind::Other),
    };
    if !word {
        return Some(LegacyStoryElementKind::Other);
    }
    match local {
        b"p" if matches!(owner, LegacyBlockOwner::Root | LegacyBlockOwner::Cell) => None,
        b"tbl" if matches!(owner, LegacyBlockOwner::Root | LegacyBlockOwner::Cell) => {
            Some(LegacyStoryElementKind::Table)
        }
        b"tr" if owner == LegacyBlockOwner::Table => Some(LegacyStoryElementKind::Row),
        b"tc" if owner == LegacyBlockOwner::Row => Some(LegacyStoryElementKind::Cell),
        b"sdt" => Some(LegacyStoryElementKind::ContentControl(owner, false)),
        _ => Some(LegacyStoryElementKind::Other),
    }
}

fn package_story_root_matches(story_kind: PackageStoryKind, word: bool, local: &[u8]) -> bool {
    word && match story_kind {
        PackageStoryKind::Document => local == b"document",
        PackageStoryKind::Header => local == b"hdr",
        PackageStoryKind::Footer => local == b"ftr",
        PackageStoryKind::Footnotes => local == b"footnotes",
        PackageStoryKind::Endnotes => local == b"endnotes",
        PackageStoryKind::Comments => local == b"comments",
    }
}

fn apply_namespace_declarations(
    element: &BytesStart<'_>,
    bindings: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid namespace declaration: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            bindings.insert(
                String::from_utf8_lossy(key).into_owned(),
                attribute.value.as_ref().to_vec(),
            );
        }
    }
    Ok(())
}

fn paragraph_fragment_with_bindings(
    raw: &[u8],
    start_len: usize,
    bindings: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    // Declarations go where the attributes of the start tag end.
    let insert_at = oxoffice_core::xml_element::start_tag(raw)
        .map_err(|_| Error::Other("package story paragraph start tag is missing".to_owned()))?
        .attributes_end;
    let mut output = raw[..insert_at].to_vec();
    for (key, value) in bindings {
        if start_tag_has_raw_attribute(&raw[..start_len], key.as_bytes()) {
            continue;
        }
        output.push(b' ');
        output.extend_from_slice(key.as_bytes());
        output.extend_from_slice(b"=\"");
        output.extend_from_slice(value);
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insert_at..]);
    Ok(output)
}

fn start_tag_has_raw_attribute(start: &[u8], name: &[u8]) -> bool {
    let mut reader = quick_xml::Reader::from_reader(start);
    let mut buffer = Vec::new();
    matches!(
        reader.read_event_into(&mut buffer),
        Ok(Event::Start(element) | Event::Empty(element))
            if element
                .attributes()
                .flatten()
                .any(|attribute| attribute.key.as_ref() == name)
    )
}

pub(super) fn patch_legacy_story_field_sources(
    xml: &[u8],
    paragraphs: &[LegacyStoryParagraph],
    preserve_original_cache_sources: bool,
) -> Result<Vec<u8>> {
    let mut edits = Vec::new();
    for paragraph in paragraphs {
        let mut search_start = 0usize;
        let replacements = if preserve_original_cache_sources {
            paragraph_field_source_replacements(&paragraph.original, &paragraph.paragraph)?
        } else {
            staged_paragraph_field_source_replacements(&paragraph.paragraph)?
        };
        for (source, replacement) in replacements {
            let Some(start) = find_typed_field_source(
                xml,
                paragraph.start,
                paragraph.end,
                &source,
                search_start,
            )?
            else {
                return Err(Error::Other(
                    "package story field source was not found at its typed paragraph boundary"
                        .to_owned(),
                ));
            };
            let end = start + source.len();
            edits.push(FieldSourceEdit {
                start: paragraph.start + start,
                end: paragraph.start + end,
                replacement,
            });
            search_start = end;
        }
        let mut groups = Vec::new();
        append_nested_textbox_groups(&paragraph.paragraph, &mut groups)?;
        if groups.is_empty() {
            continue;
        }
        let window_start = paragraph.start;
        let window_end = paragraph.end;
        let spans = word_txbx_paragraph_spans(xml, &[])?
            .into_iter()
            .filter(|(start, _)| *start >= window_start && *start < window_end)
            .collect::<Vec<_>>();
        if spans.len() != groups.len() {
            return Err(Error::Other(
                "text box paragraph boundaries do not match the typed story owner".to_owned(),
            ));
        }
        for ((start, end), group) in spans.into_iter().zip(groups) {
            let mut box_search = 0usize;
            for (source, replacement) in group {
                let Some(relative) =
                    find_bound_field_source(xml, start, end, &source, box_search, &[])?
                else {
                    return Err(Error::Other(
                        "text box field source was not found at its text box paragraph boundary"
                            .to_owned(),
                    ));
                };
                if source != replacement {
                    edits.push(FieldSourceEdit {
                        start: start + relative,
                        end: start + relative + source.len(),
                        replacement,
                    });
                }
                box_search = relative + source.len();
            }
        }
    }
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

pub(super) fn same_field_inventory(left: &CT_P, right: &CT_P) -> bool {
    fn fields(paragraph: &CT_P) -> Vec<(&Field, bool)> {
        let mut fields = Vec::new();
        for run in accepted_toc_runs(paragraph) {
            for content in &run.run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut fields);
                }
            }
        }
        fields
    }
    let left = fields(left);
    let right = fields(right);
    if left.len() != right.len() {
        return false;
    }
    // The text of an operand, a nested field's place marked; the nested
    // fields themselves are compared one by one above.
    fn argument(operand: &FieldArgument) -> Option<String> {
        match operand {
            FieldArgument::Text(text) => Some(text.clone()),
            FieldArgument::Nested(_) => None,
            FieldArgument::Joined(pieces) => Some(
                pieces
                    .iter()
                    .map(|piece| argument(piece).unwrap_or_else(|| "\u{0}".into()))
                    .collect(),
            ),
        }
    }
    for ((left, locked_left), (right, locked_right)) in left.iter().zip(&right) {
        if locked_left != locked_right || left.effective_display() != right.effective_display() {
            return false;
        }
        let a = left.effective_instruction();
        let b = right.effective_instruction();
        if a.name != b.name
            || a.arguments.len() != b.arguments.len()
            || a.switches.len() != b.switches.len()
        {
            return false;
        }
        if a.arguments
            .iter()
            .zip(&b.arguments)
            .any(|(a, b)| argument(a) != argument(b))
        {
            return false;
        }
        if a.switches.iter().zip(&b.switches).any(|(a, b)| {
            a.name != b.name
                || match (&a.argument, &b.argument) {
                    (None, None) => false,
                    (Some(a), Some(b)) => argument(a) != argument(b),
                    _ => true,
                }
        }) {
            return false;
        }
    }
    // Physical shared-owner equivalence is checked without treating fresh
    // source IDs as persistent identities across independent parsing.
    let groups = |fields: &[(&Field, bool)]| {
        let mut owners = HashMap::new();
        fields
            .iter()
            .enumerate()
            .map(|(index, (field, _))| {
                field
                    .source_owner_id()
                    .map_or(index, |owner| *owners.entry(owner).or_insert(index))
            })
            .collect::<Vec<_>>()
    };
    if groups(&left) != groups(&right) {
        return false;
    }

    true
}

pub(super) fn paragraph_field_source_replacements(
    original: &CT_P,
    staged: &CT_P,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    fn instruction_context(paragraph: &CT_P) -> Vec<(String, bool)> {
        fn field_context(field: &Field, locked: bool, output: &mut Vec<(String, bool)>) {
            let locked = locked || field.locked() == Some(true);
            output.push((field.effective_instruction_text(), locked));
            for child in field.nested_fields_in_source_order() {
                field_context(child, locked, output);
            }
        }
        let mut output = Vec::new();
        for run in accepted_toc_runs(paragraph) {
            for content in &run.run.content {
                if let RunContent::Field(field) = content {
                    field_context(field, false, &mut output);
                }
            }
        }
        output
    }
    if instruction_context(original) != instruction_context(staged) {
        return Err(Error::Other(
            "field cache publication changed physical instruction or lock ownership".into(),
        ));
    }
    let original = staged_paragraph_field_source_replacements(original)?;
    let staged = staged_paragraph_field_source_replacements(staged)?;
    if original.len() != staged.len() {
        return Err(Error::Other(
            "field cache publication changed physical source span cardinality".into(),
        ));
    }
    Ok(original
        .into_iter()
        .zip(staged)
        .map(|((source, _), (_, replacement))| (source, replacement))
        .collect())
}

pub(super) fn staged_paragraph_field_source_replacements(
    paragraph: &CT_P,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut replacements = Vec::new();
    let mut next_run = 0;
    for boundary in 0..=paragraph.runs.len() {
        for (_, owner) in accepted_paragraph_owners(paragraph, boundary) {
            match owner {
                AcceptedParagraphOwner::Control(control) => {
                    append_control_field_source_replacements(control, &mut replacements)?;
                }
                AcceptedParagraphOwner::Revision(revision) => {
                    if matches!(
                        revision.kind(),
                        RevisionKind::Insertion | RevisionKind::MoveTo
                    ) && let Some(content) = revision.content_paragraph()
                    {
                        replacements.extend(staged_paragraph_field_source_replacements(content)?);
                    }
                }
            }
        }
        if boundary < next_run || boundary == paragraph.runs.len() {
            continue;
        }
        next_run =
            boundary + paragraph.field_source_replacements_at(boundary, &mut replacements)?;
    }
    Ok(replacements)
}

fn append_control_field_source_replacements(
    control: &CT_Sdt,
    output: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    output.extend(
        control
            .inline_field_source_replacements()
            .into_iter()
            .map(|(source, replacement)| (source.to_vec(), replacement.to_vec())),
    );
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            if matches!(
                revision.kind(),
                RevisionKind::Insertion | RevisionKind::MoveTo
            ) && let Some(paragraph) = revision.content_paragraph()
            {
                output.extend(staged_paragraph_field_source_replacements(paragraph)?);
            }
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::ContentControl(control) => {
                append_control_field_source_replacements(control, output)?
            }
            SdtContent::Paragraph(paragraph) => {
                output.extend(staged_paragraph_field_source_replacements(paragraph)?);
            }
            SdtContent::Run(run) => {
                if let [RunContent::Field(field)] = run.content.as_slice()
                    && let Some((source, replacement)) = field.source_replacement()?
                {
                    output.push((source.to_vec(), replacement));
                }
            }
            SdtContent::Table(_)
            | SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn note_is_normal(element: &BytesStart<'_>, resolver: &NamespaceResolver) -> bool {
    let mut id = None;
    let mut note_type = None;
    for attribute in element.attributes() {
        let Ok(attribute) = attribute else {
            return false;
        };
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if !namespace_is_word(&namespace) {
            continue;
        }
        if local.as_ref() == b"id" {
            let Ok(value) = oxoffice_core::xml::attribute_text(&attribute) else {
                return false;
            };
            let Ok(value) = value.parse::<i32>() else {
                return false;
            };
            if id.replace(value).is_some() {
                return false;
            }
        } else if local.as_ref() == b"type" {
            let Ok(value) = oxoffice_core::xml::attribute_text(&attribute) else {
                return false;
            };
            if note_type.replace(value.into_owned()).is_some() {
                return false;
            }
        }
    }
    match note_type.as_deref() {
        Some("separator" | "continuationSeparator" | "continuationNotice") => false,
        Some("normal") => id.is_some(),
        Some(_) => false,
        None => id.is_some_and(|id| id > 0),
    }
}

pub(super) fn collect_legacy_form_anchors(
    output: &mut Vec<LegacyFormAnchor>,
    part: &str,
    xml: &[u8],
    kind: PackageStoryKind,
) -> Result<()> {
    let story = legacy_story_paragraphs(xml, kind)?;
    let mut ordinal = 0usize;
    for paragraph in &story {
        let sources = legacy_form_sources_in_paragraph(&paragraph.paragraph)?;
        place_legacy_sources(
            output,
            part,
            xml,
            paragraph.start,
            paragraph.end,
            &sources,
            &mut ordinal,
        )?;
    }
    for paragraph in &story {
        let boxes = textbox_paragraphs(&paragraph.paragraph);
        if boxes.is_empty() {
            continue;
        }
        let spans = word_txbx_paragraph_spans(xml, &[])?
            .into_iter()
            .filter(|(start, _)| *start >= paragraph.start && *start < paragraph.end)
            .collect::<Vec<_>>();
        if spans.len() != boxes.len() {
            return Err(Error::Other(
                "text box paragraph boundaries do not match the typed story owner".to_owned(),
            ));
        }
        for ((start, end), box_paragraph) in spans.into_iter().zip(boxes) {
            let sources = legacy_form_sources_in_paragraph(box_paragraph)?;
            place_legacy_sources(output, part, xml, start, end, &sources, &mut ordinal)?;
        }
    }
    Ok(())
}

fn legacy_form_sources_in_paragraph(paragraph: &CT_P) -> Result<Vec<Vec<u8>>> {
    let mut fields = Vec::new();
    let mut ordinal = 0usize;
    let mut sources = Vec::new();
    append_legacy_form_infos_in_paragraph(
        &mut fields,
        "",
        &mut ordinal,
        paragraph,
        true,
        &mut sources,
    )?;
    if sources.len() != fields.len() {
        return Err(Error::Other(
            "legacy form field has no parsed physical source".to_owned(),
        ));
    }
    Ok(sources)
}

fn place_legacy_sources(
    output: &mut Vec<LegacyFormAnchor>,
    part: &str,
    xml: &[u8],
    paragraph_start: usize,
    paragraph_end: usize,
    sources: &[Vec<u8>],
    ordinal: &mut usize,
) -> Result<()> {
    let mut search_start = 0usize;
    for source in sources {
        let Some(relative) =
            find_typed_field_source(xml, paragraph_start, paragraph_end, source, search_start)?
        else {
            return Err(Error::Other(
                "legacy form field source was not found in its accepted paragraph".to_owned(),
            ));
        };
        let start = paragraph_start + relative;
        output.push(LegacyFormAnchor {
            part: part.to_owned(),
            ordinal: *ordinal,
            start,
            end: start + source.len(),
        });
        *ordinal += 1;
        // The next accepted field may be nested inside this span. Advance one
        // byte so a later identical sibling is distinct and a nested field remains.
        search_start = relative.saturating_add(1);
    }
    Ok(())
}

fn textbox_paragraphs(paragraph: &CT_P) -> Vec<&CT_P> {
    let mut output = Vec::new();
    push_nested_textbox_paragraphs(paragraph, &mut output);
    output
}

fn push_nested_textbox_paragraphs<'a>(paragraph: &'a CT_P, output: &mut Vec<&'a CT_P>) {
    for path in paragraph.accepted_run_paths() {
        let Some(run) = paragraph.accepted_run(&path) else {
            continue;
        };
        for content in &run.content {
            if let RunContent::Drawing(drawing) = content {
                push_shape_paragraphs(drawing, output);
            }
        }
    }
}

fn push_shape_paragraphs<'a>(drawing: &'a CT_Drawing, output: &mut Vec<&'a CT_P>) {
    let Some(shape) = drawing
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.shape.as_ref())
    else {
        return;
    };
    let mut paragraphs = Vec::new();
    if let Some(body) = &shape.text_body {
        collect_body_paragraphs(body, &mut paragraphs, None);
    } else {
        paragraphs.extend(shape.text.iter());
    }
    for paragraph in paragraphs {
        output.push(paragraph);
        push_nested_textbox_paragraphs(paragraph, output);
    }
}

pub(crate) fn find_typed_field_source(
    xml: &[u8],
    paragraph_start: usize,
    paragraph_end: usize,
    source: &[u8],
    search_start: usize,
) -> Result<Option<usize>> {
    let paragraph_xml = &xml[paragraph_start..paragraph_end];
    let mut candidate_start = search_start;
    while let Some(relative) = find_bytes(&paragraph_xml[candidate_start..], source) {
        let candidate = candidate_start + relative;
        if field_source_has_typed_ancestors(xml, paragraph_start + candidate, paragraph_start)? {
            return Ok(Some(candidate));
        }
        candidate_start = candidate + source.len().max(1);
    }
    Ok(None)
}

fn field_source_has_typed_ancestors(
    xml: &[u8],
    source_start: usize,
    paragraph_start: usize,
) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut ancestors = Vec::<(usize, bool, Vec<u8>)>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package paragraph XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let physical_word =
                    xml[before..].starts_with(b"<w:") || xml[before..].starts_with(b"</w:");
                let word = if matches!(namespace, ResolveResult::Bound(_)) {
                    namespace_is_word(&namespace)
                } else {
                    physical_word
                };
                if before == source_start {
                    let local = local_name(&owned);
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
                ancestors.push((before, word, local_name(&owned).to_vec()));
            }
            Event::Empty(element) => {
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let physical_word =
                    xml[before..].starts_with(b"<w:") || xml[before..].starts_with(b"</w:");
                let word = if matches!(namespace, ResolveResult::Bound(_)) {
                    namespace_is_word(&namespace)
                } else {
                    physical_word
                };
                if before == source_start {
                    let local = local_name(&owned);
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
            }
            Event::End(_) => {
                ancestors.pop();
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn typed_field_ancestors(
    ancestors: &[(usize, bool, Vec<u8>)],
    paragraph_start: usize,
) -> bool {
    let Some(paragraph_index) = ancestors
        .iter()
        .position(|(start, _, _)| *start == paragraph_start)
    else {
        return false;
    };
    let (_, paragraph_word, paragraph) = &ancestors[paragraph_index];
    *paragraph_word
        && paragraph.as_slice() == b"p"
        && ancestors
            .iter()
            .skip(paragraph_index + 1)
            .all(|(_, word, local)| {
                *word
                    && matches!(
                        local.as_slice(),
                        b"hyperlink"
                            | b"sdt"
                            | b"sdtContent"
                            | b"ins"
                            | b"del"
                            | b"moveFrom"
                            | b"moveTo"
                    )
            })
}
