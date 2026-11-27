//! Admitting an edited paragraph back into its story: context, islands and meaning.

use super::*;

// This is the finite edited CT_P boundary, not a recursive Word schema.
// ECMA-376 fifth edition wml.xsd CT_P/EG_PContent expands to these 35 names.
fn edited_paragraph_schema_child(namespace: &str, local: &str) -> bool {
    (namespace == M_NS && matches!(local, "oMath" | "oMathPara"))
        || (namespace == W_NS
            && matches!(
                local,
                "pPr"
                    | "customXml"
                    | "smartTag"
                    | "sdt"
                    | "dir"
                    | "bdo"
                    | "r"
                    | "fldSimple"
                    | "hyperlink"
                    | "subDoc"
                    | "proofErr"
                    | "permStart"
                    | "permEnd"
                    | "bookmarkStart"
                    | "bookmarkEnd"
                    | "moveFromRangeStart"
                    | "moveFromRangeEnd"
                    | "moveToRangeStart"
                    | "moveToRangeEnd"
                    | "commentRangeStart"
                    | "commentRangeEnd"
                    | "customXmlInsRangeStart"
                    | "customXmlInsRangeEnd"
                    | "customXmlDelRangeStart"
                    | "customXmlDelRangeEnd"
                    | "customXmlMoveFromRangeStart"
                    | "customXmlMoveFromRangeEnd"
                    | "customXmlMoveToRangeStart"
                    | "customXmlMoveToRangeEnd"
                    | "ins"
                    | "del"
                    | "moveFrom"
                    | "moveTo"
            ))
}

#[derive(Default)]
struct EditedParagraphScopeChanges {
    namespaces: Vec<(String, Option<String>)>,
    ignorable: Vec<String>,
}

impl EditedParagraphContext {
    fn expanded_name(&self, name: &[u8], attribute: bool) -> Result<(&str, String)> {
        let name = std::str::from_utf8(name).map_err(|error| Error::Other(error.to_string()))?;
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        let uri = if prefix == "xml" {
            "http://www.w3.org/XML/1998/namespace"
        } else if prefix.is_empty() && attribute {
            ""
        } else {
            match self.namespaces.get(prefix) {
                Some(uri) => uri,
                None if prefix.is_empty() => "",
                None => {
                    return Err(Error::Other(format!(
                        "replace_paragraph_xml has an unbound prefix in {name}"
                    )));
                }
            }
        };
        Ok((uri, local.to_owned()))
    }

    fn enter(&mut self, element: &BytesStart<'_>) -> Result<EditedParagraphScopeChanges> {
        let mut changes = EditedParagraphScopeChanges::default();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let name = attribute.key.as_ref();
            let prefix = if name == b"xmlns" {
                ""
            } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
                std::str::from_utf8(prefix).map_err(|error| Error::Other(error.to_string()))?
            } else {
                continue;
            };
            changes
                .namespaces
                .push((prefix.to_owned(), self.namespaces.remove(prefix)));
        }
        update_story_namespace_scope(&mut self.namespaces, element)?;
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            let (uri, local) = self.expanded_name(attribute.key.as_ref(), true)?;
            if uri == MC_NS && local == "Ignorable" {
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|error| Error::Other(error.to_string()))?;
                for prefix in value.split_ascii_whitespace() {
                    let uri = self
                        .namespaces
                        .get(prefix)
                        .filter(|uri| uri.as_str() != MC_NS)
                        .ok_or_else(|| {
                            Error::Other(format!(
                                "replace_paragraph_xml has invalid mc:Ignorable prefix {prefix}"
                            ))
                        })?;
                    if !self.ignorable.contains(uri) {
                        self.ignorable.insert(uri.clone());
                        changes.ignorable.push(uri.clone());
                    }
                }
            }
        }
        Ok(changes)
    }

    fn leave(&mut self, changes: EditedParagraphScopeChanges) {
        for uri in changes.ignorable {
            self.ignorable.remove(&uri);
        }
        for (prefix, previous) in changes.namespaces.into_iter().rev() {
            self.namespaces.remove(&prefix);
            if let Some(uri) = previous {
                self.namespaces.insert(prefix, uri);
            }
        }
    }
}

// Original and replacement scans share these IDs. A route keeps one parent ID,
// expanded name and sibling ordinal, never a copy of every ancestor route.
#[derive(Default)]
struct EditedParagraphRoutes {
    namespaces: HashMap<String, usize>,
    routes: HashMap<(usize, usize, String, usize), usize>,
}

