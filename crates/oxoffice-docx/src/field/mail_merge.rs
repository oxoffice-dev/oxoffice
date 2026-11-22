//! Mail merge: its public types and API, rich regions, and merge fields in headers and footers.

use super::*;

/// An image value embedded by a rich mail merge.
#[derive(Debug, Clone, PartialEq)]
pub struct MailMergeImage {
    pub data: Vec<u8>,
    pub filename: String,
    pub width: Length,
    pub height: Length,
}

/// A scalar value accepted by a rich mail merge field.
#[derive(Debug, Clone, PartialEq)]
pub enum MailMergeValue {
    Text(String),
    Image(MailMergeImage),
    Fragment(Vec<u8>),
}

/// One rich mail merge record with nested lexical regions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MailMergeRecord {
    pub values: BTreeMap<String, MailMergeValue>,
    pub regions: BTreeMap<String, Vec<MailMergeRecord>>,
}

/// Root records and reusable named sources for a rich mail merge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MailMergeData {
    pub records: Vec<MailMergeRecord>,
    pub sources: BTreeMap<String, Vec<MailMergeRecord>>,
}

/// Stable location metadata supplied to a rich merge formatter.
pub struct MailMergeFormatContext<'a> {
    pub source_name: &'a str,
    pub region_path: &'a [String],
    pub field_name: &'a str,
    pub record_number: u32,
    pub output_sequence_number: u32,
}

/// Text and optional run properties exposed to a rich merge formatter.
pub struct MailMergeFormattedText {
    pub text: String,
    pub run_properties: Option<CT_RPr>,
}

/// A validated mail-merge control decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailMergeControl {
    NextRecord { record_number: u32 },
    NextRecordIf { condition: bool, record_number: u32 },
    SkipRecordIf { condition: bool, record_number: u32 },
    RecordNumber(u32),
    SequenceNumber(u32),
}

/// Owned mail-merge rows or an external source declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailMergePackageSource {
    Embedded {
        columns: Vec<String>,
        records: Vec<Vec<String>>,
    },
    External {
        uri: String,
    },
}

/// One ODSO field-map slot supplied by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMergeFieldMapping {
    pub column: u32,
    pub source_name: String,
    pub mapped_name: String,
    pub language: Option<String>,
    pub dynamic_address: Option<bool>,
}

/// One recipient-data row. `unique_tag` is opaque identity bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMergeRecipient {
    pub column: u32,
    pub unique_tag: Vec<u8>,
    pub active: bool,
}

/// Input and successful read snapshot for one mail-merge package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMergePackageOptions {
    pub settings: crate::MailMerge,
    pub source: MailMergePackageSource,
    pub mappings: Vec<MailMergeFieldMapping>,
    pub recipients: Vec<MailMergeRecipient>,
}

/// Whether prepare may use caller rows. The default is offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MailMergeSourcePolicy {
    #[default]
    Offline,
    CallerSupplied,
}

/// Why prepare returned no records without fetching a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailMergePackageDiagnosticKind {
    ExternalSourceBlocked,
    ExternalSourceUnavailable,
    UnsupportedSource,
}

/// One prepare diagnostic. The message does not echo a source URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMergePackageDiagnostic {
    pub kind: MailMergePackageDiagnosticKind,
    pub message: String,
}

