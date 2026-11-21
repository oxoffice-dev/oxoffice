//! Document properties: core, application and custom properties, and the application properties (slide count, titles) that a slide edit refreshes.

use super::*;

/// What a slide edit changed, for the package metadata that counts slides.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MetadataEdit {
    /// Slides were added: counts and title lists follow.
    Addition,
    /// A slide was removed: counts, title lists and the thumbnail follow.
    Removal,
    /// A slide was hidden or shown: the hidden-slide count follows.
    Visibility,
}
const EXTENDED_PROPERTIES_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";

impl Presentation {
    /// Returns shared package core properties without dirtying their source part.
    pub fn core_properties(&self) -> Option<&CoreProperties> {
        self.core_properties.as_ref()
    }

    /// Returns mutable shared core properties, materializing the package model.
    pub fn core_properties_mut(&mut self) -> &mut CoreProperties {
        self.core_properties_dirty = true;
        self.core_properties
            .get_or_insert_with(CoreProperties::default)
    }

    /// Changes known core fields while retaining every unowned imported byte.
    ///
    /// The core title (`dc:title`) is the user's: a slide edit never changes
    /// it, while a save after a slide edit refreshes `docProps/app.xml` (see
    /// [`Self::application_properties`]).
    pub fn set_core_properties(&mut self, mut properties: CoreProperties) -> Result<()> {
        // An empty value reads back as no value: it removes the property.
        for value in [
            &mut properties.title,
            &mut properties.creator,
            &mut properties.subject,
            &mut properties.description,
            &mut properties.keywords,
            &mut properties.last_modified_by,
            &mut properties.created,
            &mut properties.modified,
            &mut properties.category,
            &mut properties.content_status,
            &mut properties.identifier,
            &mut properties.language,
            &mut properties.last_printed,
            &mut properties.revision,
            &mut properties.version,
        ] {
            if value.as_deref() == Some("") {
                *value = None;
            }
        }
        // An unchanged field keeps the bytes the file wrote, so only a value
        // the file does not hold is refused.
        let held = self
            .package
            .package_rels
            .get_by_type(rel_types::CORE_PROPERTIES)
            .and_then(|_| self.package.get_part(&self.core_properties_part_name))
            .and_then(|source| CoreProperties::from_xml(source).ok());
        properties
            .reject_new_non_xml_characters(held.as_ref())
            .map_err(|error| invalid_shape_mutation("core properties", error.to_string()))?;
        properties
            .to_xml()
            .map_err(|error| invalid_shape_mutation("core properties", error.to_string()))?;
        if self.core_properties.as_ref() == Some(&properties) {
            return Ok(());
        }
        let mut candidate = self.clone();
        // The patch starts from what the part holds, not from a model the
        // mutable accessor may have changed since.
        let xml = if let (Some(old), Some(source)) = (
            held.as_ref().or(self.core_properties.as_ref()),
            self.package.get_part(&self.core_properties_part_name),
        ) {
            patch_core_properties(source, old, &properties)?
        } else {
            if self.package.contains_part(&self.core_properties_part_name) {
                return Err(Error::CorePropertiesPartCollision {
                    part_name: self.core_properties_part_name.clone(),
                });
            }
            properties
                .to_xml()
                .map_err(|error| invalid_shape_mutation("core properties", error.to_string()))?
        };
        candidate
            .package
            .set_part(&self.core_properties_part_name, xml);
        candidate.package.content_types.add_override(
            &self.core_properties_part_name,
            content_types::CORE_PROPERTIES,
        );
        if candidate
            .package
            .package_rels
            .get_by_type(rel_types::CORE_PROPERTIES)
            .is_none()
        {
            candidate.package.package_rels.add(
                rel_types::CORE_PROPERTIES,
                self.core_properties_part_name.trim_start_matches('/'),
            );
        }
        candidate.core_properties = Some(properties);
        candidate.core_properties_dirty = false;
        self.commit_candidate(candidate)
    }

    /// Removes the owned core bundle without changing any other source part.
    pub fn remove_core_properties(&mut self) -> Result<Option<CoreProperties>> {
        let Some(old) = self.core_properties.clone() else {
            return Ok(None);
        };
        let mut candidate = self.clone();
        candidate
            .package
            .remove_part(&self.core_properties_part_name);
        candidate
            .package
            .content_types
            .remove_override(&self.core_properties_part_name);
        candidate
            .package
            .package_rels
            .items
            .retain(|rel| rel.rel_type != rel_types::CORE_PROPERTIES);
        candidate.core_properties = None;
        candidate.core_properties_dirty = false;
        self.commit_candidate(candidate)?;
        Ok(Some(old))
    }