impl EditedParagraphRoutes {
    fn namespace(&mut self, uri: &str) -> usize {
        let next = self.namespaces.len();
        *self.namespaces.entry(uri.to_owned()).or_insert(next)
    }

    fn child(&mut self, parent: usize, uri: &str, local: &str, ordinal: usize) -> usize {
        let namespace = self.namespace(uri);
        let next = self.routes.len() + 1;
        *self
            .routes
            .entry((parent, namespace, local.to_owned(), ordinal))
            .or_insert(next)
    }
}

pub(super) fn edited_paragraph_ancestor_context(
    xml: &[u8],
    offset: usize,
) -> Result<EditedParagraphContext> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut ancestors = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) if before == offset => {
                // Do not inherit the original paragraph's own declarations into
                // a replacement that removed or rebound them.
                let _ = element;
                let mut context = EditedParagraphContext::default();
                for ancestor in &ancestors {
                    context.enter(ancestor)?;
                }
                return Ok(context);
            }
            Event::Start(element) => {
                ancestors.push(element.into_owned());
            }
            Event::Empty(_) => {}
            Event::End(_) => {
                ancestors.pop();
            }
            Event::Eof => {
                return Err(Error::Other(
                    "replace_paragraph_xml source ancestry was not found".into(),
                ));
            }
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct EditedParagraphIsland {
    // Expanded ancestor names and ordered element occurrences belong to this
    // selected physical owner. Equal raw bytes elsewhere are no authority.
    route: usize,
    name: (usize, String),
    raw: Vec<u8>,
    meaning: Vec<(usize, String, bool)>,
    authored_ignorable: bool,
}

fn edited_paragraph_complex_mc(
    element: &BytesStart<'_>,
    context: &EditedParagraphContext,
) -> Result<bool> {
    let (uri, _) = context.expanded_name(element.name().as_ref(), false)?;
    if uri == MC_NS {
        return Ok(true);
    }
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (uri, local) = context.expanded_name(attribute.key.as_ref(), true)?;
        if uri == MC_NS && local != "Ignorable" {
            return Ok(true);
        }
    }
    Ok(false)
}

struct EditedParagraphMeaning {
    names: Vec<(usize, String, bool)>,
    complex_mc: bool,
}