/// Column-keyed records and the diagnostics that explain an empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMergePreparedData {
    pub records: Vec<BTreeMap<String, String>>,
    pub diagnostics: Vec<MailMergePackageDiagnostic>,
}
impl Document {
    /// Write one mail-merge package.
    ///
    /// The embedded table, recipient part, and current ownership graph are
    /// validated before a candidate is published.
    pub fn set_mail_merge_package(&mut self, options: MailMergePackageOptions) -> Result<()> {
        require_authored_merge_settings(&options.settings)?;
        validate_merge_mappings(&options)?;
        let embedded_xml = match &options.source {
            MailMergePackageSource::Embedded { columns, records } => {
                Some(encode_merge_table(columns, records)?)
            }
            MailMergePackageSource::External { uri } => {
                validate_mail_merge_external_uri(uri)?;
                None
            }
        };
        self.validate_existing_merge_ownership()?;
        recipient_part_census(&self.package)?;
        let mut candidate = self.clone_for_staging();
        if let Some(xml) = embedded_xml.as_deref() {
            let existing = candidate.merge_store_doc_var()?;
            if let Some(item_id) = existing {
                let part = crate::content_control::resolve_custom_xml_part(
                    &candidate.package,
                    &candidate.doc_part_name,
                    &item_id,
                )?;
                require_merge_store_schema(&candidate.package, &part)?;
                let current = candidate.package.get_part(&part).unwrap_or_default();
                if current != xml {
                    if merge_store_has_other_owner(&candidate, &part, &item_id)? {
                        let new_id = candidate.add_custom_xml_store(xml, &[MERGE_DATA_NS])?;
                        candidate.set_document_variable(MERGE_STORE_DOC_VAR, &new_id)?;
                    } else {
                        candidate.package.set_part(&part, xml.to_vec());
                    }
                }
            } else if candidate.merge_settings_recognized()
                && !candidate.settings_declare_external_merge_source()?
            {
                return Err(Error::Other(
                    "mail-merge package source is missing".to_owned(),
                ));
            } else {
                let item_id = candidate.add_custom_xml_store(xml, &[MERGE_DATA_NS])?;
                candidate.set_document_variable(MERGE_STORE_DOC_VAR, &item_id)?;
            }
        }
        candidate.set_mail_merge_settings(options.settings.clone())?;
        if embedded_xml.is_none() {
            release_owned_merge_store(&mut candidate)?;
        }
        let recipient_id = install_recipient_part(&mut candidate, &options.recipients)?;
        install_merge_source_graph(&mut candidate, &options, recipient_id.as_deref())?;
        let mut reopened = candidate.prepare_and_reopen_staged()?;
        reopened.invalidate_layout();
        let mut baseline = self.clone_for_staging();
        if baseline.to_bytes()? == reopened.to_bytes()? {
            return Ok(());
        }
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Read the owned mail-merge projection.
    ///
    /// An unsupported active map or alternate recipient part is an error here.
    /// Prepare keeps that state on the private view and classifies it.
    pub fn mail_merge_package(&self) -> Result<Option<MailMergePackageOptions>> {
        Ok(match self.read_merge_view()? {
            None => None,
            Some(view) if view.unsupported_projection => {
                return Err(Error::Other(
                    "mail-merge package projection is unsupported".to_owned(),
                ));
            }
            Some(view) => Some(view.options),
        })
    }

    /// Remove one present mail-merge package.
    ///
    /// The owned graph is validated before any edit. An absent configuration
    /// returns false. A second call also returns false.
    pub fn remove_mail_merge_package(&mut self) -> Result<bool> {
        self.validate_existing_merge_ownership()?;
        if self.read_merge_view()?.is_none() {
            return Ok(false);
        }
        let mut candidate = self.clone_for_staging();
        release_owned_recipient(&mut candidate)?;
        release_owned_merge_store(&mut candidate)?;
        clear_mail_merge_markup(&mut candidate)?;
        let mut reopened = candidate.prepare_and_reopen_staged()?;
        reopened.invalidate_layout();
        let mut baseline = self.clone_for_staging();
        if baseline.to_bytes()? == reopened.to_bytes()? {
            return Ok(false);
        }
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// Prepare rows from the private mail-merge view.
    ///
    /// Offline caller rows and embedded caller rows are refused before a
    /// malformed store or an ambiguous graph is resolved. A valid external
    /// source is blocked while offline before it is classified as unsupported.
    pub fn prepare_mail_merge_data(
        &self,
        policy: MailMergeSourcePolicy,
        external_records: Option<&[BTreeMap<String, String>]>,
    ) -> Result<MailMergePreparedData> {
        if matches!(policy, MailMergeSourcePolicy::Offline) && external_records.is_some() {
            return Err(Error::Other(
                "mail-merge offline policy rejects caller rows".to_owned(),
            ));
        }
        if external_records.is_some() && self.merge_store_doc_var()?.is_some() {
            return Err(Error::Other(
                "mail-merge embedded table rejects caller rows".to_owned(),
            ));
        }
        let Some(view) = self.read_merge_view()? else {
            return Err(Error::Other("mail-merge package is absent".to_owned()));
        };
        let evaluation = merge_settings_need_evaluation(&view.options.settings);
        match view.options.source {
            MailMergePackageSource::Embedded { columns, records } => {
                if external_records.is_some() {
                    return Err(Error::Other(
                        "mail-merge embedded table rejects caller rows".to_owned(),
                    ));
                }
                if view.unsupported_projection
                    || view.recipient_blocked
                    || evaluation
                    || !embedded_mappings_are_faithful(&view.options.mappings, &columns)
                {
                    return Ok(merge_diagnostic(
                        MailMergePackageDiagnosticKind::UnsupportedSource,
                        "mail-merge source cannot be applied",
                    ));
                }
                let mut prepared = Vec::with_capacity(records.len());
                for record in records {
                    let mut row = BTreeMap::new();
                    for (column, cell) in columns.iter().zip(record) {
                        row.insert(column.clone(), cell);
                    }
                    prepared.push(row);
                }
                Ok(MailMergePreparedData {
                    records: prepared,
                    diagnostics: Vec::new(),
                })
            }
            MailMergePackageSource::External { uri } => {
                if matches!(policy, MailMergeSourcePolicy::Offline) {
                    return Ok(merge_diagnostic(
                        MailMergePackageDiagnosticKind::ExternalSourceBlocked,
                        "mail-merge external source is blocked",
                    ));
                }
                let identity = identity_mappings(&view.options.mappings);
                let unsupported = view.sources_differ
                    || view.recipient_blocked
                    || view.unsupported_projection
                    || evaluation
                    || !merge_uri_scheme_is_file_or_https(&uri)
                    || !(view.options.mappings.is_empty() || identity);
                if unsupported {
                    return Ok(merge_diagnostic(
                        MailMergePackageDiagnosticKind::UnsupportedSource,
                        "mail-merge source cannot be applied",
                    ));
                }
                let Some(rows) = external_records else {
                    return Ok(merge_diagnostic(
                        MailMergePackageDiagnosticKind::ExternalSourceUnavailable,
                        "mail-merge external source is unavailable",
                    ));
                };
                if identity
                    && rows.iter().any(|row| {
                        view.options
                            .mappings
                            .iter()
                            .any(|mapping| !row.contains_key(&mapping.source_name))
                    })
                {
                    return Ok(merge_diagnostic(
                        MailMergePackageDiagnosticKind::UnsupportedSource,
                        "mail-merge source cannot be applied",
                    ));
                }
                Ok(MailMergePreparedData {
                    records: rows.to_vec(),
                    diagnostics: Vec::new(),
                })
            }
        }
    }

    fn read_merge_view(&self) -> Result<Option<MergePackageView>> {
        self.reject_unproved_settings_owner()?;
        let graph = self
            .settings
            .as_ref()
            .map(|settings| inspect_merge_xml(&settings.to_xml()?))
            .transpose()?
            .map(|inspection| inspection.graph)
            .unwrap_or_default();
        let item_id = self.merge_store_doc_var()?;
        let recipients = if graph.data_source.is_some()
            || graph.odso_source.is_some()
            || graph.recipient_id.is_some()
            || item_id.is_some()
        {
            Some(classify_recipients(self, graph.recipient_id.as_deref())?)
        } else {
            None
        };
        let (loaded_recipients, recipient_alternate) = match &recipients {
            Some(RecipientClass::Standard { part, .. }) => (
                decode_recipients(self.package.get_part(part).unwrap_or_default())?,
                false,
            ),
            Some(RecipientClass::Alternate) => (Vec::new(), true),
            _ => (Vec::new(), false),
        };
        let recipient_blocked =
            graph.recipient_graph || recipient_alternate || !loaded_recipients.is_empty();
        let unsupported_projection = recipient_alternate || graph.unsupported_field_map;
        let recognized = merge_configuration_recognized(&graph);
        let external = match (graph.data_source, graph.odso_source) {
            (Some(data_source), Some(odso_source)) => {
                let data_uri = resolve_settings_source_target(self, &data_source)?;
                let odso_uri = resolve_settings_source_target(self, &odso_source)?;
                let differ = data_uri != odso_uri;
                let active = if self
                    .mail_merge_settings()
                    .and_then(|settings| settings.data_type.as_deref())
                    == Some("native")
                {
                    odso_uri
                } else {
                    data_uri
                };
                Some((active, differ))
            }
            (Some(data_source), None) => {
                Some((resolve_settings_source_target(self, &data_source)?, false))
            }
            (None, Some(odso_source)) => {
                Some((resolve_settings_source_target(self, &odso_source)?, false))
            }
            (None, None) => None,
        };
        if item_id.is_some() && external.is_some() {
            return Err(Error::Other(
                "mail-merge package ownership is ambiguous".to_owned(),
            ));
        }
        let settings = self.mail_merge_settings().cloned();
        if let Some(item_id) = item_id {
            let part = crate::content_control::resolve_custom_xml_part(
                &self.package,
                &self.doc_part_name,
                &item_id,
            )?;
            require_merge_store_schema(&self.package, &part)?;
            let xml = self
                .package
                .get_part(&part)
                .ok_or_else(|| Error::Other("mail-merge package source is missing".to_owned()))?;
            let MailMergePackageSource::Embedded { columns, records } = decode_merge_table(xml)?
            else {
                return Err(Error::Other(
                    "mail-merge package source is not an embedded table".to_owned(),
                ));
            };
            let settings = settings
                .ok_or_else(|| Error::Other("mail-merge package source is missing".to_owned()))?;
            self.require_active_header(graph.header_id.as_deref())?;
            return Ok(Some(MergePackageView {
                options: MailMergePackageOptions {
                    settings,
                    source: MailMergePackageSource::Embedded { columns, records },
                    mappings: graph.mappings,
                    recipients: loaded_recipients.clone(),
                },
                sources_differ: false,
                recipient_blocked,
                unsupported_projection,
            }));
        }
        if let Some((uri, sources_differ)) = external {
            let settings = settings
                .ok_or_else(|| Error::Other("mail-merge package source is missing".to_owned()))?;
            self.require_active_header(graph.header_id.as_deref())?;
            return Ok(Some(MergePackageView {
                options: MailMergePackageOptions {
                    settings,
                    source: MailMergePackageSource::External { uri },
                    mappings: graph.mappings,
                    recipients: loaded_recipients,
                },
                sources_differ,
                recipient_blocked,
                unsupported_projection,
            }));
        }
        if settings.as_ref().is_some_and(mail_merge_has_modeled_value) || recognized {
            return Err(Error::Other(
                "mail-merge package source is missing".to_owned(),
            ));
        }
        Ok(None)
    }

    fn require_active_header(&self, header_id: Option<&str>) -> Result<()> {
        let Some(header_id) = header_id else {
            return Ok(());
        };
        resolve_settings_source_target(self, header_id).map_err(|_| {
            Error::Other("mail-merge header source reference cannot be proved".to_owned())
        })?;
        Ok(())
    }

    fn validate_existing_merge_ownership(&self) -> Result<()> {
        self.reject_unproved_settings_owner()?;
        let Some(settings) = &self.settings else {
            return Ok(());
        };
        let inspection = inspect_merge_xml(&settings.to_xml()?)?;
        if inspection.graph.unsafe_field_map {
            return Err(Error::Other(
                "mail-merge field map cannot preserve an unknown assignment".to_owned(),
            ));
        }
        if inspection.graph.unsafe_source {
            return Err(Error::Other(
                "mail-merge source reference cannot be proved".to_owned(),
            ));
        }
        if let Some(id) = inspection.graph.data_source.as_deref() {
            resolve_settings_source_target(self, id)?;
        }
        if let Some(id) = inspection.graph.odso_source.as_deref() {
            resolve_settings_source_target(self, id)?;
        }
        if let Some(id) = inspection.graph.header_id.as_deref() {
            resolve_settings_source_target(self, id)?;
        }
        if inspection.graph.recipient_id.is_some() {
            classify_recipients(self, inspection.graph.recipient_id.as_deref())?;
        }
        let has_external =
            inspection.graph.data_source.is_some() || inspection.graph.odso_source.is_some();
        let Some(item_id) = self.merge_store_doc_var()? else {
            return Ok(());
        };
        match crate::content_control::resolve_custom_xml_part(
            &self.package,
            &self.doc_part_name,
            &item_id,
        ) {
            Ok(part) => {
                require_merge_store_schema(&self.package, &part)?;
                let xml = self.package.get_part(&part).ok_or_else(|| {
                    Error::Other("mail-merge package source is missing".to_owned())
                })?;
                decode_merge_table(xml)?;
            }
            Err(Error::Other(message)) if message.contains("was not found") => {
                if has_external || self.merge_settings_recognized() {
                    return Err(Error::Other(
                        "mail-merge package ownership is ambiguous".to_owned(),
                    ));
                }
                return Err(Error::Other(
                    "mail-merge store document variable is stale".to_owned(),
                ));
            }
            Err(error) => return Err(error),
        }
        if has_external {
            return Err(Error::Other(
                "mail-merge package ownership is ambiguous".to_owned(),
            ));
        }
        Ok(())
    }

    fn merge_settings_recognized(&self) -> bool {
        self.mail_merge_settings()
            .is_some_and(mail_merge_has_modeled_value)
    }

    pub(super) fn merge_store_doc_var(&self) -> Result<Option<String>> {
        let Some(settings) = &self.settings else {
            return Ok(None);
        };
        let mut values = settings
            .document_variables()
            .iter()
            .filter(|variable| variable.name == MERGE_STORE_DOC_VAR);
        let Some(first) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            return Err(Error::Other(
                "mail-merge store document variable is duplicated".to_owned(),
            ));
        }
        Ok(Some(first.value.clone()))
    }

    fn settings_declare_external_merge_source(&self) -> Result<bool> {
        let Some(settings) = &self.settings else {
            return Ok(false);
        };
        let graph = inspect_merge_xml(&settings.to_xml()?)?.graph;
        if let Some(id) = [graph.data_source, graph.odso_source]
            .into_iter()
            .flatten()
            .next()
        {
            resolve_settings_source_target(self, &id)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn reject_unproved_settings_owner(&self) -> Result<()> {
        let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) else {
            return Ok(());
        };
        let settings: Vec<_> = relationships
            .items
            .iter()
            .filter(|item| item.rel_type == rel_types::SETTINGS)
            .collect();
        if settings.is_empty() {
            return Ok(());
        }
        let internal = settings
            .iter()
            .filter(|item| crate::document::relationship_is_internal(item))
            .count();
        let mut recognized = Vec::new();
        for item in &settings {
            let target = OpcPackage::resolve_rel_target(&self.doc_part_name, &item.target);
            let is_owned = match self.package.get_part(&target) {
                Some(xml) => merge_configuration_recognized(&inspect_merge_xml(xml)?.graph),
                None => false,
            };
            recognized.push((crate::document::relationship_is_internal(item), is_owned));
        }
        let non_internal_owned = recognized
            .iter()
            .any(|(is_internal, is_owned)| !is_internal && *is_owned);
        let any_owned = recognized.iter().any(|(_, is_owned)| *is_owned);
        if non_internal_owned || (internal != 1 && any_owned) {
            return Err(Error::Other(
                "mail-merge settings owner is ambiguous".to_owned(),
            ));
        }
        Ok(())
    }

    /// Materialize one independent document for each flat mail-merge record.
    pub fn mail_merge(&self, records: &[BTreeMap<String, String>]) -> Result<Vec<Document>> {
        if records.is_empty() {
            return Err(Error::Other(
                "mail merge requires at least one record".to_owned(),
            ));
        }

        let mut outputs = Vec::with_capacity(records.len());
        for (record_index, record) in records.iter().enumerate() {
            let record_number = u32::try_from(record_index)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| Error::Other("mail merge record count exceeds u32".to_owned()))?;
            let context = FieldEvaluationContext {
                merge_fields: record.clone(),
                merge_record_number: Some(record_number),
                merge_sequence_number: Some(record_number),
                ..Default::default()
            };
            let mut candidate = self.clone_for_staging();
            candidate.update_fields_with_policy(&context, true)?;
            outputs.push(candidate.prepare_and_reopen_staged()?);
        }
        Ok(outputs)
    }

    /// Materialize one document whose record bodies form next-page sections.
    pub fn mail_merge_sections(&self, records: &[BTreeMap<String, String>]) -> Result<Document> {
        reject_varying_non_body_merge_fields(self, records)?;
        let mut candidates = self.mail_merge(records)?;

        let mut identity_state = BodyIdentityState::from_documents(&candidates)?;
        let mut identifiers = candidates
            .first()
            .ok_or_else(|| Error::Other("mail merge requires at least one record".to_owned()))?
            .identifiers
            .clone();
        for candidate in candidates.iter_mut().skip(1) {
            remap_body_identities(candidate, &mut identifiers, &mut identity_state)?;
        }
        combine_mail_merge_sections(candidates)
    }

    /// Materialize independent documents from nested, typed mail merge data.
    #[allow(clippy::type_complexity)]
    pub fn mail_merge_rich(
        &self,
        data: &MailMergeData,
        mut formatter: Option<
            &mut dyn FnMut(&MailMergeFormatContext<'_>, &mut MailMergeFormattedText) -> Result<()>,
        >,
    ) -> Result<Vec<Document>> {
        if data.records.is_empty() {
            return Err(Error::Other(
                "rich mail merge requires at least one record".to_owned(),
            ));
        }

        let mut outputs = Vec::with_capacity(data.records.len());
        for (record_index, record) in data.records.iter().enumerate() {
            let record_number = one_based_record_number(record_index)?;
            let mut candidate = self.clone_for_staging();
            let mut identity_state =
                BodyIdentityState::from_documents(std::slice::from_ref(&candidate))?;
            let template = std::mem::take(&mut candidate.document.body.content);
            let mut region_path = Vec::new();
            let scopes = vec![record];
            let mut output_sequence_number = 0;
            candidate.document.body.content = expand_rich_body(
                &mut candidate,
                &template,
                data,
                &scopes,
                "records",
                &mut region_path,
                record_number,
                &mut output_sequence_number,
                &mut identity_state,
                &mut formatter,
            )?;
            candidate.invalidate_layout();
            outputs.push(candidate.prepare_and_reopen_staged()?);
        }
        Ok(outputs)
    }

    /// Materialize rich merge records as next-page sections in one document.
    #[allow(clippy::type_complexity)]
    pub fn mail_merge_sections_rich(
        &self,
        data: &MailMergeData,
        formatter: Option<
            &mut dyn FnMut(&MailMergeFormatContext<'_>, &mut MailMergeFormattedText) -> Result<()>,
        >,
    ) -> Result<Document> {
        let mut candidates = self.mail_merge_rich(data, formatter)?;
        let mut identity_state = BodyIdentityState::from_documents(&candidates)?;
        let mut identifiers = candidates
            .first()
            .ok_or_else(|| Error::Other("rich mail merge requires at least one record".to_owned()))?
            .identifiers
            .clone();
        for candidate in candidates.iter_mut().skip(1) {
            remap_body_identities(candidate, &mut identifiers, &mut identity_state)?;
        }
        combine_mail_merge_sections(candidates)
    }
}

fn one_based_record_number(index: usize) -> Result<u32> {
    u32::try_from(index)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| Error::Other("rich mail merge record count exceeds u32".to_owned()))
}

fn combine_mail_merge_sections(mut candidates: Vec<Document>) -> Result<Document> {
    let bodies = candidates
        .iter()
        .map(|candidate| candidate.document.body.clone())
        .collect::<Vec<_>>();
    let mut combined = candidates.remove(0);
    combined.document.body.content.clear();
    combined.document.body.sect_pr = None;
    let final_index = bodies.len() - 1;
    for (index, mut body) in bodies.into_iter().enumerate() {
        combined.document.body.content.append(&mut body.content);
        if index == final_index {
            combined.document.body.sect_pr = body.sect_pr;
        } else {
            let mut section = body.sect_pr.unwrap_or_else(empty_section_properties);
            section.section_type = Some(ST_SectionType::NextPage);
            let mut paragraph = CT_P::new();
            paragraph.properties = Some(CT_PPr {
                sect_pr: Some(section),
                ..Default::default()
            });
            combined
                .document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
        }
    }
    combined.invalidate_layout();
    combined.prepare_and_reopen_staged()
}

type RichFormatter<'a> = Option<
    &'a mut dyn FnMut(&MailMergeFormatContext<'_>, &mut MailMergeFormattedText) -> Result<()>,
>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RichRegionMarker {
    Start(String),
    End(String),
}

struct RichExpandedRow {
    source_index: usize,
    row: CT_Row,
}

const RICH_MERGE_REGION_DEPTH_LIMIT: usize = 32;

fn merge_field_name(field: &Field) -> Option<String> {
    let instruction = field.effective_instruction();
    if instruction.name != "MERGEFIELD" {
        return None;
    }
    instruction
        .arguments
        .first()
        .and_then(argument_text)
        .map(str::to_owned)
}

fn rich_region_marker(paragraph: &CT_P) -> Option<RichRegionMarker> {
    if paragraph_has_raw_content(paragraph)
        || paragraph.properties.is_some()
        || !paragraph.content_controls.is_empty()
        || !paragraph.revisions.is_empty()
        || !paragraph.hyperlinks.is_empty()
        || !paragraph.comment_ranges.is_empty()
        || !paragraph.bookmark_markers.is_empty()
        || !paragraph.equations.is_empty()
        || paragraph.runs.len() != 1
    {
        return None;
    }
    let run = &paragraph.runs[0];
    if run
        .extra_xml
        .iter()
        .any(|raw| !raw.iter().all(u8::is_ascii_whitespace))
        || !run.alt_drawings.is_empty()
    {
        return None;
    }
    let [RunContent::Field(field)] = run.content.as_slice() else {
        return None;
    };
    let instruction = field.effective_instruction();
    if instruction.name != "MERGEFIELD"
        || instruction.arguments.len() != 1
        || !instruction.switches.is_empty()
    {
        return None;
    }
    let name = merge_field_name(field)?;
    name.strip_prefix("TableStart:")
        .map(|name| RichRegionMarker::Start(name.to_owned()))
        .or_else(|| {
            name.strip_prefix("TableEnd:")
                .map(|name| RichRegionMarker::End(name.to_owned()))
        })
}

/// Whether a paragraph holds raw XML other than whitespace. The attributes
/// of its start tag, such as `w:rsidR` or `w14:paraId`, are not content.
fn paragraph_has_raw_content(paragraph: &CT_P) -> bool {
    paragraph.extra_xml.iter().any(|(position, raw)| {
        !CT_P::raw_is_root_attributes(*position, raw) && !raw.iter().all(u8::is_ascii_whitespace)
    })
}

fn find_body_region_end(items: &[BodyContent], start: usize, name: &str) -> Result<usize> {
    let mut stack = vec![name.to_owned()];
    for (index, item) in items.iter().enumerate().skip(start) {
        let BodyContent::Paragraph(paragraph) = item else {
            continue;
        };
        match rich_region_marker(paragraph) {
            Some(RichRegionMarker::Start(nested)) => stack.push(nested),
            Some(RichRegionMarker::End(end)) => {
                if stack.last() != Some(&end) {
                    return Err(Error::Other(format!(
                        "crossed rich mail merge region marker TableEnd:{end}"
                    )));
                }
                stack.pop();
                if stack.is_empty() {
                    return Ok(index);
                }
            }
            None => {}
        }
    }
    Err(Error::Other(format!(
        "rich mail merge region TableStart:{name} has no matching end"
    )))
}

#[allow(clippy::too_many_arguments)]
fn expand_rich_body(
    document: &mut Document,
    items: &[BodyContent],
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &mut Vec<String>,
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<Vec<BodyContent>> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < items.len() {
        if let BodyContent::Paragraph(paragraph) = &items[index]
            && let Some(marker) = rich_region_marker(paragraph)
        {
            match marker {
                RichRegionMarker::Start(name) => {
                    if region_path.len() >= RICH_MERGE_REGION_DEPTH_LIMIT {
                        return Err(Error::Other(format!(
                            "rich mail merge region depth exceeds {RICH_MERGE_REGION_DEPTH_LIMIT}"
                        )));
                    }
                    let end = find_body_region_end(items, index + 1, &name)?;
                    let records = resolve_region_records(scopes, data, &name)?;
                    region_path.push(name.clone());
                    for (child_index, child) in records.iter().enumerate() {
                        let mut child_scopes = scopes.to_vec();
                        child_scopes.push(child);
                        let mut child_content = expand_rich_body(
                            document,
                            &items[index + 1..end],
                            data,
                            &child_scopes,
                            &name,
                            region_path,
                            one_based_record_number(child_index)?,
                            output_sequence_number,
                            identity_state,
                            formatter,
                        )?;
                        remap_rich_body_content(document, &mut child_content, identity_state)?;
                        output.extend(child_content);
                    }
                    region_path.pop();
                    index = end + 1;
                    continue;
                }
                RichRegionMarker::End(name) => {
                    return Err(Error::Other(format!(
                        "unexpected rich mail merge region marker TableEnd:{name}"
                    )));
                }
            }
        }
        if let BodyContent::Paragraph(paragraph) = &items[index]
            && let Some(fragment) = whole_paragraph_fragment(paragraph, scopes)?
        {
            output.extend(import_rich_fragment(document, fragment, identity_state)?);
            index += 1;
            continue;
        }
        let mut item = items[index].clone();
        replace_rich_body_item(
            document,
            &mut item,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        )?;
        output.push(item);
        index += 1;
    }
    Ok(output)
}

fn whole_paragraph_fragment<'a>(
    paragraph: &CT_P,
    scopes: &[&'a MailMergeRecord],
) -> Result<Option<&'a [u8]>> {
    if paragraph_has_raw_content(paragraph)
        || !paragraph.content_controls.is_empty()
        || !paragraph.revisions.is_empty()
        || !paragraph.hyperlinks.is_empty()
        || !paragraph.comment_ranges.is_empty()
        || !paragraph.bookmark_markers.is_empty()
        || !paragraph.equations.is_empty()
        || paragraph.runs.len() != 1
    {
        return Ok(None);
    }
    let [RunContent::Field(field)] = paragraph.runs[0].content.as_slice() else {
        return Ok(None);
    };
    let Some(name) = merge_field_name(field) else {
        return Ok(None);
    };
    match resolve_rich_value(scopes, &name) {
        Some(MailMergeValue::Fragment(bytes)) => Ok(Some(bytes)),
        _ => Ok(None),
    }
}