    /// Returns cached application properties without dirtying retained XML.
    ///
    /// A save after a slide edit refreshes the slide, notes and hidden-slide
    /// counts of `docProps/app.xml`. After any edit of a slide's content but
    /// a hide or a show, or a change of the slide list, it also drops what
    /// oxoffice does not recount: the title lists (`HeadingPairs`,
    /// `TitlesOfParts`) and the `Words` and `Paragraphs` counts, which
    /// PowerPoint writes again on its next save. A hide or a show alone
    /// keeps them.
    pub fn application_properties(&self) -> Option<&AppProperties> {
        self.application_properties.as_ref()
    }

    /// Publishes application properties after staged validation and source preservation.
    ///
    /// A field equal to what [`Self::application_properties`] gave is left
    /// as the part has it: after a slide edit not yet committed, the refresh
    /// of the counts wins over a stale copy, and a field set to a new value
    /// wins over the refresh.
    pub fn set_application_properties(&mut self, properties: AppProperties) -> Result<()> {
        if let Some(version) = &properties.application_version {
            let bytes = version.as_bytes();
            if bytes.len() != 7
                || bytes[2] != b'.'
                || !bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
            {
                return Err(invalid_shape_mutation(
                    "application properties",
                    "application_version must be XX.YYYY",
                ));
            }
        }
        let generated = properties
            .to_xml()
            .map_err(|e| invalid_shape_mutation("application properties", e.to_string()))?;
        // A slide edit still pending refreshes app.xml when it is committed:
        // commit it first, then apply only the fields the caller changed, so
        // that neither the refresh nor a stale copy undoes the other.
        let before = self.application_properties.clone();
        let pending = self.clone();
        self.commit_candidate(pending)?;
        let properties = match (&before, &self.application_properties) {
            (Some(before), Some(current)) if before != current => {
                changed_application_properties(before, &properties, current)
            }
            _ => properties,
        };
        if self.application_properties.as_ref() == Some(&properties) {
            return Ok(());
        }
        let mut candidate = self.clone();
        let xml = if let Some(name) = property_part(
            &self.package,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
        )? {
            patch_application_properties(
                required_part(&self.package, &name)?,
                self.application_properties.as_ref().ok_or_else(|| {
                    invalid_shape_mutation(
                        "application properties",
                        "missing parsed property owner",
                    )
                })?,
                &properties,
            )?
        } else {
            generated
        };
        publish_property_part(
            &mut candidate.package,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
            "/docProps/app.xml",
            xml,
        )?;
        candidate.application_properties = Some(properties);
        self.commit_candidate(candidate)
    }

    /// Removes the owned application part and relationship together.
    pub fn remove_application_properties(&mut self) -> Result<Option<AppProperties>> {
        let Some(old) = self.application_properties.clone() else {
            return Ok(None);
        };
        let mut candidate = self.clone();
        if let Some(name) = property_part(
            &candidate.package,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
        )? {
            candidate.package.remove_part(&name);
            candidate.package.content_types.remove_override(&name);
            candidate
                .package
                .package_rels
                .items
                .retain(|rel| rel.rel_type != rel_types::EXTENDED_PROPERTIES);
        }
        candidate.application_properties = None;
        self.commit_candidate(candidate)?;
        Ok(Some(old))
    }

    /// Returns custom properties in retained package order.
    pub fn custom_properties(&self) -> &[CustomProperty] {
        self.custom_properties
            .as_ref()
            .map(|p| p.properties.as_slice())
            .unwrap_or_default()
    }
    /// Finds a case-sensitive custom property name.
    pub fn custom_property(&self, name: &str) -> Option<&CustomProperty> {
        self.custom_properties()
            .iter()
            .find(|p| p.name.as_deref() == Some(name))
    }
    /// Changes one named value while retaining its existing identity and source order.
    pub fn set_custom_property(&mut self, mut property: CustomProperty) -> Result<()> {
        let name = property
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                invalid_shape_mutation("custom properties", "property name must not be empty")
            })?;
        oxoffice_core::xml::reject_non_xml_characters("custom property name", &name)
            .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
        if let Some(old) = self.custom_property(&name) {
            if old.value == property.value {
                return Ok(());
            }
            if matches!(old.value, CustomPropertyValue::Raw(_)) {
                return Err(invalid_shape_mutation(
                    "custom properties",
                    "unsupported retained custom values are read-only",
                ));
            }
            property.pid = old.pid;
            property.fmtid = old.fmtid.clone();
        } else if property.pid < 2
            || self
                .custom_properties()
                .iter()
                .any(|p| p.pid == property.pid)
        {
            return Err(invalid_shape_mutation(
                "custom properties",
                "custom property PID must be unique and at least 2",
            ));
        }
        oxoffice_core::xml::reject_non_xml_characters("custom property fmtid", &property.fmtid)
            .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
        let value = custom_value_xml(&property.value)?;
        let mut candidate = self.clone();
        let properties = candidate
            .custom_properties
            .get_or_insert_with(CustomProperties::default);
        if let Some(index) = properties
            .properties
            .iter()
            .position(|p| p.name.as_deref() == Some(&name))
        {
            properties.properties[index] = property.clone();
        } else {
            properties.properties.push(property.clone());
        }
        let part = property_part(
            &self.package,
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
        )?;
        let xml = if let Some(part) = part {
            patch_custom_property(
                required_part(&self.package, &part)?,
                &name,
                Some((&property, &value)),
            )?
        } else {
            properties
                .to_xml()
                .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?
        };
        publish_property_part(
            &mut candidate.package,
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
            "/docProps/custom.xml",
            xml,
        )?;
        self.commit_candidate(candidate)
    }
    /// Removes one property without changing other values or order.
    pub fn remove_custom_property(&mut self, name: &str) -> Result<Option<CustomProperty>> {
        let Some(old) = self.custom_property(name).cloned() else {
            return Ok(None);
        };
        let mut candidate = self.clone();
        let part = property_part(
            &self.package,
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
        )?
        .ok_or_else(|| invalid_shape_mutation("custom properties", "property bundle is absent"))?;
        let xml = patch_custom_property(required_part(&self.package, &part)?, name, None)?;
        candidate.package.set_part(&part, xml);
        candidate
            .custom_properties
            .as_mut()
            .expect("existing property")
            .properties
            .retain(|p| p.name.as_deref() != Some(name));
        self.commit_candidate(candidate)?;
        Ok(Some(old))
    }
}

