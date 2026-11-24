//! The mail merge package: recipients, data source graph, settings markup and the merge table codec.

use super::*;

pub(super) const MERGE_DATA_NS: &str = "urn:rdocx:mail-merge-data:v1";
pub(super) const MERGE_STORE_DOC_VAR: &str = "rdocx.mailMerge.storeItemId";
const MERGE_SOURCE_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/mailMergeSource";
const MERGE_RECIPIENT_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/recipientData";
const MERGE_CUSTOM_XML_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const MERGE_RECIPIENT_STANDARD: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.mailMergeRecipientData+xml";
const MERGE_RECIPIENT_ALTERNATE: &str = "application/vnd.ms-word.mailMergeRecipientData+xml";
const MERGE_PROPS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const MERGE_PROPS_NS: &[u8] = b"http://schemas.openxmlformats.org/officeDocument/2006/customXml";
const MERGE_DATA_TYPES: &[&str] = &[
    "textFile",
    "database",
    "spreadsheet",
    "query",
    "odbc",
    "native",
];
const MERGE_FIELD_ROLES: [&str; 30] = [
    "Unique",
    "CourtesyTitle",
    "FirstName",
    "MiddleName",
    "LastName",
    "Suffix",
    "Nickname",
    "JobTitle",
    "Company",
    "Address1",
    "Address2",
    "City",
    "State",
    "PostalCode",
    "CountryorRegion",
    "BusinessPhone",
    "BusinessFax",
    "HomePhone",
    "HomeFax",
    "EmailAddress",
    "WebPage",
    "SpouseCourtesyTitle",
    "SpouseFirstName",
    "SpouseMiddleName",
    "SpouseLastName",
    "SpouseNickname",
    "RubyFirstName",
    "RubyLastName",
    "Address3",
    "Department",
];