fn edited_paragraph_island_meaning(
    xml: &[u8],
    context: &mut EditedParagraphContext,
    routes: &mut EditedParagraphRoutes,
) -> Result<EditedParagraphMeaning> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut scopes = Vec::new();
    let mut meaning = Vec::new();
    let mut complex = false;
    loop {
        let event = reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let changes = context.enter(element)?;
                let (uri, local) = context.expanded_name(element.name().as_ref(), false)?;
                let mc_choice = uri == MC_NS && local == "Choice";
                meaning.push((
                    routes.namespace(uri),
                    local,
                    context.ignorable.contains(uri),
                ));
                complex |= edited_paragraph_complex_mc(element, context)?;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let (uri, local) = context.expanded_name(attribute.key.as_ref(), true)?;
                    meaning.push((
                        routes.namespace(uri),
                        format!("@{local}"),
                        context.ignorable.contains(uri),
                    ));
                    // Opaque QName-valued producer attributes retain any
                    // in-scope prefix meanings they actually mention.
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    for token in value.split_ascii_whitespace() {
                        let prefix = token.split_once(':').map(|(prefix, _)| prefix);
                        if let Some(prefix) = prefix
                            && let Some(uri) = context.namespaces.get(prefix)
                        {
                            meaning.push((routes.namespace(uri), format!("value:{prefix}"), false));
                        }
                        if (uri == MC_NS || (mc_choice && uri.is_empty() && local == "Requires"))
                            && let Some(uri) = context.namespaces.get(token)
                        {
                            meaning.push((routes.namespace(uri), format!("mc:{token}"), false));
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    scopes.push(changes);
                } else {
                    context.leave(changes);
                }
            }
            Event::End(_) => {
                context.leave(scopes.pop().expect("open island element"));
            }
            Event::Eof => {
                return Ok(EditedParagraphMeaning {
                    names: meaning,
                    complex_mc: complex,
                });
            }
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct EditedParagraphMcAttribute {
    route: usize,
    name: String,
    raw_value: Vec<u8>,
    meaning: Vec<(String, Option<usize>, bool)>,
}

#[derive(PartialEq)]
enum EditedParagraphSection {
    Parsed(Box<CT_SectPr>),
    Preserved(EditedParagraphIsland),
}

struct EditedParagraphAdmission {
    islands: Vec<EditedParagraphIsland>,
    mc_attributes: Vec<EditedParagraphMcAttribute>,
    sections: Vec<EditedParagraphSection>,
}

fn edited_paragraph_admission(
    xml: &[u8],
    inherited: &EditedParagraphContext,
    routes: &mut EditedParagraphRoutes,
) -> Result<EditedParagraphAdmission> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut context = inherited.clone();
    let mut frames = vec![(EditedParagraphScopeChanges::default(), 0usize, 0usize)];
    let mut islands = Vec::new();
    let mut mc_attributes = Vec::new();
    let mut ppr = false;
    let mut ppr_route = None;
    let mut sections = Vec::new();
    let mut content = false;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let (_, parent_route, next) = frames.last_mut().expect("initial frame");
                let parent_route = *parent_route;
                let changes = context.enter(element)?;
                let (uri, local) = context.expanded_name(element.name().as_ref(), false)?;
                let route = routes.child(parent_route, uri, &local, *next);
                *next += 1;
                let direct = frames.len() == 2;
                let schema = edited_paragraph_schema_child(uri, &local);
                if direct && schema {
                    if uri == W_NS && local == "pPr" {
                        if ppr || content {
                            return Err(Error::Other(format!(
                                "replace_paragraph_xml requires a single first {{{W_NS}}}pPr child"
                            )));
                        }
                        ppr = true;
                        ppr_route = Some(route);
                    } else {
                        content = true;
                    }
                }
                if Some(parent_route) == ppr_route && uri == W_NS && local == "sectPr" {
                    let end = story_element_end(xml, start)?;
                    let raw = &xml[start..end];
                    // Empty sectPr is preserved raw by CT_PPr. Expand a temporary
                    // copy for the same semantic model used by paired elements.
                    let mut section = raw.to_vec();
                    if matches!(event, Event::Empty(_)) {
                        section
                            .truncate(oxoffice_core::xml_element::start_tag(raw)?.attributes_end);
                        section.extend_from_slice(b"></");
                        section.extend_from_slice(element.name().as_ref());
                        section.push(b'>');
                    }
                    let section = close_content_fragment_namespaces(&section, &context.namespaces)?;
                    let mut paragraph = format!("<w:p xmlns:w=\"{W_NS}\"><w:pPr>").into_bytes();
                    paragraph.extend_from_slice(&section);
                    paragraph.extend_from_slice(b"</w:pPr></w:p>");
                    match CT_P::from_xml_fragment_checked(&paragraph) {
                        Ok(parsed) => {
                            let section = parsed
                                .properties
                                .and_then(|properties| properties.sect_pr)
                                .ok_or_else(|| {
                                    Error::Other(
                                        "paragraph section properties could not be checked".into(),
                                    )
                                })?;
                            sections.push(EditedParagraphSection::Parsed(Box::new(section)));
                        }
                        Err(_) if matches!(event, Event::Empty(_)) => {
                            // The ordinary checked paragraph parser already admitted
                            // this raw Empty section. Preserve its exact bytes and
                            // namespace meaning when paired parsing cannot model it.
                            let mut opaque_context = context.clone();
                            let meaning =
                                edited_paragraph_island_meaning(raw, &mut opaque_context, routes)?;
                            sections.push(EditedParagraphSection::Preserved(
                                EditedParagraphIsland {
                                    route: 0,
                                    name: (routes.namespace(W_NS), "sectPr".into()),
                                    raw: raw.to_vec(),
                                    meaning: meaning.names,
                                    authored_ignorable: false,
                                },
                            ));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                if (!direct
                    && uri == W_NS
                    && matches!(local.as_str(), "drawing" | "pict" | "object"))
                    || (schema && uri == M_NS)
                {
                    // Existing native providers own supported foreign payloads,
                    // math and their established compatibility routes.
                    if matches!(event, Event::Start(_)) {
                        reader
                            .read_to_end(element.name())
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                    }
                    context.leave(changes);
                    continue;
                }
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let (attribute_uri, attribute_local) =
                        context.expanded_name(attribute.key.as_ref(), true)?;
                    if attribute_uri != MC_NS || attribute_local == "Ignorable" {
                        continue;
                    }
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    let meaning = value
                        .split_ascii_whitespace()
                        .map(|token| {
                            let prefix = token.split_once(':').map_or(token, |(prefix, _)| prefix);
                            let uri = if prefix == "xml" {
                                Some("http://www.w3.org/XML/1998/namespace")
                            } else {
                                context.namespaces.get(prefix).map(String::as_str)
                            };
                            let ignored = uri.is_some_and(|uri| context.ignorable.contains(uri));
                            (
                                token.to_owned(),
                                uri.map(|uri| routes.namespace(uri)),
                                ignored,
                            )
                        })
                        .collect();
                    mc_attributes.push(EditedParagraphMcAttribute {
                        route,
                        name: attribute_local,
                        raw_value: attribute.value.as_ref().to_vec(),
                        meaning,
                    });
                }
                if (direct && !schema) || uri == MC_NS {
                    let end = story_element_end(xml, start)?;
                    let raw = &xml[start..end];
                    let ignored_foreign = !uri.is_empty()
                        && uri != W_NS
                        && uri != M_NS
                        && uri != MC_NS
                        && context.ignorable.contains(uri);
                    let name = (routes.namespace(uri), local);
                    let meaning = edited_paragraph_island_meaning(raw, &mut context, routes)?;
                    let authored_ignorable = ignored_foreign && !meaning.complex_mc;
                    islands.push(EditedParagraphIsland {
                        route,
                        name,
                        raw: raw.to_vec(),
                        meaning: meaning.names,
                        authored_ignorable,
                    });
                    if matches!(event, Event::Start(_)) {
                        reader
                            .read_to_end(element.name())
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                    }
                } else if matches!(event, Event::Start(_)) {
                    frames.push((changes, route, 0));
                    continue;
                }
                context.leave(changes);
            }
            Event::End(_) => {
                let (changes, _, _) = frames.pop().expect("open paragraph element");
                context.leave(changes);
            }
            Event::Eof => {
                return Ok(EditedParagraphAdmission {
                    islands,
                    mc_attributes,
                    sections,
                });
            }
            _ => {}
        }
    }
}