impl Presentation {
    /// Keeps the package metadata in step with a slide edit.
    ///
    /// `docProps/app.xml` gets the deck's counts where it has them: after a
    /// slide is added or removed, the slide, notes and hidden-slide counts,
    /// and what the edit makes stale goes (see
    /// [`Self::refreshed_application_properties`]). After a hide or a show,
    /// only the hidden-slide count. A save refreshes them again whenever a
    /// slide, its notes or the slide list changed, so text edits, notes and
    /// moves are covered too. An app.xml the source-preserving patch cannot
    /// edit (one outside its namespace, or with a count in a foreign
    /// namespace) is left as it was rather than refusing the edit.
    ///
    /// After a removal the package thumbnail goes too: it pictures the slide
    /// that was first when the deck was last saved, which moves and inserts
    /// may have taken anywhere.
    pub(super) fn refresh_document_metadata(&mut self, edit: MetadataEdit) -> Result<()> {
        let thumbnail = edit == MetadataEdit::Removal;
        let structure = edit != MetadataEdit::Visibility;
        let thumbnails = self
            .package
            .package_rels
            .items
            .iter()
            .filter(|relationship| {
                relationship.rel_type == rel_types::THUMBNAIL
                    && !relationship_is_external(relationship)
            })
            .filter(|_| thumbnail)
            .map(|relationship| OpcPackage::resolve_rel_target("/", &relationship.target))
            .collect::<Vec<_>>();
        for part in &thumbnails {
            self.package.remove_part(part);
            self.package.content_types.remove_override(part);
        }
        self.package.package_rels.items.retain(|relationship| {
            !thumbnail
                || relationship.rel_type != rel_types::THUMBNAIL
                || relationship_is_external(relationship)
        });
        let Some((part, xml, properties)) =
            self.refreshed_application_properties(structure, structure)?
        else {
            return Ok(());
        };
        self.package.set_part(&part, xml);
        self.application_properties = Some(properties);
        Ok(())
    }