pub(super) fn validate_merge_mappings(options: &MailMergePackageOptions) -> Result<()> {
    let mut seen = HashSet::new();
    for mapping in &options.mappings {
        if !MERGE_FIELD_ROLES.contains(&mapping.mapped_name.as_str()) {
            return Err(Error::Other(format!(
                "mail-merge field role {} is not canonical",
                mapping.mapped_name
            )));
        }
        if !seen.insert(mapping.mapped_name.clone()) {
            return Err(Error::Other(format!(
                "mail-merge field role {} is duplicated",
                mapping.mapped_name
            )));
        }
        if let MailMergePackageSource::Embedded { columns, .. } = &options.source {
            let Some(name) = columns.get(mapping.column as usize) else {
                return Err(Error::Other(
                    "mail-merge field column is outside the embedded table".to_owned(),
                ));
            };
            if name != &mapping.source_name {
                return Err(Error::Other(
                    "mail-merge field column does not match the embedded name".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

#[doc(hidden)]
pub fn validate_mail_merge_external_uri(uri: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("mail-merge external URI", uri)?;
    if !merge_uri_is_literal_ascii(uri) || !merge_uri_scheme_is_file_or_https(uri) {
        return Err(Error::Other(
            "mail-merge external URI is not a file or https declaration".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn merge_uri_scheme_is_file_or_https(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {
            let Some(rest) = rest.strip_prefix("//") else {
                return false;
            };
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = authority
                .rsplit_once('@')
                .map(|(_, host)| host)
                .unwrap_or(authority);
            let host = host.split_once(':').map(|(host, _)| host).unwrap_or(host);
            !authority.is_empty() && !host.is_empty()
        }
        "file" => {
            let path_text = rest.split(['?', '#']).next().unwrap_or("");
            let path = if let Some(rest) = path_text.strip_prefix("//") {
                match rest.find('/') {
                    Some(index) => &rest[index..],
                    None => return false,
                }
            } else {
                path_text
            };
            path.starts_with('/') && !path.is_empty()
        }
        _ => false,
    }
}

fn merge_uri_is_literal_ascii(uri: &str) -> bool {
    let bytes = uri.as_bytes();
    if bytes.is_empty()
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

pub(super) fn identity_mappings(mappings: &[MailMergeFieldMapping]) -> bool {
    !mappings.is_empty()
        && mappings
            .iter()
            .all(|mapping| mapping.source_name == mapping.mapped_name)
}

pub(super) fn embedded_mappings_are_faithful(
    mappings: &[MailMergeFieldMapping],
    columns: &[String],
) -> bool {
    mappings.iter().all(|mapping| {
        mapping.source_name == mapping.mapped_name
            && columns.get(mapping.column as usize).map(String::as_str)
                == Some(mapping.source_name.as_str())
    })
}

pub(super) fn merge_settings_need_evaluation(settings: &crate::MailMerge) -> bool {
    settings.connect_string.is_some()
        || settings.query.is_some()
        || settings.data_type.as_deref() == Some("native")
}

pub(super) fn merge_diagnostic(
    kind: MailMergePackageDiagnosticKind,
    message: &str,
) -> MailMergePreparedData {
    MailMergePreparedData {
        records: Vec::new(),
        diagnostics: vec![MailMergePackageDiagnostic {
            kind,
            message: message.to_owned(),
        }],
    }
}

pub(super) enum RecipientClass {
    Absent,
    Standard {
        part: String,
        bytes: Vec<u8>,
        shared: bool,
    },
    Alternate,
}

fn text_has_payload(text: &[u8]) -> bool {
    text.iter().any(|byte| !byte.is_ascii_whitespace())
}

/// Whether an entity stands for more than white space (`&#10;` does not).
fn entity_has_payload(entity: &quick_xml::events::BytesRef<'_>) -> bool {
    text_has_payload(oxoffice_core::xml_text::resolve_entity(entity).as_bytes())
}

fn content_type_base(content_type: &str) -> &str {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
}

pub(super) fn recipient_part_census(package: &OpcPackage) -> Result<Vec<(String, bool)>> {
    let mut found = Vec::new();
    for name in package.parts.keys() {
        let Some(content_type) = package.content_types.content_type_for(name) else {
            continue;
        };
        let standard = content_type_base(content_type) == MERGE_RECIPIENT_STANDARD;
        let alternate = content_type_base(content_type) == MERGE_RECIPIENT_ALTERNATE;
        if standard || alternate {
            found.push((name.clone(), standard));
        }
    }
    if found.len() > 1 {
        return Err(Error::Other(
            "mail-merge recipient part is duplicated".to_owned(),
        ));
    }
    Ok(found)
}

pub(super) fn classify_recipients(
    document: &Document,
    recipient_id: Option<&str>,
) -> Result<RecipientClass> {
    let Some(recipient_id) = recipient_id else {
        return Ok(RecipientClass::Absent);
    };
    let settings_part = resolved_settings_part(document)?
        .ok_or_else(|| Error::Other("mail-merge recipient reference is missing".to_owned()))?;
    let relationships = document
        .package
        .get_part_rels(&settings_part)
        .ok_or_else(|| Error::Other("mail-merge recipient reference is missing".to_owned()))?;
    let matches: Vec<_> = relationships
        .items
        .iter()
        .filter(|item| item.id == recipient_id)
        .collect();
    let [rel] = matches.as_slice() else {
        return Err(Error::Other(if matches.is_empty() {
            "mail-merge recipient reference is missing".to_owned()
        } else {
            "mail-merge recipient reference is duplicated".to_owned()
        }));
    };
    if rel.rel_type != MERGE_RECIPIENT_REL {
        return Err(Error::Other(
            "mail-merge recipient reference is not recipientData".to_owned(),
        ));
    }
    if !crate::document::relationship_is_internal(rel) {
        return Err(Error::Other(
            if rel.target_mode.as_deref() == Some("External") {
                "mail-merge recipient reference is external".to_owned()
            } else {
                "mail-merge recipient reference mode is invalid".to_owned()
            },
        ));
    }
    let resolved = OpcPackage::resolve_rel_target(&settings_part, &rel.target);
    let parts = recipient_part_census(&document.package)?;
    let Some((part, standard)) = parts
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&resolved))
    else {
        return Err(Error::Other(
            "mail-merge recipient part is missing".to_owned(),
        ));
    };
    if parts.len() != 1 {
        return Err(Error::Other(
            "mail-merge recipient part is duplicated".to_owned(),
        ));
    }
    if document
        .package
        .get_part_rels(part)
        .is_some_and(|relationships| !relationships.items.is_empty())
    {
        return Err(Error::Other(
            "mail-merge recipient part has outgoing relationships".to_owned(),
        ));
    }
    let bytes = document
        .package
        .get_part(part)
        .ok_or_else(|| Error::Other("mail-merge recipient part is missing".to_owned()))?
        .to_vec();
    if *standard {
        decode_recipients(&bytes)?;
        Ok(RecipientClass::Standard {
            part: part.clone(),
            bytes,
            shared: internal_part_targets(&document.package, part) != 1,
        })
    } else {
        prove_alternate_recipient(&bytes)?;
        Ok(RecipientClass::Alternate)
    }
}

const MERGE_ALTERNATE_NS: &[u8] = b"http://schemas.microsoft.com/office/word/2006/wordml";

fn prove_alternate_recipient(xml: &[u8]) -> Result<()> {
    validate_strict_xml_1_0(xml)
        .map_err(|_| Error::Other("mail-merge alternate recipient is malformed".to_owned()))?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut saw_root = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|_| Error::Other("mail-merge alternate recipient is malformed".to_owned()))?
            .into_owned();
        match event {
            Event::Start(element) | Event::Empty(element) if !saw_root => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let qualified = local.as_ref() == b"recipients"
                    && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == MERGE_ALTERNATE_NS);
                if !qualified {
                    return Err(Error::Other(
                        "mail-merge alternate recipient root is not proved".to_owned(),
                    ));
                }
                saw_root = true;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root {
        return Err(Error::Other(
            "mail-merge alternate recipient root is not proved".to_owned(),
        ));
    }
    Ok(())
}

fn encode_recipients(recipients: &[MailMergeRecipient]) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let mut output = format!("<w:recipients xmlns:w=\"{W_NS}\">").into_bytes();
    for recipient in recipients {
        let tag = base64::engine::general_purpose::STANDARD.encode(&recipient.unique_tag);
        oxoffice_core::xml::reject_non_xml_characters("mail-merge recipient tag", &tag)?;
        output.extend_from_slice(b"<w:recipientData>");
        for name in oxoffice_docx_oxml::settings::RECIPIENT_DATA_ORDER {
            match *name {
                b"active" => push_valued_element(
                    &mut output,
                    "active",
                    if recipient.active { "true" } else { "false" },
                )?,
                b"column" => {
                    push_valued_element(&mut output, "column", &recipient.column.to_string())?
                }
                b"uniqueTag" => push_valued_element(&mut output, "uniqueTag", &tag)?,
                _ => {
                    return Err(Error::Other(
                        "mail-merge recipient order is incomplete".to_owned(),
                    ));
                }
            }
        }
        output.extend_from_slice(b"</w:recipientData>");
    }
    output.extend_from_slice(b"</w:recipients>");
    Ok(output)
}

pub(super) fn decode_recipients(xml: &[u8]) -> Result<Vec<MailMergeRecipient>> {
    use base64::Engine as _;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut recipients = Vec::new();
    let mut active = None;
    let mut column = None;
    let mut tag = None;
    let mut saw_root = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid mail-merge recipients: {error}")))?
            .into_owned();
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
                let name = local.as_ref().to_vec();
                if !word {
                    return Err(Error::Other(
                        "mail-merge recipients are malformed".to_owned(),
                    ));
                }
                let parent = stack.last().map(Vec::as_slice);
                match (parent, name.as_slice()) {
                    (None, b"recipients") => {
                        if saw_root {
                            return Err(Error::Other(
                                "mail-merge recipients are malformed".to_owned(),
                            ));
                        }
                        saw_root = true;
                        reject_recipient_attributes(reader.resolver(), element, false)?;
                    }
                    (Some(b"recipients"), b"recipientData") => {
                        if active.is_some() || column.is_some() || tag.is_some() {
                            return Err(Error::Other(
                                "mail-merge recipients are malformed".to_owned(),
                            ));
                        }
                        reject_recipient_attributes(reader.resolver(), element, false)?;
                    }
                    (Some(b"recipientData"), b"active") if column.is_none() && tag.is_none() => {
                        reject_recipient_attributes(reader.resolver(), element, true)?;
                        if active.is_some() {
                            return Err(Error::Other(
                                "mail-merge recipient active is duplicated".to_owned(),
                            ));
                        }
                        active = Some(parse_recipient_active(&word_val(
                            reader.resolver(),
                            element,
                            b"val",
                        )?)?);
                    }
                    (Some(b"recipientData"), b"column") if tag.is_none() => {
                        reject_recipient_attributes(reader.resolver(), element, true)?;
                        if column.is_some() {
                            return Err(Error::Other(
                                "mail-merge recipient column is duplicated".to_owned(),
                            ));
                        }
                        column = Some(parse_recipient_column(&word_val(
                            reader.resolver(),
                            element,
                            b"val",
                        )?)?);
                    }
                    (Some(b"recipientData"), b"uniqueTag") if column.is_some() => {
                        reject_recipient_attributes(reader.resolver(), element, true)?;
                        if tag.is_some() {
                            return Err(Error::Other(
                                "mail-merge recipient tag is duplicated".to_owned(),
                            ));
                        }
                        let value =
                            word_val(reader.resolver(), element, b"val")?.ok_or_else(|| {
                                Error::Other("mail-merge recipient tag is missing".to_owned())
                            })?;
                        let compact: String = value
                            .chars()
                            .filter(|character| !character.is_ascii_whitespace())
                            .collect();
                        tag = Some(
                            base64::engine::general_purpose::STANDARD
                                .decode(compact.as_bytes())
                                .map_err(|_| {
                                    Error::Other("mail-merge recipient tag is malformed".to_owned())
                                })?,
                        );
                    }
                    _ => {
                        return Err(Error::Other(
                            "mail-merge recipients are malformed".to_owned(),
                        ));
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(name);
                } else if name == b"recipientData" {
                    recipients.push(finish_recipient(active.take(), column.take(), tag.take())?);
                }
            }
            Event::End(_) => {
                let Some(name) = stack.pop() else {
                    return Err(Error::Other(
                        "mail-merge recipients are malformed".to_owned(),
                    ));
                };
                if name == b"recipientData" {
                    recipients.push(finish_recipient(active.take(), column.take(), tag.take())?);
                }
            }
            Event::Text(text) => {
                if text_has_payload(text.as_ref()) {
                    return Err(Error::Other(
                        "mail-merge recipients are malformed".to_owned(),
                    ));
                }
            }
            Event::CData(text) => {
                if text_has_payload(text.as_ref()) {
                    return Err(Error::Other(
                        "mail-merge recipients are malformed".to_owned(),
                    ));
                }
            }
            Event::GeneralRef(entity) => {
                if entity_has_payload(entity) {
                    return Err(Error::Other(
                        "mail-merge recipients are malformed".to_owned(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root || !stack.is_empty() || recipients.is_empty() {
        return Err(Error::Other(
            "mail-merge recipients are malformed".to_owned(),
        ));
    }
    Ok(recipients)
}

fn finish_recipient(
    active: Option<bool>,
    column: Option<u32>,
    tag: Option<Vec<u8>>,
) -> Result<MailMergeRecipient> {
    Ok(MailMergeRecipient {
        column: column
            .ok_or_else(|| Error::Other("mail-merge recipient column is missing".to_owned()))?,
        unique_tag: tag
            .ok_or_else(|| Error::Other("mail-merge recipient tag is missing".to_owned()))?,
        active: active.unwrap_or(true),
    })
}

fn parse_recipient_active(value: &Option<String>) -> Result<bool> {
    match value.as_deref() {
        None => Ok(true),
        Some(value) => oxoffice_docx_oxml::shared::ST_OnOff::parse(value)
            .ok_or_else(|| Error::Other("mail-merge recipient active is malformed".to_owned())),
    }
}

fn parse_recipient_column(value: &Option<String>) -> Result<u32> {
    value
        .as_deref()
        .ok_or_else(|| Error::Other("mail-merge recipient column is missing".to_owned()))?
        .parse::<u32>()
        .map_err(|_| Error::Other("mail-merge recipient column is malformed".to_owned()))
}

fn reject_recipient_attributes(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    allow_value: bool,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge recipient attribute: {error}"))
        })?;
        let raw = attribute.key.as_ref();
        if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
            continue;
        }
        if allow_value {
            let (namespace, local) = resolver.resolve_attribute(attribute.key);
            if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                && local.as_ref() == b"val"
            {
                continue;
            }
        }
        return Err(Error::Other(
            "mail-merge recipients are malformed".to_owned(),
        ));
    }
    Ok(())
}

fn current_recipient_id(document: &Document) -> Result<Option<String>> {
    let Some(settings) = &document.settings else {
        return Ok(None);
    };
    Ok(inspect_merge_xml(&settings.to_xml()?)?.graph.recipient_id)
}

pub(super) fn install_recipient_part(
    candidate: &mut Document,
    recipients: &[MailMergeRecipient],
) -> Result<Option<String>> {
    let current_id = current_recipient_id(candidate)?;
    match classify_recipients(candidate, current_id.as_deref())? {
        RecipientClass::Alternate => Err(Error::Other(
            "mail-merge recipient data is unsupported".to_owned(),
        )),
        RecipientClass::Absent => {
            if recipients.is_empty() {
                return Ok(None);
            }
            if !recipient_part_census(&candidate.package)?.is_empty() {
                return Err(Error::Other(
                    "mail-merge recipient part is already present".to_owned(),
                ));
            }
            let settings = resolved_settings_part(candidate)?
                .ok_or_else(|| Error::Other("mail-merge settings part is missing".to_owned()))?;
            candidate
                .identifiers
                .observe_package_graph(&candidate.package)?;
            let part = candidate
                .identifiers
                .reserve_preferred_part_name("/word/recipientData.xml")?;
            candidate.add_override_journaled(&part, MERGE_RECIPIENT_STANDARD);
            candidate.identifiers.register_content_type_override(&part);
            candidate
                .package
                .set_part(&part, encode_recipients(recipients)?);
            let id = candidate.add_relative_internal_relationship_checked(
                &settings,
                MERGE_RECIPIENT_REL,
                &part,
            )?;
            Ok(Some(id))
        }
        RecipientClass::Standard {
            part,
            bytes,
            shared,
        } => {
            if recipients.is_empty() {
                if let Some(id) = current_id.clone() {
                    drop_relationship_id(candidate, &part, &id)?;
                }
                if !shared && internal_part_targets(&candidate.package, &part) == 0 {
                    retire_owned_part(candidate, &part);
                }
                return Ok(None);
            }
            let next = encode_recipients(recipients)?;
            if next != bytes {
                if shared {
                    return Err(Error::Other(
                        "mail-merge recipient part is shared".to_owned(),
                    ));
                }
                candidate.package.set_part(&part, next);
            }
            Ok(current_id)
        }
    }
}

pub(super) fn release_owned_recipient(candidate: &mut Document) -> Result<()> {
    let current_id = current_recipient_id(candidate)?;
    match classify_recipients(candidate, current_id.as_deref())? {
        RecipientClass::Alternate => Err(Error::Other(
            "mail-merge recipient data is unsupported".to_owned(),
        )),
        RecipientClass::Absent => Ok(()),
        RecipientClass::Standard { part, shared, .. } => {
            if let Some(id) = current_id {
                drop_relationship_id(candidate, &part, &id)?;
            }
            if !shared && internal_part_targets(&candidate.package, &part) == 0 {
                retire_owned_part(candidate, &part);
            }
            Ok(())
        }
    }
}

fn drop_relationship_id(document: &mut Document, part: &str, relationship_id: &str) -> Result<()> {
    let Some(settings) = resolved_settings_part(document)? else {
        return Ok(());
    };
    let mut removed = Vec::new();
    document.package.record_part_rels(&settings);
    if let Some(relationships) = document.package.get_part_rels_mut(&settings) {
        relationships.items.retain(|item| {
            let matched = item.id == relationship_id
                && item.rel_type == MERGE_RECIPIENT_REL
                && crate::document::relationship_is_internal(item)
                && OpcPackage::resolve_rel_target(&settings, &item.target)
                    .eq_ignore_ascii_case(part);
            if matched {
                removed.push(item.id.clone());
            }
            !matched
        });
    }
    document
        .identifiers
        .retire_authored_story_relationships(&settings, removed);
    Ok(())
}

pub(super) fn release_owned_merge_store(candidate: &mut Document) -> Result<()> {
    let Some(item_id) = candidate.merge_store_doc_var()? else {
        return Ok(());
    };
    let part = match crate::content_control::resolve_custom_xml_part(
        &candidate.package,
        &candidate.doc_part_name,
        &item_id,
    ) {
        Ok(part) => part,
        Err(Error::Other(message)) if message.contains("was not found") => {
            candidate.remove_document_variable(MERGE_STORE_DOC_VAR)?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    require_merge_store_schema(&candidate.package, &part)?;
    let current = candidate
        .package
        .get_part(&part)
        .ok_or_else(|| Error::Other("mail-merge package source is missing".to_owned()))?;
    decode_merge_table(current)?;
    if !merge_store_has_other_owner(candidate, &part, &item_id)? {
        let properties = owned_store_properties(&candidate.package, &part)?;
        drop_edges_of_type(candidate, &part, MERGE_CUSTOM_XML_REL);
        retire_owned_part(candidate, &part);
        if let Some(properties) = properties
            && internal_part_targets(&candidate.package, &properties) == 0
        {
            retire_owned_part(candidate, &properties);
        }
    }
    candidate.remove_document_variable(MERGE_STORE_DOC_VAR)?;
    Ok(())
}

fn owned_store_properties(package: &OpcPackage, item_part: &str) -> Result<Option<String>> {
    let Some(relationships) = package.get_part_rels(item_part) else {
        return Ok(None);
    };
    let properties: Vec<_> = relationships
        .items
        .iter()
        .filter(|item| item.rel_type == MERGE_PROPS_REL)
        .collect();
    if properties.len() != 1 || !crate::document::relationship_is_internal(properties[0]) {
        return Err(Error::Other(
            "mail-merge store properties relationship is not unique".to_owned(),
        ));
    }
    Ok(Some(OpcPackage::resolve_rel_target(
        item_part,
        &properties[0].target,
    )))
}

fn drop_edges_of_type(document: &mut Document, part_name: &str, rel_type: &str) {
    let owners: Vec<String> = std::iter::once(document.doc_part_name.clone())
        .chain(document.package.part_rels.keys().cloned())
        .collect();
    let mut seen = HashSet::new();
    for owner in owners {
        if !seen.insert(owner.clone()) {
            continue;
        }
        let mut removed = Vec::new();
        let matches = |item: &oxoffice_opc::Relationship| {
            item.rel_type == rel_type
                && item.target_mode.as_deref() != Some("External")
                && OpcPackage::resolve_rel_target(&owner, &item.target)
                    .eq_ignore_ascii_case(part_name)
        };
        if document
            .package
            .get_part_rels(&owner)
            .is_some_and(|relationships| relationships.items.iter().any(matches))
        {
            // Only an owner that loses an edge is journaled, at the cost of
            // its relationships.
            document.package.record_part_rels(&owner);
        }
        if let Some(relationships) = document.package.get_part_rels_mut(&owner) {
            relationships.items.retain(|item| {
                let matched = item.rel_type == rel_type
                    && item.target_mode.as_deref() != Some("External")
                    && OpcPackage::resolve_rel_target(&owner, &item.target)
                        .eq_ignore_ascii_case(part_name);
                if matched {
                    removed.push(item.id.clone());
                }
                !matched
            });
        }
        if !removed.is_empty() {
            document
                .identifiers
                .retire_authored_story_relationships(&owner, removed);
        }
    }
}

/// Add the custom XML store of a merge table to `document` in place, inside
/// the open edit of the mail-merge call.
pub(super) fn add_merge_store(document: &mut Document, xml: &[u8]) -> Result<String> {
    crate::content_control::validate_store_item(xml)?;
    crate::bibliography::add_custom_xml_store_in_place(document, xml, &[MERGE_DATA_NS])
}

fn retire_owned_part(document: &mut Document, part_name: &str) {
    document.package.remove_part(part_name);
    document.package.remove_part_rels(part_name);
    document.package.remove_content_type_override(part_name);
    document.identifiers.retire_authored_part(part_name);
}

pub(super) fn clear_mail_merge_markup(candidate: &mut Document) -> Result<()> {
    let Some(part_name) = resolved_settings_part(candidate)? else {
        return Ok(());
    };
    let current = candidate
        .settings
        .as_ref()
        .ok_or_else(|| Error::Other("mail-merge settings part is missing".to_owned()))?
        .to_xml()?;
    let inspection = inspect_merge_xml(&current)?;
    if inspection.graph.unsafe_field_map {
        return Err(Error::Other(
            "mail-merge field map cannot preserve an unknown assignment".to_owned(),
        ));
    }
    let header_id = inspection.graph.header_id.clone();
    let old_sources = [
        inspection.graph.data_source.clone(),
        inspection.graph.odso_source.clone(),
    ];
    let rewritten = splice_cleared_mail_merge(&current, &inspection)?;
    candidate.settings = Some(oxoffice_docx_oxml::settings::CT_Settings::from_xml(
        &rewritten,
    )?);
    let retained = header_id.into_iter().collect::<Vec<_>>();
    if let Some(relationships) = candidate.package.get_part_rels(&part_name) {
        let remove: Vec<_> = relationships
            .items
            .iter()
            .filter(|item| {
                item.rel_type == MERGE_SOURCE_REL
                    && !retained.iter().any(|id| id == &item.id)
                    && old_sources
                        .iter()
                        .any(|id| id.as_deref() == Some(item.id.as_str()))
            })
            .map(|item| item.id.clone())
            .collect();
        if !remove.is_empty() {
            candidate.package.record_part_rels(&part_name);
            candidate
                .package
                .get_or_create_part_rels(&part_name)
                .items
                .retain(|item| !remove.iter().any(|id| id == &item.id));
            candidate
                .identifiers
                .retire_authored_story_relationships(&part_name, remove);
        }
    }
    Ok(())
}

fn splice_cleared_mail_merge(xml: &[u8], inspection: &MergeInspection) -> Result<Vec<u8>> {
    let Some(region) = inspection.region.as_ref() else {
        return Ok(xml.to_vec());
    };
    let (content_start, content_end) = if region.start_tag_end == region.end {
        (region.end, region.end)
    } else {
        (region.start_tag_end, region.end_tag_start)
    };
    let inner = splice_children(
        xml,
        content_start,
        content_end,
        &region.children,
        oxoffice_docx_oxml::settings::MAIL_MERGE_ORDER,
        |child| clear_mail_child(xml, child),
        Vec::new(),
    )?;
    if inner.iter().all(|byte| byte.is_ascii_whitespace()) {
        let mut out = xml[..region.start].to_vec();
        out.extend_from_slice(&xml[region.end..]);
        return Ok(out);
    }
    let mut out = Vec::with_capacity(xml.len());
    out.extend_from_slice(&xml[..region.start]);
    if region.start_tag_end == region.end {
        out.extend(expand_empty_element(
            &xml[region.start..region.end],
            &inner,
        )?);
    } else {
        out.extend_from_slice(&xml[region.start..region.start_tag_end]);
        out.extend_from_slice(&inner);
        out.extend_from_slice(&xml[region.end_tag_start..region.end]);
    }
    out.extend_from_slice(&xml[region.end..]);
    Ok(out)
}

fn clear_mail_child(xml: &[u8], child: &MergeChild) -> Result<ChildAction> {
    if !child.word {
        return Ok(ChildAction::Keep);
    }
    match child.local.as_slice() {
        b"headerSource" => Ok(ChildAction::Keep),
        b"odso" => {
            let rebuilt = clear_odso(xml, child)?;
            if rebuilt.iter().all(|byte| byte.is_ascii_whitespace()) {
                Ok(ChildAction::Drop)
            } else {
                Ok(ChildAction::Replace(rebuilt))
            }
        }
        b"mainDocumentType"
        | b"linkToQuery"
        | b"dataType"
        | b"connectString"
        | b"query"
        | b"dataSource"
        | b"doNotSuppressBlankLines"
        | b"destination"
        | b"addressFieldName"
        | b"mailSubject"
        | b"mailAsAttachment"
        | b"viewMergedData"
        | b"activeRecord"
        | b"checkErrors" => Ok(ChildAction::Drop),
        _ => Ok(ChildAction::Keep),
    }
}

fn clear_odso(xml: &[u8], child: &MergeChild) -> Result<Vec<u8>> {
    let (content_start, content_end) = if child.start_tag_end == child.end {
        (child.end, child.end)
    } else {
        (child.start_tag_end, child.end_tag_start)
    };
    let inner = splice_children(
        xml,
        content_start,
        content_end,
        &child.children,
        oxoffice_docx_oxml::settings::ODSO_ORDER,
        |nested| {
            if !nested.word {
                return Ok(ChildAction::Keep);
            }
            match nested.local.as_slice() {
                b"src" | b"fieldMapData" | b"recipientData" => Ok(ChildAction::Drop),
                _ => Ok(ChildAction::Keep),
            }
        },
        Vec::new(),
    )?;
    if inner.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(Vec::new());
    }
    if child.start_tag_end == child.end {
        expand_empty_element(&xml[child.start..child.end], &inner)
    } else {
        let mut out = xml[child.start..child.start_tag_end].to_vec();
        out.extend(inner);
        out.extend_from_slice(&xml[child.end_tag_start..child.end]);
        Ok(out)
    }
}

pub(super) fn install_merge_source_graph(
    candidate: &mut Document,
    options: &MailMergePackageOptions,
    recipient_id: Option<&str>,
) -> Result<()> {
    let Some(part_name) = resolved_settings_part(candidate)? else {
        return Err(Error::Other(
            "mail-merge settings part is missing".to_owned(),
        ));
    };
    let current = candidate
        .settings
        .as_ref()
        .ok_or_else(|| Error::Other("mail-merge settings part is missing".to_owned()))?
        .to_xml()?;
    let previous = inspect_merge_xml(&current)?;
    if previous.graph.unsafe_field_map {
        return Err(Error::Other(
            "mail-merge field map cannot preserve an unknown assignment".to_owned(),
        ));
    }
    let header_id = previous.graph.header_id.clone();
    let source_id = if let MailMergePackageSource::External { uri } = &options.source {
        Some(reuse_or_add_source_relationship(
            candidate, &part_name, uri,
        )?)
    } else {
        None
    };
    let rewritten = splice_mail_merge(
        &current,
        &previous,
        source_id.as_deref(),
        recipient_id,
        options,
    )?;
    candidate.settings = Some(oxoffice_docx_oxml::settings::CT_Settings::from_xml(
        &rewritten,
    )?);
    let retained = source_id.into_iter().chain(header_id).collect::<Vec<_>>();
    let old_sources = [
        previous.graph.data_source.clone(),
        previous.graph.odso_source.clone(),
    ];
    if let Some(relationships) = candidate.package.get_part_rels(&part_name) {
        let remove: Vec<_> = relationships
            .items
            .iter()
            .filter(|item| {
                item.rel_type == MERGE_SOURCE_REL
                    && !retained.iter().any(|id| id == &item.id)
                    && old_sources
                        .iter()
                        .any(|id| id.as_deref() == Some(item.id.as_str()))
            })
            .map(|item| item.id.clone())
            .collect();
        if !remove.is_empty() {
            candidate.package.record_part_rels(&part_name);
            candidate
                .package
                .get_or_create_part_rels(&part_name)
                .items
                .retain(|item| !remove.iter().any(|id| id == &item.id));
        }
    }
    Ok(())
}

fn reuse_or_add_source_relationship(
    candidate: &mut Document,
    part_name: &str,
    uri: &str,
) -> Result<String> {
    if let Some(relationships) = candidate.package.get_part_rels(part_name)
        && let Some(existing) = relationships.items.iter().find(|item| {
            item.rel_type == MERGE_SOURCE_REL
                && item.target_mode.as_deref() == Some("External")
                && item.target == uri
        })
    {
        return Ok(existing.id.clone());
    }
    candidate.add_external_relationship_checked(part_name, MERGE_SOURCE_REL, uri)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeFrameKind {
    Root,
    Settings,
    MailMerge,
    Odso,
    FieldMap,
    Other,
}

struct MergeFrame {
    kind: MergeFrameKind,
    local: Vec<u8>,
    word: bool,
    start: usize,
    start_tag_end: usize,
    children: Vec<MergeChild>,
}

struct MergeChild {
    local: Vec<u8>,
    word: bool,
    start: usize,
    start_tag_end: usize,
    end_tag_start: usize,
    end: usize,
    children: Vec<MergeChild>,
}

pub(super) struct MergeRegion {
    start: usize,
    start_tag_end: usize,
    end_tag_start: usize,
    end: usize,
    children: Vec<MergeChild>,
}

enum ChildAction {
    Keep,
    Drop,
    Replace(Vec<u8>),
}

struct PendingInsert {
    local: &'static [u8],
    bytes: Vec<u8>,
}

pub(super) fn inspect_merge_xml(xml: &[u8]) -> Result<MergeInspection> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<MergeFrame> = Vec::new();
    let mut inspection = MergeInspection {
        graph: ParsedMergeGraph::default(),
        region: None,
    };
    let mut slot: Option<FieldSlot> = None;
    let mut slot_index = 0usize;
    let mut field_leaf_depth = 0usize;
    let mut source_leaf_depth = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid mail-merge settings: {error}")))?
            .into_owned();
        let after = reader.buffer_position() as usize;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if field_leaf_depth > 0 {
                    inspection.graph.unsafe_field_map = true;
                    if matches!(event, Event::Start(_)) {
                        field_leaf_depth += 1;
                    }
                }
                if source_leaf_depth > 0 {
                    inspection.graph.unsafe_source = true;
                    if matches!(event, Event::Start(_)) {
                        source_leaf_depth += 1;
                    }
                }
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
                let name = local.as_ref().to_vec();
                let parent = stack
                    .last()
                    .map(|frame| frame.kind)
                    .unwrap_or(MergeFrameKind::Root);
                let kind = merge_frame_kind(parent, word, &name);
                if kind == MergeFrameKind::MailMerge && inspection.region.is_some() {
                    return Err(Error::Other(
                        "mail-merge settings element is ambiguous".to_owned(),
                    ));
                }
                if kind == MergeFrameKind::Odso
                    && stack.last().is_some_and(|frame| {
                        frame
                            .children
                            .iter()
                            .any(|child| child.word && child.local == b"odso")
                    })
                {
                    return Err(Error::Other(
                        "mail-merge ODSO element is ambiguous".to_owned(),
                    ));
                }
                if parent == MergeFrameKind::MailMerge
                    && word
                    && oxoffice_docx_oxml::settings::MAIL_MERGE_MEMBERS
                        .iter()
                        .any(|member| name == member.as_bytes())
                {
                    inspection.graph.modeled_member = true;
                }
                if stack.len() == 2
                    && stack[0].kind == MergeFrameKind::Settings
                    && word
                    && name == b"docVar"
                    && stack[1].word
                    && stack[1].local == b"docVars"
                    && word_val(reader.resolver(), element, b"name")?.as_deref()
                        == Some(MERGE_STORE_DOC_VAR)
                {
                    inspection.graph.owned_docvar = true;
                }
                if parent == MergeFrameKind::MailMerge && word && name == b"dataSource" {
                    if inspection.graph.data_source.is_some() {
                        return Err(Error::Other(
                            "mail-merge data source reference is ambiguous".to_owned(),
                        ));
                    }
                    inspection.graph.data_source = Some(single_relationship_id(
                        reader.resolver(),
                        element,
                        "data source",
                    )?);
                } else if parent == MergeFrameKind::MailMerge && word && name == b"headerSource" {
                    if inspection.graph.header_id.is_some() {
                        return Err(Error::Other(
                            "mail-merge header source reference is ambiguous".to_owned(),
                        ));
                    }
                    inspection.graph.header_id = Some(single_relationship_id(
                        reader.resolver(),
                        element,
                        "header source",
                    )?);
                } else if parent == MergeFrameKind::Odso && word && name == b"src" {
                    if inspection.graph.odso_source.is_some() {
                        return Err(Error::Other(
                            "mail-merge ODSO source reference is ambiguous".to_owned(),
                        ));
                    }
                    inspection.graph.odso_source = Some(single_relationship_id(
                        reader.resolver(),
                        element,
                        "ODSO source",
                    )?);
                } else if parent == MergeFrameKind::Odso && word && name == b"recipientData" {
                    if inspection.graph.recipient_id.is_some() {
                        return Err(Error::Other(
                            "mail-merge recipient reference is duplicated".to_owned(),
                        ));
                    }
                    inspection.graph.recipient_graph = true;
                    inspection.graph.recipient_id = Some(single_relationship_id(
                        reader.resolver(),
                        element,
                        "recipient",
                    )?);
                } else if parent == MergeFrameKind::Odso && word && name == b"fieldMapData" {
                    if slot_index >= MERGE_FIELD_ROLES.len() {
                        inspection.graph.unsupported_field_map = true;
                    }
                    if field_element_is_unsafe(reader.resolver(), element, false)? {
                        inspection.graph.unsafe_field_map = true;
                    }
                    if slot_index < MERGE_FIELD_ROLES.len() {
                        slot = Some(FieldSlot::default());
                        if matches!(event, Event::Empty(_)) {
                            finish_field_slot(&mut inspection.graph, &mut slot, &mut slot_index)?;
                        }
                    }
                } else if parent == MergeFrameKind::FieldMap && word {
                    if let Some(current) = slot.as_mut() {
                        if field_element_is_unsafe(reader.resolver(), element, true)?
                            || !matches!(
                                name.as_slice(),
                                b"type"
                                    | b"name"
                                    | b"mappedName"
                                    | b"column"
                                    | b"lid"
                                    | b"dynamicAddress"
                            )
                        {
                            inspection.graph.unsafe_field_map = true;
                        }
                        let value = word_val(reader.resolver(), element, b"val")?;
                        match name.as_slice() {
                            b"type" if current.kind.is_some() => {
                                return Err(Error::Other(
                                    "mail-merge field map type is duplicated".to_owned(),
                                ));
                            }
                            b"type" => current.kind = value,
                            b"name" if current.name.is_some() => {
                                return Err(Error::Other(
                                    "mail-merge field map name is duplicated".to_owned(),
                                ));
                            }
                            b"name" => current.name = value,
                            b"column" if current.column.is_some() => {
                                return Err(Error::Other(
                                    "mail-merge field map column is duplicated".to_owned(),
                                ));
                            }
                            b"column" => current.column = value,
                            b"lid" if current.language.is_some() => {
                                return Err(Error::Other(
                                    "mail-merge field map language is duplicated".to_owned(),
                                ));
                            }
                            b"lid" => current.language = value,
                            b"dynamicAddress" if current.dynamic_seen => {
                                return Err(Error::Other(
                                    "mail-merge field map dynamic address is duplicated".to_owned(),
                                ));
                            }
                            b"dynamicAddress" => {
                                current.dynamic_seen = true;
                                current.dynamic = value;
                            }
                            _ => {}
                        }
                    }
                } else if parent == MergeFrameKind::FieldMap {
                    inspection.graph.unsafe_field_map = true;
                }
                let source_shaped = matches!(
                    name.as_slice(),
                    b"dataSource" | b"src" | b"headerSource" | b"recipientData"
                );
                if source_shaped
                    && matches!(parent, MergeFrameKind::MailMerge | MergeFrameKind::Odso)
                {
                    let replaced =
                        matches!(name.as_slice(), b"dataSource" | b"src" | b"recipientData");
                    if !word
                        || (replaced && source_reference_is_unsafe(reader.resolver(), element)?)
                    {
                        inspection.graph.unsafe_source = true;
                    }
                    if replaced
                        && word
                        && source_leaf_depth == 0
                        && matches!(event, Event::Start(_))
                    {
                        source_leaf_depth = 1;
                    }
                }
                if field_leaf_depth == 0
                    && parent == MergeFrameKind::FieldMap
                    && word
                    && matches!(event, Event::Start(_))
                    && matches!(
                        name.as_slice(),
                        b"type" | b"name" | b"mappedName" | b"column" | b"lid" | b"dynamicAddress"
                    )
                {
                    field_leaf_depth = 1;
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(MergeFrame {
                        kind,
                        local: name,
                        word,
                        start: before,
                        start_tag_end: after,
                        children: Vec::new(),
                    });
                } else if let Some(parent_frame) = stack.last_mut()
                    && matches!(
                        parent_frame.kind,
                        MergeFrameKind::MailMerge | MergeFrameKind::Odso
                    )
                {
                    parent_frame.children.push(MergeChild {
                        local: name,
                        word,
                        start: before,
                        start_tag_end: after,
                        end_tag_start: after,
                        end: after,
                        children: Vec::new(),
                    });
                }
            }
            Event::Text(text) => {
                if field_leaf_depth > 0 && text_has_payload(text.as_ref()) {
                    inspection.graph.unsafe_field_map = true;
                }
                if source_leaf_depth > 0 && text_has_payload(text.as_ref()) {
                    inspection.graph.unsafe_source = true;
                }
            }
            Event::CData(text) => {
                if field_leaf_depth > 0 && text_has_payload(text.as_ref()) {
                    inspection.graph.unsafe_field_map = true;
                }
            }
            Event::GeneralRef(entity) => {
                if field_leaf_depth > 0 && entity_has_payload(entity) {
                    inspection.graph.unsafe_field_map = true;
                }
                if source_leaf_depth > 0 && entity_has_payload(entity) {
                    inspection.graph.unsafe_source = true;
                }
            }
            Event::End(_) => {
                field_leaf_depth = field_leaf_depth.saturating_sub(1);
                source_leaf_depth = source_leaf_depth.saturating_sub(1);
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "invalid mail-merge settings: unbalanced element".to_owned(),
                    ));
                };
                if frame.kind == MergeFrameKind::FieldMap {
                    finish_field_slot(&mut inspection.graph, &mut slot, &mut slot_index)?;
                }
                let children =
                    if matches!(frame.kind, MergeFrameKind::Odso | MergeFrameKind::MailMerge) {
                        frame.children
                    } else {
                        Vec::new()
                    };
                let child = MergeChild {
                    local: frame.local,
                    word: frame.word,
                    start: frame.start,
                    start_tag_end: frame.start_tag_end,
                    end_tag_start: before,
                    end: after,
                    children,
                };
                if frame.kind == MergeFrameKind::MailMerge {
                    inspection.region = Some(MergeRegion {
                        start: child.start,
                        start_tag_end: child.start_tag_end,
                        end_tag_start: child.end_tag_start,
                        end: child.end,
                        children: child.children,
                    });
                } else if let Some(parent_frame) = stack.last_mut()
                    && matches!(
                        parent_frame.kind,
                        MergeFrameKind::MailMerge | MergeFrameKind::Odso
                    )
                {
                    parent_frame.children.push(child);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(Error::Other(
            "invalid mail-merge settings: unbalanced element".to_owned(),
        ));
    }
    Ok(inspection)
}

fn merge_frame_kind(parent: MergeFrameKind, word: bool, local: &[u8]) -> MergeFrameKind {
    if !word {
        return MergeFrameKind::Other;
    }
    match (parent, local) {
        (MergeFrameKind::Root, b"settings") => MergeFrameKind::Settings,
        (MergeFrameKind::Settings, b"mailMerge") => MergeFrameKind::MailMerge,
        (MergeFrameKind::MailMerge, b"odso") => MergeFrameKind::Odso,
        (MergeFrameKind::Odso, b"fieldMapData") => MergeFrameKind::FieldMap,
        _ => MergeFrameKind::Other,
    }
}

fn source_reference_is_unsafe(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge source reference: {error}"))
        })?;
        let raw = attribute.key.as_ref();
        if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == R_NS.as_bytes())
            && local.as_ref() == b"id"
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn field_element_is_unsafe(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    allow_value: bool,
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge field map attribute: {error}"))
        })?;
        let raw = attribute.key.as_ref();
        if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
            continue;
        }
        if allow_value {
            let (namespace, local) = resolver.resolve_attribute(attribute.key);
            if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                && local.as_ref() == b"val"
            {
                continue;
            }
        }
        return Ok(true);
    }
    Ok(false)
}