fn import_rich_fragment(
    document: &mut Document,
    bytes: &[u8],
    identity_state: &mut BodyIdentityState,
) -> Result<Vec<BodyContent>> {
    import_fragment_content_with_state(
        document,
        bytes,
        false,
        false,
        FragmentConflictPolicy::rename_all().with_style_reuse(true),
        &document.doc_part_name.clone(),
        identity_state,
        false,
        false,
        &mut Vec::new(),
        None,
    )
    .map(|imported| imported.typed)
}

fn remap_rich_body_content(
    document: &mut Document,
    content: &mut Vec<BodyContent>,
    identity_state: &mut BodyIdentityState,
) -> Result<()> {
    let mut occurrence = document.clone_for_staging();
    occurrence.document.body.content = std::mem::take(content);
    occurrence.document.body.sect_pr = None;
    remap_body_identities(&mut occurrence, &mut document.identifiers, identity_state)?;
    *content = occurrence.document.into_inner().body.content;
    Ok(())
}

fn remap_rich_rows(
    document: &mut Document,
    rows: &mut [RichExpandedRow],
    identity_state: &mut BodyIdentityState,
) -> Result<()> {
    let mut table = CT_Tbl::new();
    table.rows = rows
        .iter_mut()
        .map(|item| std::mem::take(&mut item.row))
        .collect();
    let mut content = vec![BodyContent::Table(Box::new(table))];
    remap_rich_body_content(document, &mut content, identity_state)?;
    let Some(BodyContent::Table(table)) = content.pop() else {
        return Err(Error::Other(
            "rich mail merge row identity staging lost its table".to_owned(),
        ));
    };
    for (item, row) in rows.iter_mut().zip(table.rows) {
        item.row = row;
    }
    Ok(())
}