    /// `docProps/app.xml` with the deck's counts, or `None` when there is
    /// none, or the source-preserving patch cannot edit it. `counts` refreshes
    /// the slide and notes counts (the hidden-slide count always follows);
    /// `stale` drops what an edit of slide text, order or structure makes
    /// stale and oxoffice does not recount: the title lists
    /// (`HeadingPairs`, `TitlesOfParts`, whose group names are localised) and
    /// the `Words` and `Paragraphs` counts. PowerPoint writes them again on
    /// its next save. The core title (`dc:title`) is the user's and is kept.
    pub(super) fn refreshed_application_properties(
        &self,
        counts: bool,
        stale: bool,
    ) -> Result<Option<(String, Vec<u8>, AppProperties)>> {
        let (Some(old), Some(part)) = (
            self.application_properties.clone(),
            property_part(
                &self.package,
                rel_types::EXTENDED_PROPERTIES,
                content_types::EXTENDED_PROPERTIES,
            )?,
        ) else {
            return Ok(None);
        };
        let count = |keep: fn(&SlideRecord) -> bool| {
            i32::try_from(self.slides.iter().filter(|record| keep(record)).count())
                .unwrap_or(i32::MAX)
        };
        let mut new = old.clone();
        if counts {
            new.slides = old.slides.map(|_| count(|_| true));
            // `Notes` counts slides containing notes, not empty notes pages.
            new.notes = old.notes.map(|_| {
                count(|record| {
                    record
                        .notes
                        .as_ref()
                        .is_some_and(|notes| !notes.notes.notes_text().trim().is_empty())
                })
            });
        }
        new.hidden_slides = old
            .hidden_slides
            .map(|_| count(|record| record.slide.hidden()));
        let source = required_part(&self.package, &part)?;
        let Ok((patched, (children, _))) = patch_application_properties(source, &old, &new)
            .and_then(|patched| {
                let ranges = property_child_ranges(&patched, EXTENDED_PROPERTIES_NS, "Properties")?;
                Ok((patched, ranges))
            })
        else {
            return Ok(None);
        };
        let mut xml = Vec::with_capacity(patched.len());
        let mut cursor = 0;
        for (_, _, start, end) in children.iter().filter(|(uri, local, _, _)| {
            stale
                && uri.as_deref() == Some(EXTENDED_PROPERTIES_NS)
                && matches!(
                    local.as_str(),
                    "HeadingPairs" | "TitlesOfParts" | "Words" | "Paragraphs"
                )
        }) {
            xml.extend_from_slice(&patched[cursor..*start]);
            cursor = *end;
        }
        xml.extend_from_slice(&patched[cursor..]);
        if xml.as_slice() == source {
            return Ok(None);
        }
        let properties = AppProperties::from_xml(&xml)
            .map_err(|error| footer_malformed(&part, error.to_string()))?;
        Ok(Some((part, xml, properties)))
    }
}

fn expanded_property_root(source: &[u8], namespace: &str, root: &str) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    loop {
        let begin = reader.buffer_position() as usize;
        let (qualified, event) = {
            let (ns, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|e| invalid_shape_mutation("properties", e.to_string()))?;
            (xml_is(&ns, namespace), event)
        };
        let end = reader.buffer_position() as usize;
        match event {
            XmlEvent::Empty(start)
                if qualified && xml_local_name(start.name().as_ref()) == root.as_bytes() =>
            {
                let mut result = source[..end - 2].to_vec();
                result.push(b'>');
                result.extend_from_slice(b"</");
                result.extend_from_slice(start.name().as_ref());
                result.push(b'>');
                result.extend_from_slice(&source[end..]);
                return Ok(result);
            }
            XmlEvent::Start(_) | XmlEvent::Empty(_) | XmlEvent::Eof => return Ok(source.to_vec()),
            XmlEvent::Decl(_) | XmlEvent::Comment(_) | XmlEvent::PI(_) => {}
            XmlEvent::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            _ => {
                return Err(invalid_shape_mutation(
                    "properties",
                    format!("unsupported property root at {begin}"),
                ));
            }
        };
        buffer.clear();
    }
}

type PropertyChildRanges = Vec<(Option<String>, String, usize, usize)>;