fn single_relationship_id(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    role: &str,
) -> Result<String> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge {role} reference: {error}"))
        })?;
        let raw = attribute.key.as_ref();
        if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == R_NS.as_bytes())
            && local.as_ref() == b"id"
        {
            if found.is_some() {
                return Err(Error::Other(format!(
                    "mail-merge {role} reference is ambiguous"
                )));
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    Error::Other(format!("invalid mail-merge {role} reference: {error}"))
                })?;
            if value.is_empty() {
                return Err(Error::Other(format!(
                    "mail-merge {role} reference is missing"
                )));
            }
            found = Some(value.into_owned());
        }
    }
    found.ok_or_else(|| Error::Other(format!("mail-merge {role} reference is missing")))
}

fn splice_mail_merge(
    xml: &[u8],
    inspection: &MergeInspection,
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<Vec<u8>> {
    let region = inspection
        .region
        .as_ref()
        .ok_or_else(|| Error::Other("mail-merge settings element is missing".to_owned()))?;
    let inner = splice_mail_children(xml, region, source_id, recipient_id, options)?;
    let mut out = Vec::with_capacity(xml.len() + inner.len());
    out.extend_from_slice(&xml[..region.start]);
    if region.start_tag_end == region.end {
        out.extend(expand_empty_element(
            &xml[region.start..region.end],
            &inner,
        )?);
    } else {
        out.extend_from_slice(&xml[region.start..region.start_tag_end]);
        out.extend_from_slice(&inner);
        out.extend_from_slice(&xml[region.end_tag_start..region.end]);
    }
    out.extend_from_slice(&xml[region.end..]);
    Ok(out)
}

fn splice_mail_children(
    xml: &[u8],
    region: &MergeRegion,
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<Vec<u8>> {
    let settings = &options.settings;
    let mut pending = Vec::new();
    queue_text(
        &mut pending,
        &region.children,
        b"connectString",
        settings.connect_string.as_deref(),
    )?;
    queue_text(
        &mut pending,
        &region.children,
        b"query",
        settings.query.as_deref(),
    )?;
    if let Some(id) = source_id {
        let mut bytes = Vec::new();
        push_relationship_element(&mut bytes, "dataSource", id)?;
        queue_absent(&mut pending, &region.children, b"dataSource", bytes);
    }
    queue_text(
        &mut pending,
        &region.children,
        b"addressFieldName",
        settings.address_field_name.as_deref(),
    )?;
    queue_text(
        &mut pending,
        &region.children,
        b"mailSubject",
        settings.mail_subject.as_deref(),
    )?;
    if !region
        .children
        .iter()
        .any(|child| child.word && child.local == b"odso")
    {
        pending.push(PendingInsert {
            local: b"odso",
            bytes: synthetic_odso(source_id, recipient_id, options)?,
        });
    }
    let (content_start, content_end) = if region.start_tag_end == region.end {
        (region.end, region.end)
    } else {
        (region.start_tag_end, region.end_tag_start)
    };
    splice_children(
        xml,
        content_start,
        content_end,
        &region.children,
        oxoffice_docx_oxml::settings::MAIL_MERGE_ORDER,
        |child| mail_child_action(xml, child, source_id, recipient_id, options),
        pending,
    )
}

fn mail_child_action(
    xml: &[u8],
    child: &MergeChild,
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<ChildAction> {
    if !child.word {
        return Ok(ChildAction::Keep);
    }
    match child.local.as_slice() {
        b"dataSource" => match source_id {
            Some(id) => {
                let mut bytes = Vec::new();
                push_relationship_element(&mut bytes, "dataSource", id)?;
                Ok(ChildAction::Replace(bytes))
            }
            None => Ok(ChildAction::Drop),
        },
        b"headerSource" => Ok(ChildAction::Keep),
        b"odso" => Ok(ChildAction::Replace(rebuild_odso(
            xml,
            child,
            source_id,
            recipient_id,
            options,
        )?)),
        b"connectString" => {
            text_action(options.settings.connect_string.as_deref(), "connectString")
        }
        b"query" => text_action(options.settings.query.as_deref(), "query"),
        b"addressFieldName" => text_action(
            options.settings.address_field_name.as_deref(),
            "addressFieldName",
        ),
        b"mailSubject" => text_action(options.settings.mail_subject.as_deref(), "mailSubject"),
        _ => Ok(ChildAction::Keep),
    }
}

fn rebuild_odso(
    xml: &[u8],
    child: &MergeChild,
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<Vec<u8>> {
    let inner = splice_odso_children(
        xml,
        if child.start_tag_end == child.end {
            child.end
        } else {
            child.start_tag_end
        },
        if child.start_tag_end == child.end {
            child.end
        } else {
            child.end_tag_start
        },
        &child.children,
        source_id,
        recipient_id,
        options,
    )?;
    if child.start_tag_end == child.end {
        expand_empty_element(&xml[child.start..child.end], &inner)
    } else {
        let mut out = xml[child.start..child.start_tag_end].to_vec();
        out.extend(inner);
        out.extend_from_slice(&xml[child.end_tag_start..child.end]);
        Ok(out)
    }
}

fn synthetic_odso(
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<Vec<u8>> {
    let inner = splice_odso_children(b"", 0, 0, &[], source_id, recipient_id, options)?;
    let mut out = format!("<w:odso xmlns:w=\"{W_NS}\">").into_bytes();
    out.extend(inner);
    out.extend_from_slice(b"</w:odso>");
    Ok(out)
}

fn splice_odso_children(
    xml: &[u8],
    content_start: usize,
    content_end: usize,
    children: &[MergeChild],
    source_id: Option<&str>,
    recipient_id: Option<&str>,
    options: &MailMergePackageOptions,
) -> Result<Vec<u8>> {
    let maps = field_map_elements(&options.mappings)?;
    let mut pending = Vec::new();
    if let Some(id) = source_id {
        let mut bytes = Vec::new();
        push_relationship_element(&mut bytes, "src", id)?;
        queue_absent(&mut pending, children, b"src", bytes);
    }
    if let Some(id) = recipient_id {
        let mut bytes = Vec::new();
        push_relationship_element(&mut bytes, "recipientData", id)?;
        queue_absent(&mut pending, children, b"recipientData", bytes);
    }
    queue_absent(&mut pending, children, b"fieldMapData", maps.clone());
    let mut replaced_maps = false;
    splice_children(
        xml,
        content_start,
        content_end,
        children,
        oxoffice_docx_oxml::settings::ODSO_ORDER,
        |child| {
            if !child.word {
                return Ok(ChildAction::Keep);
            }
            match child.local.as_slice() {
                b"src" => match source_id {
                    Some(id) => {
                        let mut bytes = Vec::new();
                        push_relationship_element(&mut bytes, "src", id)?;
                        Ok(ChildAction::Replace(bytes))
                    }
                    None => Ok(ChildAction::Drop),
                },
                b"fieldMapData" => {
                    if replaced_maps {
                        Ok(ChildAction::Drop)
                    } else {
                        replaced_maps = true;
                        Ok(ChildAction::Replace(maps.clone()))
                    }
                }
                b"recipientData" => match recipient_id {
                    Some(id) => {
                        let mut bytes = Vec::new();
                        push_relationship_element(&mut bytes, "recipientData", id)?;
                        Ok(ChildAction::Replace(bytes))
                    }
                    None => Ok(ChildAction::Drop),
                },
                _ => Ok(ChildAction::Keep),
            }
        },
        pending,
    )
}

fn splice_children(
    xml: &[u8],
    content_start: usize,
    content_end: usize,
    children: &[MergeChild],
    order: &[&[u8]],
    mut decide: impl FnMut(&MergeChild) -> Result<ChildAction>,
    mut pending: Vec<PendingInsert>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cursor = content_start;
    for child in children {
        if cursor < child.start && child.start <= xml.len() {
            out.extend_from_slice(&xml[cursor..child.start]);
        }
        if child.word
            && let Some(index) = order
                .iter()
                .position(|name| *name == child.local.as_slice())
        {
            out.extend(take_pending_before(&mut pending, order, Some(index)));
        }
        match decide(child)? {
            ChildAction::Keep => {
                if child.end <= xml.len() {
                    out.extend_from_slice(&xml[child.start..child.end]);
                }
            }
            ChildAction::Drop => {}
            ChildAction::Replace(bytes) => out.extend(bytes),
        }
        cursor = child.end;
    }
    if cursor < content_end && content_end <= xml.len() {
        out.extend_from_slice(&xml[cursor..content_end]);
    }
    out.extend(take_pending_before(&mut pending, order, None));
    Ok(out)
}

fn take_pending_before(
    pending: &mut Vec<PendingInsert>,
    order: &[&[u8]],
    limit: Option<usize>,
) -> Vec<u8> {
    let mut emitted = Vec::new();
    let mut index = 0;
    while index < pending.len() {
        let position = order.iter().position(|name| *name == pending[index].local);
        let ready = match (position, limit) {
            (Some(position), Some(limit)) => position < limit,
            (Some(_), None) | (None, None) => true,
            (None, Some(_)) => false,
        };
        if ready {
            emitted.extend(pending.remove(index).bytes);
        } else {
            index += 1;
        }
    }
    emitted
}

fn queue_text(
    pending: &mut Vec<PendingInsert>,
    children: &[MergeChild],
    local: &'static [u8],
    value: Option<&str>,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let mut bytes = Vec::new();
    push_valued_element(
        &mut bytes,
        std::str::from_utf8(local).unwrap_or("value"),
        value,
    )?;
    queue_absent(pending, children, local, bytes);
    Ok(())
}

fn queue_absent(
    pending: &mut Vec<PendingInsert>,
    children: &[MergeChild],
    local: &'static [u8],
    bytes: Vec<u8>,
) {
    if bytes.is_empty()
        || children
            .iter()
            .any(|child| child.word && child.local.as_slice() == local)
    {
        return;
    }
    pending.push(PendingInsert { local, bytes });
}

fn text_action(value: Option<&str>, local: &str) -> Result<ChildAction> {
    match value {
        Some(value) => {
            let mut bytes = Vec::new();
            push_valued_element(&mut bytes, local, value)?;
            Ok(ChildAction::Replace(bytes))
        }
        None => Ok(ChildAction::Drop),
    }
}

fn push_valued_element(output: &mut Vec<u8>, local: &str, value: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("mail-merge setting", value)?;
    output.extend_from_slice(b"<w:");
    output.extend_from_slice(local.as_bytes());
    output.extend_from_slice(b" xmlns:w=\"");
    output.extend_from_slice(W_NS.as_bytes());
    output.extend_from_slice(b"\" w:val=\"");
    push_merge_attribute(output, value);
    output.extend_from_slice(b"\"/>");
    Ok(())
}

fn push_relationship_element(output: &mut Vec<u8>, local: &str, id: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("mail-merge relationship", id)?;
    output.extend_from_slice(b"<w:");
    output.extend_from_slice(local.as_bytes());
    output.extend_from_slice(b" xmlns:w=\"");
    output.extend_from_slice(W_NS.as_bytes());
    output.extend_from_slice(b"\" xmlns:r=\"");
    output.extend_from_slice(R_NS.as_bytes());
    output.extend_from_slice(b"\" r:id=\"");
    push_merge_attribute(output, id);
    output.extend_from_slice(b"\"/>");
    Ok(())
}

fn field_map_elements(mappings: &[MailMergeFieldMapping]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for role in MERGE_FIELD_ROLES {
        output.extend_from_slice(format!("<w:fieldMapData xmlns:w=\"{W_NS}\">").as_bytes());
        if let Some(mapping) = mappings.iter().find(|item| item.mapped_name == role) {
            oxoffice_core::xml::reject_non_xml_characters(
                "mail-merge field name",
                &mapping.source_name,
            )?;
            output.extend_from_slice(b"<w:type w:val=\"dbColumn\"/>");
            push_valued_element(&mut output, "name", &mapping.source_name)?;
            push_valued_element(&mut output, "mappedName", role)?;
            push_valued_element(&mut output, "column", &mapping.column.to_string())?;
            if let Some(language) = &mapping.language {
                oxoffice_core::xml::reject_non_xml_characters(
                    "mail-merge field language",
                    language,
                )?;
                push_valued_element(&mut output, "lid", language)?;
            }
            if let Some(dynamic) = mapping.dynamic_address {
                push_valued_element(
                    &mut output,
                    "dynamicAddress",
                    if dynamic { "true" } else { "false" },
                )?;
            }
        } else {
            output.extend_from_slice(b"<w:type w:val=\"null\"/>");
        }
        output.extend_from_slice(b"</w:fieldMapData>");
    }
    Ok(output)
}

fn expand_empty_element(raw: &[u8], inner: &[u8]) -> Result<Vec<u8>> {
    if raw.len() < 3 || !raw.starts_with(b"<") || &raw[raw.len() - 2..] != b"/>" {
        return Err(Error::Other(
            "mail-merge element span is malformed".to_owned(),
        ));
    }
    let name_end = raw
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || *byte == b'/' || *byte == b'>')
        .unwrap_or(raw.len());
    let name = &raw[1..name_end];
    let mut out = raw[..raw.len() - 2].to_vec();
    out.push(b'>');
    out.extend_from_slice(inner);
    out.extend_from_slice(b"</");
    out.extend_from_slice(name);
    out.push(b'>');
    Ok(out)
}

#[derive(Default)]
struct FieldSlot {
    kind: Option<String>,
    name: Option<String>,
    column: Option<String>,
    language: Option<String>,
    dynamic: Option<String>,
    dynamic_seen: bool,
}

fn finish_field_slot(
    graph: &mut ParsedMergeGraph,
    slot: &mut Option<FieldSlot>,
    slot_index: &mut usize,
) -> Result<()> {
    let Some(current) = slot.take() else {
        return Ok(());
    };
    graph.recognized_field_map = true;
    let role = MERGE_FIELD_ROLES[*slot_index];
    *slot_index += 1;
    match current.kind.as_deref() {
        None | Some("null") => Ok(()),
        Some("dbColumn") => {
            let column = current
                .column
                .ok_or_else(|| Error::Other("mail-merge field column is missing".to_owned()))?;
            let column = column
                .parse::<u32>()
                .map_err(|_| Error::Other("mail-merge field column is missing".to_owned()))?;
            graph.mappings.push(MailMergeFieldMapping {
                column,
                source_name: current.name.unwrap_or_default(),
                mapped_name: role.to_owned(),
                language: current.language,
                dynamic_address: if current.dynamic_seen {
                    Some(parse_recipient_active(&current.dynamic)?)
                } else {
                    None
                },
            });
            Ok(())
        }
        Some(_) => {
            graph.unsupported_field_map = true;
            Ok(())
        }
    }
}

fn word_val(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    attribute_name: &[u8],
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge settings attribute: {error}"))
        })?;
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            && local.as_ref() == attribute_name
        {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    Error::Other(format!("invalid mail-merge settings attribute: {error}"))
                })?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

pub(super) fn resolve_settings_source_target(
    document: &Document,
    relationship_id: &str,
) -> Result<String> {
    let part_name = resolved_settings_part(document)?
        .ok_or_else(|| Error::Other("mail-merge settings part is missing".to_owned()))?;
    let relationships = document
        .package
        .get_part_rels(&part_name)
        .ok_or_else(|| Error::Other("mail-merge source relationship is missing".to_owned()))?;
    let matches: Vec<_> = relationships
        .items
        .iter()
        .filter(|item| item.id == relationship_id)
        .collect();
    let [item] = matches.as_slice() else {
        return Err(Error::Other(
            "mail-merge source relationship is ambiguous".to_owned(),
        ));
    };
    if item.target_mode.as_deref() != Some("External") || item.rel_type != MERGE_SOURCE_REL {
        return Err(Error::Other(
            "mail-merge source relationship is not an external mailMergeSource".to_owned(),
        ));
    }
    Ok(item.target.clone())
}

pub(super) fn require_authored_merge_settings(settings: &crate::MailMerge) -> Result<()> {
    if settings.main_document_type.is_none() {
        return Err(Error::Other(
            "mail-merge main document type is required".to_owned(),
        ));
    }
    if !settings
        .data_type
        .as_deref()
        .is_some_and(|value| MERGE_DATA_TYPES.contains(&value))
    {
        return Err(Error::Other(
            "mail-merge data type is not a known value".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn merge_configuration_recognized(graph: &ParsedMergeGraph) -> bool {
    graph.modeled_member
        || graph.owned_docvar
        || graph.data_source.is_some()
        || graph.odso_source.is_some()
        || graph.recognized_field_map
        || graph.recipient_id.is_some()
        || graph.recipient_graph
        || graph.unsafe_field_map
        || graph.unsafe_source
        || graph.unsupported_field_map
}

pub(super) fn mail_merge_has_modeled_value(settings: &crate::MailMerge) -> bool {
    settings.main_document_type.is_some()
        || settings.link_to_query.is_some()
        || settings.data_type.is_some()
        || settings.connect_string.is_some()
        || settings.query.is_some()
        || settings.do_not_suppress_blank_lines.is_some()
        || settings.destination.is_some()
        || settings.address_field_name.is_some()
        || settings.mail_subject.is_some()
        || settings.mail_as_attachment.is_some()
        || settings.view_merged_data.is_some()
        || settings.active_record.is_some()
        || settings.check_errors.is_some()
}

fn resolved_settings_part(document: &Document) -> Result<Option<String>> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Ok(None);
    };
    let owners: Vec<_> = relationships
        .items
        .iter()
        .filter(|item| {
            item.rel_type == rel_types::SETTINGS && crate::document::relationship_is_internal(item)
        })
        .collect();
    let relationship = match owners.as_slice() {
        [] => return Ok(None),
        [relationship] => relationship,
        _ => {
            return Err(Error::Other(
                "mail-merge settings owner is ambiguous".to_owned(),
            ));
        }
    };
    let resolved = OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
    let keys: Vec<_> = document
        .package
        .parts
        .keys()
        .filter(|name| name.eq_ignore_ascii_case(&resolved))
        .cloned()
        .collect();
    match keys.as_slice() {
        [name] => Ok(Some(name.clone())),
        [] => Err(Error::Other(
            "mail-merge settings part is missing".to_owned(),
        )),
        _ => Err(Error::Other(
            "mail-merge settings part is ambiguous".to_owned(),
        )),
    }
}

fn internal_part_targets(package: &OpcPackage, part_name: &str) -> usize {
    let wanted = part_name.to_ascii_lowercase();
    package
        .part_rels
        .iter()
        .flat_map(|(owner, relationships)| {
            relationships.items.iter().filter(|item| {
                item.target_mode.as_deref() != Some("External")
                    && OpcPackage::resolve_rel_target(owner, &item.target)
                        .eq_ignore_ascii_case(&wanted)
            })
        })
        .count()
}

pub(super) fn merge_store_has_other_owner(
    document: &Document,
    part_name: &str,
    item_id: &str,
) -> Result<bool> {
    let expected = crate::content_control::normalize_store_item_id(item_id);
    if document_or_parts_bind_store(document, &expected)? {
        return Ok(true);
    }
    if document.settings.as_ref().is_some_and(|settings| {
        settings.document_variables().iter().any(|variable| {
            variable.name != MERGE_STORE_DOC_VAR
                && crate::content_control::normalize_store_item_id(&variable.value) == expected
        })
    }) {
        return Ok(true);
    }
    Ok(internal_part_targets(&document.package, part_name) != 1)
}

fn document_or_parts_bind_store(document: &Document, expected: &str) -> Result<bool> {
    if body_binds_store(document, expected)? {
        return Ok(true);
    }
    for (name, xml) in &document.package.parts {
        if name.eq_ignore_ascii_case(&document.doc_part_name) {
            // The body model is the main part; its stored bytes may be older.
            continue;
        }
        let Some(content_type) = document.package.content_types.content_type_for(name) else {
            continue;
        };
        if !content_type_is_xml(content_type) {
            continue;
        }
        if xml_binds_store(xml, expected)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a control of the body binds the store `expected`, read from the
/// typed controls rather than from the serialized body. A body that holds a
/// block kept as raw XML is serialized and scanned as before.
fn body_binds_store(document: &Document, expected: &str) -> Result<bool> {
    let body = &document.document.body;
    if body
        .content
        .iter()
        .any(|block| matches!(block, oxoffice_docx_oxml::document::BodyContent::RawXml(_)))
    {
        return xml_binds_store(&document.document.to_xml()?, expected);
    }
    let bound = body.content_controls().into_iter().any(|control| {
        control
            .properties
            .as_ref()
            .and_then(|properties| properties.data_binding.as_ref())
            .and_then(|binding| binding.store_item_id.as_deref())
            .is_some_and(|id| crate::content_control::normalize_store_item_id(id) == expected)
    });
    debug_assert_eq!(
        bound,
        xml_binds_store(&document.document.to_xml()?, expected)?,
        "the typed controls tell another store binding than the serialized body"
    );
    Ok(bound)
}

fn content_type_is_xml(content_type: &str) -> bool {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    base.eq_ignore_ascii_case("application/xml")
        || base.eq_ignore_ascii_case("text/xml")
        || base.to_ascii_lowercase().ends_with("+xml")
}

fn xml_binds_store(xml: &[u8], expected: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid mail-merge binding scan: {error}")))?
            .into_owned();
        match &event {
            Event::Empty(element) | Event::Start(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                    && local.as_ref() == b"dataBinding"
                    && attribute_store_id(reader.resolver(), element, expected)?
                {
                    return Ok(true);
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn attribute_store_id(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    expected: &str,
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid mail-merge binding scan: {error}")))?;
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            && local.as_ref() == b"storeItemID"
        {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    Error::Other(format!("invalid mail-merge binding scan: {error}"))
                })?;
            return Ok(crate::content_control::normalize_store_item_id(value.as_ref()) == expected);
        }
    }
    Ok(false)
}

pub(super) fn require_merge_store_schema(package: &OpcPackage, item_part: &str) -> Result<()> {
    let relationships = package.get_part_rels(item_part).ok_or_else(|| {
        Error::Other("mail-merge store properties relationship is missing".to_owned())
    })?;
    let properties: Vec<_> = relationships
        .items
        .iter()
        .filter(|item| item.rel_type == MERGE_PROPS_REL)
        .collect();
    if properties.len() != 1 {
        return Err(Error::Other(
            "mail-merge store properties relationship is not unique".to_owned(),
        ));
    }
    if !crate::document::relationship_is_internal(properties[0]) {
        return Err(Error::Other(
            if properties[0].target_mode.as_deref() == Some("External") {
                "mail-merge store properties relationship is external".to_owned()
            } else {
                "mail-merge store properties relationship mode is invalid".to_owned()
            },
        ));
    }
    let properties_part = OpcPackage::resolve_rel_target(item_part, &properties[0].target);
    let xml = package
        .get_part(&properties_part)
        .ok_or_else(|| Error::Other("mail-merge store properties part is missing".to_owned()))?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut saw_schema = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid mail-merge store properties: {error}")))?
            .into_owned();
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let props = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == MERGE_PROPS_NS);
                let name = local.as_ref();
                let parent = stack.last().map(Vec::as_slice);
                let allowed = match parent {
                    None => props && name == b"datastoreItem",
                    Some(b"datastoreItem") => props && name == b"schemaRefs",
                    Some(b"schemaRefs") => props && name == b"schemaRef",
                    _ => false,
                };
                if !allowed {
                    return Err(Error::Other(
                        "mail-merge store properties are not a proved schema identity".to_owned(),
                    ));
                }
                if name == b"schemaRef" {
                    if saw_schema
                        || !attribute_is_schema(reader.resolver(), element, MERGE_DATA_NS)?
                    {
                        return Err(Error::Other(
                            "mail-merge package source schema does not match".to_owned(),
                        ));
                    }
                    saw_schema = true;
                }
                if matches!(&event, Event::Start(_)) {
                    stack.push(name.to_vec());
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
    if saw_schema && stack.is_empty() {
        Ok(())
    } else {
        Err(Error::Other(
            "mail-merge package source schema does not match".to_owned(),
        ))
    }
}

fn attribute_is_schema(
    resolver: &NamespaceResolver,
    element: &BytesStart<'_>,
    schema: &str,
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid mail-merge store properties: {error}"))
        })?;
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == MERGE_PROPS_NS)
            && local.as_ref() == b"uri"
        {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    Error::Other(format!("invalid mail-merge store properties: {error}"))
                })?;
            return Ok(value.as_ref() == schema);
        }
    }
    Ok(false)
}

const MERGE_TABLE_XML_CAP: usize = 16 * 1024 * 1024;
const MERGE_TABLE_ROW_CAP: usize = 100_000;
const MERGE_TABLE_COLUMN_CAP: usize = 1024;
const MERGE_TABLE_OPEN: &str = "<mergeData xmlns=\"urn:rdocx:mail-merge-data:v1\"><columns>";
const MERGE_TABLE_AFTER_COLUMNS: &str = "</columns><records>";
const MERGE_TABLE_RECORD_OPEN: &str = "<record>";
const MERGE_TABLE_RECORD_CLOSE: &str = "</record>";
const MERGE_TABLE_CELL_OPEN: &str = "<cell>";
const MERGE_TABLE_CELL_CLOSE: &str = "</cell>";
const MERGE_TABLE_TAIL: &str = "</records></mergeData>";
const MERGE_TABLE_ZERO_TAIL: &str = "</columns><records></records></mergeData>";

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeTableBudgetPhase {
    Columns,
    Record,
    Between,
}

/// Counts one owned embedded table before any output buffer is allocated.
///
/// The native encoder and the later Python embedded factory share this cap.
/// Column names are owned only after the borrowed name fits the budget.
#[doc(hidden)]
pub struct MailMergeTableBudget {
    encoded: usize,
    column_count: usize,
    row_count: usize,
    cells_in_row: usize,
    columns: HashSet<String>,
    phase: MergeTableBudgetPhase,
}

impl Default for MailMergeTableBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl MailMergeTableBudget {
    pub fn new() -> Self {
        Self {
            encoded: MERGE_TABLE_OPEN.len(),
            column_count: 0,
            row_count: 0,
            cells_in_row: 0,
            columns: HashSet::new(),
            phase: MergeTableBudgetPhase::Columns,
        }
    }

    pub fn add_column(&mut self, name: &str) -> Result<()> {
        if self.phase != MergeTableBudgetPhase::Columns {
            return Err(merge_table_error(
                "mail-merge table column was added after records",
            ));
        }
        oxoffice_core::xml::reject_non_xml_characters("mail-merge column", name)?;
        if name.is_empty() {
            return Err(merge_table_error("mail-merge table column is empty"));
        }
        if self.column_count == MERGE_TABLE_COLUMN_CAP {
            return Err(merge_table_error("mail-merge table exceeds 1024 columns"));
        }
        if self.columns.contains(name) {
            return Err(merge_table_error("mail-merge table has a duplicate column"));
        }
        let extra = merge_checked_add("<column name=\"".len(), merge_attribute_escaped_len(name)?)?;
        let extra = merge_checked_add(extra, "\"/>".len())?;
        self.charge(extra, MERGE_TABLE_ZERO_TAIL.len())?;
        self.columns.insert(name.to_owned());
        self.column_count += 1;
        Ok(())
    }

    pub fn begin_record(&mut self) -> Result<()> {
        if self.column_count == 0 {
            return Err(merge_table_error("mail-merge table has no columns"));
        }
        if self.phase == MergeTableBudgetPhase::Record {
            return Err(merge_table_error("mail-merge table row is open"));
        }
        if self.row_count == MERGE_TABLE_ROW_CAP {
            return Err(merge_table_error("mail-merge table exceeds 100000 rows"));
        }
        let extra = if self.phase == MergeTableBudgetPhase::Columns {
            merge_checked_add(
                MERGE_TABLE_AFTER_COLUMNS.len(),
                MERGE_TABLE_RECORD_OPEN.len(),
            )?
        } else {
            MERGE_TABLE_RECORD_OPEN.len()
        };
        let close = merge_checked_add(MERGE_TABLE_RECORD_CLOSE.len(), MERGE_TABLE_TAIL.len())?;
        self.charge(extra, close)?;
        self.phase = MergeTableBudgetPhase::Record;
        self.cells_in_row = 0;
        self.row_count += 1;
        Ok(())
    }

    pub fn add_cell(&mut self, value: &str) -> Result<()> {
        if self.phase != MergeTableBudgetPhase::Record {
            return Err(merge_table_error("mail-merge table row is not open"));
        }
        oxoffice_core::xml::reject_non_xml_characters("mail-merge cell", value)?;
        if self.cells_in_row == self.column_count {
            return Err(merge_table_error("mail-merge table row is not rectangular"));
        }
        let extra = merge_checked_add(MERGE_TABLE_CELL_OPEN.len(), merge_text_escaped_len(value)?)?;
        let extra = merge_checked_add(extra, MERGE_TABLE_CELL_CLOSE.len())?;
        let close = merge_checked_add(MERGE_TABLE_RECORD_CLOSE.len(), MERGE_TABLE_TAIL.len())?;
        self.charge(extra, close)?;
        self.cells_in_row += 1;
        Ok(())
    }

    pub fn end_record(&mut self) -> Result<()> {
        if self.phase != MergeTableBudgetPhase::Record || self.cells_in_row != self.column_count {
            return Err(merge_table_error("mail-merge table row is not rectangular"));
        }
        self.charge(MERGE_TABLE_RECORD_CLOSE.len(), MERGE_TABLE_TAIL.len())?;
        self.phase = MergeTableBudgetPhase::Between;
        Ok(())
    }

    pub fn finish(&self) -> Result<usize> {
        if self.column_count == 0 {
            return Err(merge_table_error("mail-merge table has no columns"));
        }
        if self.phase == MergeTableBudgetPhase::Record {
            return Err(merge_table_error("mail-merge table row is open"));
        }
        let close = if self.phase == MergeTableBudgetPhase::Columns {
            MERGE_TABLE_ZERO_TAIL.len()
        } else {
            MERGE_TABLE_TAIL.len()
        };
        let total = merge_checked_add(self.encoded, close)?;
        if total > MERGE_TABLE_XML_CAP {
            return Err(merge_table_error("mail-merge table exceeds 16 MiB"));
        }
        Ok(total)
    }

    fn charge(&mut self, extra: usize, close: usize) -> Result<()> {
        let next = merge_checked_add(self.encoded, extra)?;
        let projected = merge_checked_add(next, close)?;
        if projected > MERGE_TABLE_XML_CAP {
            return Err(merge_table_error("mail-merge table exceeds 16 MiB"));
        }
        self.encoded = next;
        Ok(())
    }
}

fn merge_table_error(message: impl Into<String>) -> Error {
    Error::Other(message.into())
}

fn merge_checked_add(left: usize, right: usize) -> Result<usize> {
    left.checked_add(right)
        .ok_or_else(|| merge_table_error("mail-merge table size overflow"))
}

fn merge_text_escaped_len(value: &str) -> Result<usize> {
    let mut len = 0usize;
    for character in value.chars() {
        let extra = match character {
            '&' => 5,
            '<' | '>' => 4,
            '\r' => 5,
            _ => character.len_utf8(),
        };
        len = merge_checked_add(len, extra)?;
    }
    Ok(len)
}

fn merge_attribute_escaped_len(value: &str) -> Result<usize> {
    let mut len = 0usize;
    for character in value.chars() {
        let extra = match character {
            '&' => 5,
            '<' | '>' => 4,
            '"' => 6,
            '\t' => 4,
            '\n' | '\r' => 5,
            _ => character.len_utf8(),
        };
        len = merge_checked_add(len, extra)?;
    }
    Ok(len)
}

fn push_merge_text(output: &mut Vec<u8>, value: &str) {
    let mut encoded = [0u8; 4];
    for character in value.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '\r' => output.extend_from_slice(b"&#13;"),
            _ => {
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
}

fn push_merge_attribute(output: &mut Vec<u8>, value: &str) {
    let mut encoded = [0u8; 4];
    for character in value.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '"' => output.extend_from_slice(b"&quot;"),
            '\t' => output.extend_from_slice(b"&#9;"),
            '\n' => output.extend_from_slice(b"&#10;"),
            '\r' => output.extend_from_slice(b"&#13;"),
            _ => {
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
}

/// Encode one owned embedded mail-merge table.
///
/// The byte budget includes markup and escapes and is counted before the
/// output buffer is allocated. Callers, including a later Python binding,
/// reuse this limit instead of inventing another cap.
pub(super) fn encode_merge_table(columns: &[String], records: &[Vec<String>]) -> Result<Vec<u8>> {
    let mut budget = MailMergeTableBudget::new();
    for column in columns {
        budget.add_column(column)?;
    }
    for row in records {
        budget.begin_record()?;
        for cell in row {
            budget.add_cell(cell)?;
        }
        budget.end_record()?;
    }
    let counted = budget.finish()?;

    let mut output = Vec::with_capacity(counted);
    output.extend_from_slice(b"<mergeData xmlns=\"urn:rdocx:mail-merge-data:v1\"><columns>");
    for column in columns {
        output.extend_from_slice(b"<column name=\"");
        push_merge_attribute(&mut output, column);
        output.extend_from_slice(b"\"/>");
    }
    output.extend_from_slice(b"</columns><records>");
    for row in records {
        output.extend_from_slice(b"<record>");
        for cell in row {
            output.extend_from_slice(b"<cell>");
            push_merge_text(&mut output, cell);
            output.extend_from_slice(b"</cell>");
        }
        output.extend_from_slice(b"</record>");
    }
    output.extend_from_slice(b"</records></mergeData>");
    if output.len() != counted || output.len() > MERGE_TABLE_XML_CAP {
        return Err(merge_table_error(
            "mail-merge table size preflight disagreed",
        ));
    }
    validate_strict_xml_1_0(&output).map_err(|error| {
        merge_table_error(format!("mail-merge table XML is not strict: {error:?}"))
    })?;
    Ok(output)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeParent {
    MergeData,
    Columns,
    Column,
    Records,
    Record,
    Cell,
}

fn namespace_is_merge(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == MERGE_DATA_NS.as_bytes())
}

fn append_merge_decoded(target: &mut String, value: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("mail-merge cell", value)?;
    target.push_str(value);
    Ok(())
}

fn append_merge_reference(target: &mut String, raw: &[u8]) -> Result<()> {
    let raw_text = std::str::from_utf8(raw)
        .map_err(|_| merge_table_error("mail-merge cell reference is not UTF-8"))?;
    let value = quick_xml::escape::unescape(raw_text).map_err(|error| {
        merge_table_error(format!("invalid mail-merge character reference: {error}"))
    })?;
    append_merge_decoded(target, &value)
}

fn merge_attribute_name(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<String> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| merge_table_error(format!("invalid mail-merge attribute: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"name" {
            if found.is_some() {
                return Err(merge_table_error(
                    "mail-merge column name attribute is duplicated",
                ));
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    merge_table_error(format!("invalid mail-merge column name: {error}"))
                })?;
            found = Some(value.into_owned());
        } else {
            return Err(merge_table_error(
                "mail-merge column has an unexpected attribute",
            ));
        }
    }
    let name = found.ok_or_else(|| merge_table_error("mail-merge column name is missing"))?;
    oxoffice_core::xml::reject_non_xml_characters("mail-merge column", &name)?;
    if name.is_empty() {
        return Err(merge_table_error("mail-merge table column is empty"));
    }
    Ok(name)
}

fn merge_whitespace(value: &str) -> bool {
    value
        .chars()
        .all(|character| matches!(character, ' ' | '\t' | '\n' | '\r'))
}

struct MergeTableParse {
    stack: Vec<MergeParent>,
    columns: Vec<String>,
    records: Vec<Vec<String>>,
    current_record: Vec<String>,
    current_cell: String,
    saw_root: bool,
    saw_columns: bool,
    saw_records: bool,
}

impl MergeTableParse {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            columns: Vec::new(),
            records: Vec::new(),
            current_record: Vec::new(),
            current_cell: String::new(),
            saw_root: false,
            saw_columns: false,
            saw_records: false,
        }
    }

    fn open(
        &mut self,
        reader: &NsReader<&[u8]>,
        element: &BytesStart<'_>,
        empty: bool,
        is_merge: bool,
    ) -> Result<()> {
        let local = element.local_name();
        if !is_merge {
            return Err(merge_table_error(
                "mail-merge table element is in the wrong namespace",
            ));
        }
        let parent = self.stack.last().copied();
        let kind = match (parent, local.as_ref()) {
            (None, b"mergeData") if !self.saw_root => MergeParent::MergeData,
            (Some(MergeParent::MergeData), b"columns")
                if !self.saw_columns && !self.saw_records =>
            {
                MergeParent::Columns
            }
            (Some(MergeParent::Columns), b"column") => MergeParent::Columns,
            (Some(MergeParent::MergeData), b"records") if self.saw_columns && !self.saw_records => {
                MergeParent::Records
            }
            (Some(MergeParent::Records), b"record") => MergeParent::Record,
            (Some(MergeParent::Record), b"cell") => MergeParent::Cell,
            _ => {
                return Err(merge_table_error(
                    "mail-merge table element is not in the schema sequence",
                ));
            }
        };
        if local.as_ref() == b"column" {
            let name = merge_attribute_name(reader, element)?;
            if self.columns.iter().any(|existing| existing == &name) {
                return Err(merge_table_error("mail-merge table has a duplicate column"));
            }
            if self.columns.len() == MERGE_TABLE_COLUMN_CAP {
                return Err(merge_table_error("mail-merge table exceeds 1024 columns"));
            }
            self.columns.push(name);
            if !empty {
                self.stack.push(MergeParent::Column);
            }
            return Ok(());
        }
        reject_merge_xmlns_only(reader, element)?;
        if local.as_ref() == b"cell" {
            self.current_cell.clear();
            if empty {
                self.current_record.push(String::new());
            } else {
                self.stack.push(MergeParent::Cell);
            }
            return Ok(());
        }
        if empty {
            match kind {
                MergeParent::Columns => {
                    return Err(merge_table_error("mail-merge table has no columns"));
                }
                MergeParent::Records => {
                    self.saw_records = true;
                }
                MergeParent::MergeData => {
                    self.saw_root = true;
                }
                MergeParent::Record => {
                    if !self.columns.is_empty() {
                        return Err(merge_table_error("mail-merge table row is not rectangular"));
                    }
                    if self.records.len() == MERGE_TABLE_ROW_CAP {
                        return Err(merge_table_error("mail-merge table exceeds 100000 rows"));
                    }
                    self.records.push(Vec::new());
                }
                MergeParent::Cell | MergeParent::Column => {}
            }
            return Ok(());
        }
        if kind == MergeParent::Record {
            self.current_record.clear();
        }
        self.stack.push(kind);
        self.saw_root = true;
        Ok(())
    }

    fn close(&mut self, local: &[u8]) -> Result<()> {
        match self.stack.pop() {
            Some(MergeParent::Cell) if local == b"cell" => {
                self.current_record
                    .push(std::mem::take(&mut self.current_cell));
            }
            Some(MergeParent::Record) if local == b"record" => {
                if self.current_record.len() != self.columns.len() {
                    return Err(merge_table_error("mail-merge table row is not rectangular"));
                }
                if self.records.len() == MERGE_TABLE_ROW_CAP {
                    return Err(merge_table_error("mail-merge table exceeds 100000 rows"));
                }
                self.records.push(std::mem::take(&mut self.current_record));
            }
            Some(MergeParent::Columns) if local == b"columns" => {
                if self.columns.is_empty() {
                    return Err(merge_table_error("mail-merge table has no columns"));
                }
                self.saw_columns = true;
            }
            Some(MergeParent::Column) if local == b"column" => {}
            Some(MergeParent::Records) if local == b"records" => {
                self.saw_records = true;
            }
            Some(MergeParent::MergeData) if local == b"mergeData" => {
                if !self.saw_columns || !self.saw_records {
                    return Err(merge_table_error(
                        "mail-merge table element is not in the schema sequence",
                    ));
                }
            }
            _ => {
                return Err(merge_table_error(
                    "mail-merge table element is not in the schema sequence",
                ));
            }
        }
        Ok(())
    }

    fn append_text(&mut self, value: &str) -> Result<()> {
        if self.stack.last() == Some(&MergeParent::Cell) {
            append_merge_decoded(&mut self.current_cell, value)
        } else if !merge_whitespace(value) {
            Err(merge_table_error(
                "mail-merge table has text outside a cell",
            ))
        } else {
            Ok(())
        }
    }

    fn in_cell(&self) -> bool {
        self.stack.last() == Some(&MergeParent::Cell)
    }

    fn finish(self) -> Result<MailMergePackageSource> {
        if !self.saw_root || !self.stack.is_empty() || !self.saw_columns || !self.saw_records {
            return Err(merge_table_error(
                "mail-merge table element is not in the schema sequence",
            ));
        }
        Ok(MailMergePackageSource::Embedded {
            columns: self.columns,
            records: self.records,
        })
    }
}