fn resolve_region_records<'a>(
    scopes: &[&'a MailMergeRecord],
    data: &'a MailMergeData,
    name: &str,
) -> Result<&'a [MailMergeRecord]> {
    if let Some(records) = scopes.last().and_then(|record| record.regions.get(name)) {
        return Ok(records);
    }
    data.sources
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| Error::Other(format!("rich mail merge source {name} is missing")))
}

fn resolve_rich_value<'a>(
    scopes: &[&'a MailMergeRecord],
    name: &str,
) -> Option<&'a MailMergeValue> {
    scopes
        .iter()
        .rev()
        .find_map(|record| record.values.get(name))
}

pub(super) fn resolve_mergefield_text(
    instruction: &FieldInstruction,
    value: Option<&str>,
    missing_as_empty: bool,
) -> std::result::Result<String, String> {
    if let Some(name) = unsupported_switch(instruction) {
        return Err(format!("field MERGEFIELD uses unsupported switch \\{name}"));
    }
    validate_instruction_shape(instruction)?;
    let name = text_argument(instruction, 0)
        .ok_or_else(|| "MERGEFIELD requires a field name".to_owned())?;
    let Some(value) = value else {
        return if missing_as_empty {
            Ok(String::new())
        } else {
            Err(format!("MERGEFIELD input {name} was not supplied"))
        };
    };
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut result = String::new();
    if let Some(prefix) = switch_text(instruction, "b") {
        result.push_str(prefix);
    }
    result.push_str(value);
    if let Some(suffix) = switch_text(instruction, "f") {
        result.push_str(suffix);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_body_item(
    document: &mut Document,
    item: &mut BodyContent,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    match item {
        BodyContent::Paragraph(paragraph) => replace_rich_paragraph(
            document,
            paragraph,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        ),
        BodyContent::Table(table) => replace_rich_table(
            document,
            table,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        ),
        BodyContent::ContentControl(control) => replace_rich_control(
            document,
            control,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        ),
        BodyContent::RawXml(_) => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_paragraph(
    document: &mut Document,
    paragraph: &mut CT_P,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    for run in &mut paragraph.runs {
        replace_rich_run(
            document,
            run,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            formatter,
        )?;
    }
    for (_, _, _, control) in &mut paragraph.content_controls {
        replace_rich_control(
            document,
            control,
            &MailMergeData::default(),
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_run(
    document: &mut Document,
    run: &mut CT_R,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    for content in &mut run.content {
        let RunContent::Field(field) = content else {
            continue;
        };
        let Some(field_name) = merge_field_name(field) else {
            continue;
        };
        if field_name.starts_with("TableStart:") || field_name.starts_with("TableEnd:") {
            return Err(Error::Other(format!(
                "rich mail merge region field {field_name} must own its whole block"
            )));
        }
        *output_sequence_number = output_sequence_number.checked_add(1).ok_or_else(|| {
            Error::Other("rich mail merge output sequence exceeds u32".to_owned())
        })?;
        let merge_value = resolve_rich_value(scopes, &field_name);
        if matches!(merge_value, Some(MailMergeValue::Text(_)) | None) {
            let value = match merge_value {
                Some(MailMergeValue::Text(value)) => Some(value.as_str()),
                None => None,
                _ => unreachable!(),
            };
            let instruction = field.effective_instruction();
            let resolved = resolve_mergefield_text(&instruction, value, true)
                .and_then(|value| apply_formats(&instruction, &value, None))
                .map_err(Error::Other)?;
            let mut formatted = MailMergeFormattedText {
                text: resolved,
                run_properties: run.properties.clone(),
            };
            if let Some(callback) = formatter.as_deref_mut() {
                callback(
                    &MailMergeFormatContext {
                        source_name,
                        region_path,
                        field_name: &field_name,
                        record_number,
                        output_sequence_number: *output_sequence_number,
                    },
                    &mut formatted,
                )?;
            }
            if let Some(character) = formatted
                .text
                .chars()
                .find(|character| !valid_xml_character(*character))
            {
                return Err(Error::Other(format!(
                    "rich mail merge field {field_name} contains invalid XML character U+{:04X}",
                    u32::from(character)
                )));
            }
            run.properties = formatted.run_properties;
            *content = RunContent::Text(CT_Text::new(&formatted.text));
            continue;
        }
        match merge_value {
            Some(MailMergeValue::Image(image)) => {
                if image.width.to_emu() <= 0 || image.height.to_emu() <= 0 {
                    return Err(Error::Other(format!(
                        "rich mail merge image {field_name} has non-positive dimensions"
                    )));
                }
                let rel_id = document.embed_image(&image.data, &image.filename);
                let mut inline =
                    CT_Inline::new(&rel_id, image.width.to_emu(), image.height.to_emu());
                inline.doc_pr_id = document.identifiers.reserve_drawing_id()?;
                *content = RunContent::Drawing(CT_Drawing::inline(inline));
            }
            Some(MailMergeValue::Fragment(_)) => {
                return Err(Error::Other(format!(
                    "rich mail merge fragment field {field_name} must own its whole paragraph"
                )));
            }
            Some(MailMergeValue::Text(_)) | None => unreachable!(),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_table(
    document: &mut Document,
    table: &mut CT_Tbl,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    let template_rows = std::mem::take(&mut table.rows);
    for (index, row) in template_rows.iter().enumerate() {
        if rich_row_region_marker(row).is_some()
            && (table.extra_xml.iter().any(|(at, _)| *at == index)
                || table.content_controls.iter().any(|(at, _, _)| *at == index))
        {
            return Err(Error::Other(
                "rich mail merge row marker cannot share a table boundary with preserved content"
                    .to_owned(),
            ));
        }
    }
    let expanded = expand_rich_rows(
        document,
        &template_rows,
        0,
        data,
        scopes,
        source_name,
        region_path,
        record_number,
        output_sequence_number,
        identity_state,
        formatter,
    )?;
    let old_extra = std::mem::take(&mut table.extra_xml);
    let old_controls = std::mem::take(&mut table.content_controls);
    for (output_index, expanded_row) in expanded.iter().enumerate() {
        table.extra_xml.extend(
            old_extra
                .iter()
                .filter(|(at, _)| *at == expanded_row.source_index)
                .map(|(_, raw)| (output_index, raw.clone())),
        );
        table.content_controls.extend(
            old_controls
                .iter()
                .filter(|(at, _, _)| *at == expanded_row.source_index)
                .map(|(_, raw_before, control)| (output_index, *raw_before, control.clone())),
        );
    }
    let output_end = expanded.len();
    table.extra_xml.extend(
        old_extra
            .iter()
            .filter(|(at, _)| *at == template_rows.len())
            .map(|(_, raw)| (output_end, raw.clone())),
    );
    table.content_controls.extend(
        old_controls
            .iter()
            .filter(|(at, _, _)| *at == template_rows.len())
            .map(|(_, raw_before, control)| (output_end, *raw_before, control.clone())),
    );
    table.rows = expanded.into_iter().map(|item| item.row).collect();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn expand_rich_rows(
    document: &mut Document,
    template_rows: &[CT_Row],
    source_offset: usize,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<Vec<RichExpandedRow>> {
    let mut output = Vec::new();
    let mut index = 0;
    while index < template_rows.len() {
        if let Some(RichRegionMarker::Start(name)) = rich_row_region_marker(&template_rows[index]) {
            if region_path.len() >= RICH_MERGE_REGION_DEPTH_LIMIT {
                return Err(Error::Other(format!(
                    "rich mail merge row region depth exceeds {RICH_MERGE_REGION_DEPTH_LIMIT}"
                )));
            }
            let end = find_row_region_end(template_rows, index + 1, &name)?;
            let records = resolve_region_records(scopes, data, &name)?;
            for (child_index, child) in records.iter().enumerate() {
                let mut child_scopes = scopes.to_vec();
                child_scopes.push(child);
                let mut child_path = region_path.to_vec();
                child_path.push(name.clone());
                let mut child_rows = expand_rich_rows(
                    document,
                    &template_rows[index + 1..end],
                    source_offset + index + 1,
                    data,
                    &child_scopes,
                    &name,
                    &child_path,
                    one_based_record_number(child_index)?,
                    output_sequence_number,
                    identity_state,
                    formatter,
                )?;
                remap_rich_rows(document, &mut child_rows, identity_state)?;
                output.extend(child_rows);
            }
            index = end + 1;
            continue;
        }
        if let Some(RichRegionMarker::End(name)) = rich_row_region_marker(&template_rows[index]) {
            return Err(Error::Other(format!(
                "unexpected rich mail merge row marker TableEnd:{name}"
            )));
        }
        let mut row = template_rows[index].clone();
        replace_rich_row(
            document,
            &mut row,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        )?;
        output.push(RichExpandedRow {
            source_index: source_offset + index,
            row,
        });
        index += 1;
    }
    Ok(output)
}

fn rich_row_region_marker(row: &CT_Row) -> Option<RichRegionMarker> {
    if row
        .extra_xml
        .iter()
        .any(|(position, raw)| !CT_Row::raw_is_root_attributes(*position, raw))
        || !row.content_controls.is_empty()
        || row.cells.len() != 1
    {
        return None;
    }
    let cell = &row.cells[0];
    if !cell.extra_xml.is_empty() || cell.content.len() != 1 {
        return None;
    }
    let CellContent::Paragraph(paragraph) = &cell.content[0] else {
        return None;
    };
    rich_region_marker(paragraph)
}

fn find_row_region_end(rows: &[CT_Row], start: usize, name: &str) -> Result<usize> {
    let mut stack = vec![name.to_owned()];
    for (index, row) in rows.iter().enumerate().skip(start) {
        match rich_row_region_marker(row) {
            Some(RichRegionMarker::Start(nested)) => stack.push(nested),
            Some(RichRegionMarker::End(end)) => {
                if stack.last() != Some(&end) {
                    return Err(Error::Other(format!(
                        "crossed rich mail merge row marker TableEnd:{end}"
                    )));
                }
                stack.pop();
                if stack.is_empty() {
                    return Ok(index);
                }
            }
            None => {}
        }
    }
    Err(Error::Other(format!(
        "rich mail merge row region TableStart:{name} has no matching end"
    )))
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_row(
    document: &mut Document,
    row: &mut CT_Row,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    for cell in &mut row.cells {
        replace_rich_cell(
            document,
            cell,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        )?;
    }
    for (_, _, control) in &mut row.content_controls {
        replace_rich_control(
            document,
            control,
            data,
            scopes,
            source_name,
            region_path,
            record_number,
            output_sequence_number,
            identity_state,
            formatter,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_cell(
    document: &mut Document,
    cell: &mut CT_Tc,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    for content in &mut cell.content {
        match content {
            CellContent::Paragraph(paragraph) => replace_rich_paragraph(
                document,
                paragraph,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            CellContent::Table(table) => replace_rich_table(
                document,
                table,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            CellContent::ContentControl(control) => replace_rich_control(
                document,
                control,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn replace_rich_control(
    document: &mut Document,
    control: &mut CT_Sdt,
    data: &MailMergeData,
    scopes: &[&MailMergeRecord],
    source_name: &str,
    region_path: &[String],
    record_number: u32,
    output_sequence_number: &mut u32,
    identity_state: &mut BodyIdentityState,
    formatter: &mut RichFormatter<'_>,
) -> Result<()> {
    for content in &mut control.content {
        match content {
            SdtContent::Paragraph(paragraph) => replace_rich_paragraph(
                document,
                paragraph,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            SdtContent::Table(table) => replace_rich_table(
                document,
                table,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            SdtContent::Row(row) => replace_rich_row(
                document,
                row,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            SdtContent::Cell(cell) => replace_rich_cell(
                document,
                cell,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            SdtContent::Run(run) => replace_rich_run(
                document,
                run,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                formatter,
            )?,
            SdtContent::ContentControl(nested) => replace_rich_control(
                document,
                nested,
                data,
                scopes,
                source_name,
                region_path,
                record_number,
                output_sequence_number,
                identity_state,
                formatter,
            )?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

pub(super) fn merge_referenced_header_footer_parts(
    document: &Document,
) -> Result<Vec<(String, Vec<u8>)>> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Ok(Vec::new());
    };
    let xml = document.document.to_xml()?;
    let mut reader = NsReader::from_reader(xml.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut references = Vec::<(String, bool)>::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid main document XML: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element)
                if namespace_is_word(&reader.resolver().resolve_element(element.name()).0) =>
            {
                let name = element.name();
                let local = local_name(name.as_ref());
                let is_header = if local == b"headerReference" {
                    true
                } else if local == b"footerReference" {
                    false
                } else {
                    buffer.clear();
                    continue;
                };
                if let Some((_, rel_id)) = resolved_element_attribute(
                    &element,
                    reader.resolver(),
                    b"id",
                    AttributeNamespace::Relationship,
                )? {
                    references.push((rel_id, is_header));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    for (rel_id, is_header) in references {
        let Some(relationship) = relationships.get_by_id(&rel_id) else {
            continue;
        };
        let relationship_type = if is_header {
            rel_types::HEADER
        } else {
            rel_types::FOOTER
        };
        if relationship.rel_type != relationship_type
            || !crate::document::relationship_is_internal(relationship)
        {
            continue;
        }
        let part_name =
            OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
        if seen.insert(part_name.clone())
            && let Some(xml) = document.package.get_part(&part_name)
        {
            let story = if is_header { "header" } else { "footer" };
            parts.push((format!("{story}:{part_name}"), xml.to_vec()));
        }
    }
    Ok(parts)
}

fn reject_varying_non_body_merge_fields(
    document: &Document,
    records: &[BTreeMap<String, String>],
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut names = HashSet::new();
    for (_, xml) in non_body_story_parts(document)? {
        collect_raw_merge_field_names(&xml, &mut names)?;
    }
    for name in names {
        let expected = records[0].get(&name).map(String::as_str).unwrap_or("");
        if records
            .iter()
            .skip(1)
            .any(|record| record.get(&name).map(String::as_str).unwrap_or("") != expected)
        {
            return Err(Error::Other(
                "sectioned mail merge cannot vary fields in headers, footers, footnotes, or endnotes"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn collect_raw_merge_field_names(xml: &[u8], names: &mut HashSet<String>) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut complex = Vec::<ComplexInstruction>::new();
    let mut in_instruction_text = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package story XML: {error}")))?;
        let word = match &event {
            Event::Start(element) | Event::Empty(element) => {
                namespace_is_word(&reader.resolver().resolve_element(element.name()).0)
            }
            Event::End(element) => {
                namespace_is_word(&reader.resolver().resolve_element(element.name()).0)
            }
            _ => false,
        };
        match event {
            Event::Start(element) if word => {
                if collect_raw_field_element(&element, reader.resolver(), names, &mut complex)? {
                    in_instruction_text = true;
                }
            }
            Event::Empty(element) if word => {
                collect_raw_field_element(&element, reader.resolver(), names, &mut complex)?;
            }
            Event::Text(text) if in_instruction_text => {
                if let Some(instruction) = complex.last_mut()
                    && instruction.collecting
                {
                    let decoded = text.decode().map_err(|error| {
                        Error::Other(format!("invalid field instruction text: {error}"))
                    })?;
                    let decoded = oxoffice_core::xml_text::normalize_line_ends(&decoded);
                    let unescaped = quick_xml::escape::unescape(&decoded).map_err(|error| {
                        Error::Other(format!("invalid field instruction entity: {error}"))
                    })?;
                    instruction.text.push_str(&unescaped);
                }
            }
            Event::CData(text) if in_instruction_text => {
                if let Some(instruction) = complex.last_mut()
                    && instruction.collecting
                {
                    instruction
                        .text
                        .push_str(&oxoffice_core::xml_text::normalize_line_ends(
                            &text.decode().map_err(|error| {
                                Error::Other(format!("invalid field instruction text: {error}"))
                            })?,
                        ));
                }
            }
            Event::End(element)
                if word && matches_local_name(element.name().as_ref(), b"instrText") =>
            {
                in_instruction_text = false;
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buffer.clear();
    }
}

fn collect_raw_field_element(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    names: &mut HashSet<String>,
    complex: &mut Vec<ComplexInstruction>,
) -> Result<bool> {
    let name = element.name();
    let local = local_name(name.as_ref());
    if local == b"fldSimple" {
        if let Some((_, instruction)) =
            resolved_element_attribute(element, resolver, b"instr", AttributeNamespace::Word)?
        {
            collect_merge_field_name(&instruction, names);
        }
    } else if local == b"fldChar" {
        match resolved_element_attribute(
            element,
            resolver,
            b"fldCharType",
            AttributeNamespace::Word,
        )?
        .map(|(_, value)| value)
        .as_deref()
        {
            Some("begin") => complex.push(ComplexInstruction {
                text: String::new(),
                collecting: true,
            }),
            Some("separate") => {
                if let Some(instruction) = complex.last_mut() {
                    instruction.collecting = false;
                }
            }
            Some("end") => {
                if let Some(instruction) = complex.pop() {
                    collect_merge_field_name(&instruction.text, names);
                }
            }
            _ => {}
        }
    }
    Ok(local == b"instrText")
}

fn collect_merge_field_name(instruction: &str, names: &mut HashSet<String>) {
    let field = Field::new(instruction, "");
    if field.instruction.name == "MERGEFIELD"
        && let Some(FieldArgument::Text(name)) = field.instruction.arguments.first()
    {
        names.insert(name.clone());
    }
}