pub(super) fn property_child_ranges(
    source: &[u8],
    namespace: &str,
    root: &str,
) -> Result<(PropertyChildRanges, usize)> {
    let mut reader = NsReader::from_reader(source);
    let mut buffer = Vec::new();
    let mut depth = 0;
    let mut pending = None;
    let mut children = Vec::new();
    let mut root_end = None;
    loop {
        let begin = reader.buffer_position() as usize;
        let (uri, event) = {
            let (resolved, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|e| invalid_shape_mutation("properties", e.to_string()))?;
            let uri = match resolved {
                quick_xml::name::ResolveResult::Bound(ns) => {
                    Some(String::from_utf8_lossy(ns.as_ref()).into_owned())
                }
                _ => None,
            };
            (uri, event)
        };
        let end = reader.buffer_position() as usize;
        match event {
            XmlEvent::Start(start) => {
                if depth == 0
                    && (uri.as_deref() != Some(namespace)
                        || xml_local_name(start.name().as_ref()) != root.as_bytes())
                {
                    return Err(invalid_shape_mutation(
                        "properties",
                        "unqualified core owner",
                    ));
                }
                if depth == 1 {
                    pending = Some((
                        uri,
                        String::from_utf8_lossy(xml_local_name(start.name().as_ref())).into_owned(),
                        begin,
                    ));
                }
                depth += 1;
            }
            XmlEvent::Empty(start) if depth == 1 => children.push((
                uri,
                String::from_utf8_lossy(xml_local_name(start.name().as_ref())).into_owned(),
                begin,
                end,
            )),
            XmlEvent::End(_) => {
                depth -= 1;
                if depth == 1
                    && let Some((uri, name, start)) = pending.take()
                {
                    children.push((uri, name, start, end));
                }
                if depth == 0 {
                    root_end = Some(begin);
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let insertion = root_end
        .ok_or_else(|| invalid_shape_mutation("properties", "missing core closing owner"))?;
    Ok((children, insertion))
}

fn patch_application_properties(
    source: &[u8],
    old: &AppProperties,
    new: &AppProperties,
) -> Result<Vec<u8>> {
    const NS: &str = EXTENDED_PROPERTIES_NS;
    let fields = [
        ("Template", old.template.clone(), new.template.clone()),
        ("Manager", old.manager.clone(), new.manager.clone()),
        ("Company", old.company.clone(), new.company.clone()),
        (
            "Pages",
            old.pages.map(|value| value.to_string()),
            new.pages.map(|value| value.to_string()),
        ),
        (
            "Words",
            old.words.map(|value| value.to_string()),
            new.words.map(|value| value.to_string()),
        ),
        (
            "Characters",
            old.characters.map(|value| value.to_string()),
            new.characters.map(|value| value.to_string()),
        ),
        (
            "PresentationFormat",
            old.presentation_format.clone(),
            new.presentation_format.clone(),
        ),
        (
            "Lines",
            old.lines.map(|value| value.to_string()),
            new.lines.map(|value| value.to_string()),
        ),
        (
            "Paragraphs",
            old.paragraphs.map(|value| value.to_string()),
            new.paragraphs.map(|value| value.to_string()),
        ),
        (
            "Slides",
            old.slides.map(|value| value.to_string()),
            new.slides.map(|value| value.to_string()),
        ),
        (
            "Notes",
            old.notes.map(|value| value.to_string()),
            new.notes.map(|value| value.to_string()),
        ),
        (
            "TotalTime",
            old.total_time.map(|value| value.to_string()),
            new.total_time.map(|value| value.to_string()),
        ),
        (
            "HiddenSlides",
            old.hidden_slides.map(|value| value.to_string()),
            new.hidden_slides.map(|value| value.to_string()),
        ),
        (
            "MMClips",
            old.multimedia_clips.map(|value| value.to_string()),
            new.multimedia_clips.map(|value| value.to_string()),
        ),
        (
            "ScaleCrop",
            old.scale_crop.map(|value| value.to_string()),
            new.scale_crop.map(|value| value.to_string()),
        ),
        (
            "LinksUpToDate",
            old.links_up_to_date.map(|value| value.to_string()),
            new.links_up_to_date.map(|value| value.to_string()),
        ),
        (
            "CharactersWithSpaces",
            old.characters_with_spaces.map(|value| value.to_string()),
            new.characters_with_spaces.map(|value| value.to_string()),
        ),
        (
            "SharedDoc",
            old.shared_document.map(|value| value.to_string()),
            new.shared_document.map(|value| value.to_string()),
        ),
        (
            "HyperlinksChanged",
            old.hyperlinks_changed.map(|value| value.to_string()),
            new.hyperlinks_changed.map(|value| value.to_string()),
        ),
        (
            "Application",
            old.application.clone(),
            new.application.clone(),
        ),
        (
            "AppVersion",
            old.application_version.clone(),
            new.application_version.clone(),
        ),
    ];
    let expanded = expanded_property_root(source, NS, "Properties")?;
    let source = expanded.as_slice();
    let (children, insertion) = property_child_ranges(source, NS, "Properties")?;
    let mut edits = Vec::new();
    let mut append = Vec::new();
    for (name, before, after) in fields {
        if before == after {
            continue;
        }
        if let Some(value) = &after {
            oxoffice_core::xml::reject_non_xml_characters("application property", value)
                .map_err(|e| invalid_shape_mutation("application properties", e.to_string()))?;
        }
        let matches = children
            .iter()
            .filter(|(uri, local, _, _)| uri.as_deref() == Some(NS) && local == name)
            .collect::<Vec<_>>();
        if matches.len() > 1
            || children
                .iter()
                .any(|(uri, local, _, _)| local == name && uri.as_deref() != Some(NS))
        {
            return Err(invalid_shape_mutation(
                "application properties",
                "ambiguous or foreign property spelling",
            ));
        }
        if let Some((_, _, start, end)) = matches.first() {
            // A removed value removes its element: an empty count or flag is
            // not a valid value.
            let retained = match &after {
                Some(value) => edit_property_text(&source[*start..*end], Some(value))?,
                None => Vec::new(),
            };
            edits.push((*start, *end, retained));
        } else if let Some(value) = after {
            append.extend(
                format!(
                    "<ap:{name} xmlns:ap=\"{NS}\">{}</ap:{name}>",
                    oxoffice_core::xml::escape_text(&value)
                )
                .into_bytes(),
            );
        }
    }
    if !append.is_empty() {
        edits.push((insertion, insertion, append));
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut result = Vec::new();
    let mut cursor = 0;
    for (start, end, bytes) in edits {
        result.extend_from_slice(&source[cursor..start]);
        result.extend(bytes);
        cursor = end;
    }
    result.extend_from_slice(&source[cursor..]);
    let parsed = AppProperties::from_xml(&result)
        .map_err(|e| invalid_shape_mutation("application properties", e.to_string()))?;
    if &parsed != new {
        return Err(invalid_shape_mutation(
            "application properties",
            "retained source prevents exact typed update",
        ));
    }
    Ok(result)
}

/// `current` with the fields `wanted` changes from `before`: a field the
/// caller left as it read it follows the part as it is now.
fn changed_application_properties(
    before: &AppProperties,
    wanted: &AppProperties,
    current: &AppProperties,
) -> AppProperties {
    let mut merged = current.clone();
    macro_rules! changed {
        ($($field:ident),* $(,)?) => {
            $(if wanted.$field != before.$field {
                merged.$field = wanted.$field.clone();
            })*
        };
    }
    changed!(
        template,
        manager,
        company,
        pages,
        words,
        characters,
        presentation_format,
        lines,
        paragraphs,
        slides,
        notes,
        total_time,
        hidden_slides,
        multimedia_clips,
        scale_crop,
        links_up_to_date,
        characters_with_spaces,
        shared_document,
        hyperlinks_changed,
        application,
        application_version,
    );
    merged
}

fn custom_value_xml(value: &CustomPropertyValue) -> Result<String> {
    let (kind,text)=match value {
        CustomPropertyValue::Lpstr(s)=>("lpstr",s.clone()),CustomPropertyValue::Lpwstr(s)=>("lpwstr",s.clone()),
        CustomPropertyValue::I4(n)=>("i4",n.to_string()),CustomPropertyValue::R8(n) if n.is_finite()=>("r8",n.to_string()),
        CustomPropertyValue::Bool(b)=>("bool",b.to_string()),CustomPropertyValue::FileTime(s)=>("filetime",s.clone()),
        CustomPropertyValue::Empty=>return Ok(r#"<vt:empty xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"/>"#.to_owned()),
        _=>return Err(invalid_shape_mutation("custom properties","new value must be a supported finite custom type")),
    };
    oxoffice_core::xml::reject_non_xml_characters("custom property value", &text)
        .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
    Ok(format!(
        "<vt:{kind} xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">{}</vt:{kind}>",
        oxoffice_core::xml::escape_text(&text)
    ))
}

fn patch_custom_property(
    source: &[u8],
    name: &str,
    replacement: Option<(&CustomProperty, &str)>,
) -> Result<Vec<u8>> {
    const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
    let expanded = expanded_property_root(source, NS, "Properties")?;
    let source = expanded.as_slice();
    let (children, insertion) = property_child_ranges(source, NS, "Properties")?;
    let mut selected = None;
    for (uri, local, start, end) in children {
        if uri.as_deref() != Some(NS) || local != "property" {
            return Err(invalid_shape_mutation(
                "custom properties",
                "unqualified custom owner",
            ));
        }
        let mut reader = quick_xml::Reader::from_reader(&source[start..end]);
        if let XmlEvent::Start(root) | XmlEvent::Empty(root) = reader
            .read_event()
            .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?
        {
            let property_name = root
                .attributes()
                .find_map(|a| a.ok().filter(|a| a.key.as_ref() == b"name"))
                .map(|a| {
                    a.decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        reader.decoder(),
                    )
                    .map(|v| v.into_owned())
                })
                .transpose()
                .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
            if property_name.as_deref() == Some(name) && selected.replace((start, end)).is_some() {
                return Err(invalid_shape_mutation(
                    "custom properties",
                    "duplicate property name",
                ));
            }
        }
    }
    let Some((start, end)) = selected else {
        if let Some((property, value)) = replacement {
            let xml = format!(
                "<property xmlns=\"{NS}\" fmtid=\"{}\" pid=\"{}\" name=\"{}\">{value}</property>",
                oxoffice_core::xml::escape_attribute_value(&property.fmtid),
                property.pid,
                oxoffice_core::xml::escape_attribute_value(name)
            );
            let mut result = source[..insertion].to_vec();
            result.extend(xml.as_bytes());
            result.extend_from_slice(&source[insertion..]);
            return Ok(result);
        }
        return Ok(source.to_vec());
    };
    let bytes = if let Some((_, value)) = replacement {
        let owner = &source[start..end];
        let mut reader = quick_xml::Reader::from_reader(owner);
        let mut depth = 0;
        let mut value_start = None;
        let mut value_range = None;
        loop {
            let begin = reader.buffer_position() as usize;
            let event = reader
                .read_event()
                .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
            let finish = reader.buffer_position() as usize;
            match event {
                XmlEvent::Start(_) => {
                    if depth == 1 {
                        value_start = Some(begin);
                    }
                    depth += 1;
                }
                XmlEvent::Empty(_) if depth == 1 => {
                    if value_range.replace((begin, finish)).is_some() {
                        return Err(invalid_shape_mutation(
                            "custom properties",
                            "ambiguous custom value",
                        ));
                    }
                }
                XmlEvent::End(_) => {
                    depth -= 1;
                    if depth == 1
                        && let Some(begin) = value_start.take()
                        && value_range.replace((begin, finish)).is_some()
                    {
                        return Err(invalid_shape_mutation(
                            "custom properties",
                            "ambiguous custom value",
                        ));
                    }
                }
                XmlEvent::Eof => break,
                _ => {}
            }
        }
        let (begin, finish) = value_range
            .ok_or_else(|| invalid_shape_mutation("custom properties", "custom value is absent"))?;
        let old_value = &owner[begin..finish];
        let mut old_reader = quick_xml::Reader::from_reader(old_value);
        let old_start = old_reader
            .read_event()
            .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
        let mut new_reader = quick_xml::Reader::from_reader(value.as_bytes());
        let new_start = new_reader
            .read_event()
            .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
        let value_bytes = match (old_start, new_start) {
            (XmlEvent::Start(old), XmlEvent::Start(new))
                if xml_local_name(old.name().as_ref()) == xml_local_name(new.name().as_ref()) =>
            {
                let raw = new_reader
                    .read_text(new.name())
                    .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
                let decoded = raw
                    .decode()
                    .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
                let text = quick_xml::escape::unescape(&decoded)
                    .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
                edit_property_text(old_value, Some(&text))?
            }
            (XmlEvent::Start(old) | XmlEvent::Empty(old), _) => {
                for attribute in old.attributes() {
                    let attribute = attribute
                        .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
                    if attribute.key.as_ref() != b"xmlns"
                        && !attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        return Err(invalid_shape_mutation(
                            "custom properties",
                            "changing the value type would discard an opaque attribute",
                        ));
                    }
                }
                edit_property_text(old_value, None)?;
                value.as_bytes().to_vec()
            }
            _ => {
                return Err(invalid_shape_mutation(
                    "custom properties",
                    "unsupported custom value owner",
                ));
            }
        };
        let mut result = owner[..begin].to_vec();
        result.extend(value_bytes);
        result.extend_from_slice(&owner[finish..]);
        result
    } else {
        Vec::new()
    };
    let mut result = source[..start].to_vec();
    result.extend(bytes);
    result.extend_from_slice(&source[end..]);
    CustomProperties::from_xml(&result)
        .map_err(|e| invalid_shape_mutation("custom properties", e.to_string()))?;
    Ok(result)
}

pub(super) fn property_part(
    package: &OpcPackage,
    rel_type: &str,
    content_type: &str,
) -> Result<Option<String>> {
    let matches = package.package_rels.get_all_by_type(rel_type);
    if matches.len() > 1 {
        return Err(invalid_shape_mutation(
            "properties",
            "ambiguous package property relationships",
        ));
    }
    let Some(rel) = matches.first() else {
        return Ok(None);
    };
    reject_external("/", rel)?;
    let name = OpcPackage::resolve_rel_target("/", &rel.target);
    required_part(package, &name)?;
    if package.content_types.content_type_for(&name) != Some(content_type) {
        return Err(invalid_shape_mutation(
            "properties",
            "property content type does not match its relationship",
        ));
    }
    Ok(Some(name))
}

fn publish_property_part(
    package: &mut OpcPackage,
    rel_type: &str,
    content_type: &str,
    default: &str,
    bytes: Vec<u8>,
) -> Result<()> {
    let name =
        property_part(package, rel_type, content_type)?.unwrap_or_else(|| default.to_owned());
    if package.package_rels.get_by_type(rel_type).is_none() {
        if package.contains_part(&name) {
            return Err(invalid_shape_mutation(
                "properties",
                "unowned property part collision",
            ));
        }
        package
            .package_rels
            .add(rel_type, name.trim_start_matches('/'));
    }
    package.content_types.add_override(&name, content_type);
    package.set_part(&name, bytes);
    Ok(())
}

fn edit_property_text(source: &[u8], value: Option<&str>) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(source);
    let mut buffer = Vec::new();
    let mut result = Vec::new();
    let mut opened = false;
    let mut wrote = false;
    loop {
        let begin = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| invalid_shape_mutation("properties", error.to_string()))?;
        let end = reader.buffer_position() as usize;
        match event {
            XmlEvent::Start(_) if !opened => {
                opened = true;
                result.extend_from_slice(&source[begin..end]);
            }
            XmlEvent::Empty(start) if !opened => {
                result.extend_from_slice(&source[begin..end - 2]);
                result.push(b'>');
                result.extend_from_slice(
                    oxoffice_core::xml::escape_text(value.unwrap_or("")).as_bytes(),
                );
                result.extend_from_slice(b"</");
                result.extend_from_slice(start.name().as_ref());
                result.push(b'>');
            }
            XmlEvent::Text(_) | XmlEvent::CData(_) | XmlEvent::GeneralRef(_) => {
                if !wrote {
                    result.extend_from_slice(
                        oxoffice_core::xml::escape_text(value.unwrap_or("")).as_bytes(),
                    );
                    wrote = true;
                }
            }
            XmlEvent::End(_) => {
                if !wrote {
                    result.extend_from_slice(
                        oxoffice_core::xml::escape_text(value.unwrap_or("")).as_bytes(),
                    );
                    wrote = true;
                }
                result.extend_from_slice(&source[begin..end]);
            }
            XmlEvent::Comment(_) | XmlEvent::PI(_) => result.extend_from_slice(&source[begin..end]),
            XmlEvent::Eof => break,
            _ => {
                return Err(invalid_shape_mutation(
                    "properties",
                    "opaque nested property value cannot be changed safely",
                ));
            }
        }
        buffer.clear();
    }
    Ok(result)
}

pub(crate) fn patch_core_properties(
    source: &[u8],
    old: &CoreProperties,
    new: &CoreProperties,
) -> Result<Vec<u8>> {
    const CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
    const DC: &str = "http://purl.org/dc/elements/1.1/";
    const DT: &str = "http://purl.org/dc/terms/";
    let fields = [
        (DC, "title", &old.title, &new.title),
        (DC, "subject", &old.subject, &new.subject),
        (DC, "creator", &old.creator, &new.creator),
        (DC, "description", &old.description, &new.description),
        (DC, "identifier", &old.identifier, &new.identifier),
        (DC, "language", &old.language, &new.language),
        (CP, "keywords", &old.keywords, &new.keywords),
        (
            CP,
            "lastModifiedBy",
            &old.last_modified_by,
            &new.last_modified_by,
        ),
        (CP, "revision", &old.revision, &new.revision),
        (CP, "lastPrinted", &old.last_printed, &new.last_printed),
        (CP, "category", &old.category, &new.category),
        (
            CP,
            "contentStatus",
            &old.content_status,
            &new.content_status,
        ),
        (CP, "version", &old.version, &new.version),
        (DT, "created", &old.created, &new.created),
        (DT, "modified", &old.modified, &new.modified),
    ];
    let expanded = expanded_property_root(source, CP, "coreProperties")?;
    let source = expanded.as_slice();
    let (children, insertion) = property_child_ranges(source, CP, "coreProperties")?;
    let mut edits = Vec::new();
    let mut append = Vec::new();
    for (uri, name, before, after) in fields {
        if before == after {
            continue;
        }
        let matches: Vec<_> = children
            .iter()
            .filter(|(ns, local, _, _)| ns.as_deref() == Some(uri) && local == name)
            .collect();
        if matches.len() > 1 {
            return Err(invalid_shape_mutation(
                "core properties",
                format!("duplicate owned {name}"),
            ));
        }
        let replacement = after.as_ref().map(|value| {
            let prefix = if uri == DC { "dc" } else if uri == DT { "dcterms" } else { "cp" };
            let typed = if uri == DT { r#" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="dcterms:W3CDTF""# } else { "" };
            format!("<{prefix}:{name} xmlns:{prefix}=\"{uri}\"{typed}>{}</{prefix}:{name}>", oxoffice_core::xml::escape_text(value)).into_bytes()
        }).unwrap_or_default();
        if let Some((_, _, start, end)) = matches.first() {
            // A removed value removes its element: an empty W3CDTF date or
            // revision is not a valid value.
            let retained = match after {
                Some(value) => edit_property_text(&source[*start..*end], Some(value))?,
                None => Vec::new(),
            };
            edits.push((*start, *end, retained));
        } else {
            append.extend(replacement);
        }
    }
    if !append.is_empty() {
        edits.push((insertion, insertion, append));
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut result = Vec::new();
    let mut cursor = 0;
    for (start, end, bytes) in edits {
        result.extend_from_slice(&source[cursor..start]);
        result.extend(bytes);
        cursor = end;
    }
    result.extend_from_slice(&source[cursor..]);
    let parsed = CoreProperties::from_xml(&result)
        .map_err(|e| invalid_shape_mutation("core properties", e.to_string()))?;
    if &parsed != new {
        return Err(invalid_shape_mutation(
            "core properties",
            "retained foreign owner prevents exact typed update",
        ));
    }
    Ok(result)
}