pub(super) fn admit_edited_paragraph(
    original: &[u8],
    incoming: &[u8],
    inherited: &EditedParagraphContext,
) -> Result<()> {
    let mut routes = EditedParagraphRoutes::default();
    let original = edited_paragraph_admission(original, inherited, &mut routes)?;
    let incoming = edited_paragraph_admission(incoming, inherited, &mut routes)?;
    if original.sections != incoming.sections {
        return Err(Error::Other(
            "element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into(),
        ));
    }
    let mut original_counts = HashMap::new();
    let mut incoming_counts = HashMap::new();
    for island in &original.islands {
        *original_counts.entry(island).or_insert(0usize) += 1;
    }
    for island in &incoming.islands {
        *incoming_counts.entry(island).or_insert(0usize) += 1;
    }
    for island in original
        .islands
        .iter()
        .chain(incoming.islands.iter())
        .filter(|island| !island.authored_ignorable)
    {
        if original_counts.get(island) != Some(&1) || incoming_counts.get(island) != Some(&1) {
            let (namespace, local) = &island.name;
            let uri = routes
                .namespaces
                .iter()
                .find_map(|(uri, id)| (id == namespace).then_some(uri))
                .expect("interned island namespace");
            return Err(Error::Other(format!(
                "replace_paragraph_xml refuses new or reinterpreted opaque child {{{uri}}}{local}"
            )));
        }
    }
    let mut original_attributes = HashMap::new();
    let mut incoming_attributes = HashMap::new();
    for attribute in &original.mc_attributes {
        *original_attributes.entry(attribute).or_insert(0usize) += 1;
    }
    for attribute in &incoming.mc_attributes {
        *incoming_attributes.entry(attribute).or_insert(0usize) += 1;
    }
    for attribute in original
        .mc_attributes
        .iter()
        .chain(incoming.mc_attributes.iter())
    {
        if original_attributes.get(attribute) != Some(&1)
            || incoming_attributes.get(attribute) != Some(&1)
        {
            return Err(Error::Other(format!(
                "replace_paragraph_xml refuses changed unsupported {{{MC_NS}}}{} declaration",
                attribute.name
            )));
        }
    }
    Ok(())
}