fn reject_merge_xmlns_only(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| merge_table_error(format!("invalid mail-merge attribute: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let _ = reader.resolver().resolve_attribute(attribute.key);
        return Err(merge_table_error(
            "mail-merge table element has an unexpected attribute",
        ));
    }
    Ok(())
}

/// Decode one owned embedded mail-merge table into the package source carrier.
///
/// The encoded length is refused before the strict validator and the namespace
/// reader run. The result is the embedded source, not a second options model.
pub(super) fn decode_merge_table(xml: &[u8]) -> Result<MailMergePackageSource> {
    if xml.len() > MERGE_TABLE_XML_CAP {
        return Err(merge_table_error("mail-merge table exceeds 16 MiB"));
    }
    validate_strict_xml_1_0(xml).map_err(|error| {
        merge_table_error(format!("mail-merge table XML is not strict: {error:?}"))
    })?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_comments = true;
    let mut buffer = Vec::new();
    let mut parse = MergeTableParse::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| merge_table_error(format!("invalid mail-merge table XML: {error}")))?
            .into_owned();
        let after = reader.buffer_position() as usize;
        match event {
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::DocType(_) => {
                return Err(merge_table_error(
                    "mail-merge table XML cannot contain a document type",
                ));
            }
            Event::Start(element) => {
                let is_merge = {
                    let (namespace, _) = reader.resolver().resolve_element(element.name());
                    namespace_is_merge(&namespace)
                };
                parse.open(&reader, &element, false, is_merge)?;
            }
            Event::Empty(element) => {
                let is_merge = {
                    let (namespace, _) = reader.resolver().resolve_element(element.name());
                    namespace_is_merge(&namespace)
                };
                parse.open(&reader, &element, true, is_merge)?;
            }
            Event::End(element) => {
                parse.close(element.local_name().as_ref())?;
            }
            Event::Text(_) => {
                let raw = oxoffice_core::xml_text::normalize_line_ends(
                    std::str::from_utf8(&xml[before..after])
                        .map_err(|_| merge_table_error("mail-merge cell text is not UTF-8"))?,
                );
                let value = quick_xml::escape::unescape(&raw).map_err(|error| {
                    merge_table_error(format!("invalid mail-merge cell text: {error}"))
                })?;
                parse.append_text(&value)?;
            }
            Event::CData(text) => {
                if !parse.in_cell() {
                    return Err(merge_table_error(
                        "mail-merge table has text outside a cell",
                    ));
                }
                let value = text.decode().map_err(|error| {
                    merge_table_error(format!("invalid mail-merge cell text: {error}"))
                })?;
                parse.append_text(&oxoffice_core::xml_text::normalize_line_ends(&value))?;
            }
            Event::GeneralRef(_) => {
                if !parse.in_cell() {
                    return Err(merge_table_error(
                        "mail-merge table has a reference outside a cell",
                    ));
                }
                append_merge_reference(&mut parse.current_cell, &xml[before..after])?;
            }
            Event::Eof => break,
        }
        buffer.clear();
    }
    parse.finish()
}
