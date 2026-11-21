//! Legacy form fields (text input, checkbox, drop-down) in the body, text boxes and shapes.

use super::*;

/// One legacy form field and its stable story-part identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyFormFieldInfo {
    pub source_part: String,
    pub ordinal: usize,
    pub name: Option<String>,
    pub enabled: bool,
    pub calculate_on_exit: bool,
    pub kind: LegacyFormFieldKind,
    pub value: LegacyFormFieldValue,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyStoryKind {
    Header,
    Footer,
    Footnotes,
    Endnotes,
    Comments,
}

impl LegacyStoryKind {
    fn package_kind(self) -> PackageStoryKind {
        match self {
            Self::Header => PackageStoryKind::Header,
            Self::Footer => PackageStoryKind::Footer,
            Self::Footnotes => PackageStoryKind::Footnotes,
            Self::Endnotes => PackageStoryKind::Endnotes,
            Self::Comments => PackageStoryKind::Comments,
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Header => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"
            }
            Self::Footer => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"
            }
            Self::Footnotes => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"
            }
            Self::Endnotes => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml"
            }
            Self::Comments => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"
            }
        }
    }
}

impl Document {
    /// Inventory typed legacy form fields in deterministic story-part order.
    pub fn legacy_form_fields(&self) -> Result<Vec<LegacyFormFieldInfo>> {
        Ok(self
            .legacy_form_fields_with_defaults()?
            .into_iter()
            .map(|(field, _)| field)
            .collect())
    }

    /// One inventory walk that also returns an explicit reset default.
    ///
    /// `None` means the reset-default metadata is absent. Old ordinal prefixes
    /// stay. Newly discovered text-box begins append in part source order.
    /// Comments use their own part sequence.
    pub fn legacy_form_fields_with_defaults(
        &self,
    ) -> Result<Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>> {
        let mut fields = Vec::new();
        let mut paragraphs = Vec::new();
        collect_body_paragraphs(&self.document.body, &mut paragraphs, None);
        append_legacy_form_infos(&mut fields, &self.doc_part_name, &paragraphs, false)?;
        append_legacy_textbox_fields(&mut fields, &self.doc_part_name, &paragraphs, false)?;

        for (part_name, kind, xml) in legacy_story_parts(self)? {
            let story = legacy_story_paragraphs(&xml, kind.package_kind())?;
            let paragraphs = story
                .iter()
                .map(|paragraph| &paragraph.paragraph)
                .collect::<Vec<_>>();
            append_legacy_form_infos(&mut fields, &part_name, &paragraphs, false)?;
            append_legacy_textbox_fields(&mut fields, &part_name, &paragraphs, false)?;
        }
        Ok(fields)
    }

    /// Physical anchors for the same accepted walk, from flushed package bytes.
    ///
    /// Public metadata does not encode field sources. Span projection parses
    /// the package that `flush_to_package` already wrote, including a field
    /// that was authored as `FieldSource::New`.
    pub(crate) fn legacy_form_physical_anchors(&self) -> Result<Vec<LegacyFormAnchor>> {
        let mut anchors = Vec::new();
        let xml = self.package.get_part(&self.doc_part_name).ok_or_else(|| {
            Error::Other(format!("form story part {} is missing", self.doc_part_name))
        })?;
        collect_legacy_form_anchors(
            &mut anchors,
            &self.doc_part_name,
            xml,
            PackageStoryKind::Document,
        )?;
        for (part_name, kind, xml) in legacy_story_parts(self)? {
            collect_legacy_form_anchors(&mut anchors, &part_name, &xml, kind.package_kind())?;
        }
        Ok(anchors)
    }

    /// Set the typed value of one existing legacy form field atomically.
    pub fn set_legacy_form_field_value(
        &mut self,
        source_part: &str,
        ordinal: usize,
        value: LegacyFormFieldValue,
    ) -> Result<LegacyFormFieldInfo> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        // Keep the pre-edit legacy owners, including related-part owners whose
        // source is rewritten below. Defer any inventory error until the
        // requested mutation has run its existing kind and identity checks.
        let expected_owners = candidate.legacy_form_fields();
        if source_part == candidate.doc_part_name {
            let mut remaining = ordinal;
            if !set_nth_legacy_form_in_body(
                &mut candidate.document.body,
                &mut remaining,
                value.clone(),
            )? && !set_nth_legacy_textbox_in_body(
                &mut candidate.document.body,
                &mut remaining,
                value.clone(),
            )? {
                return Err(Error::Other("stale legacy form identity".to_owned()));
            }
        } else {
            let (_, kind, xml) = legacy_story_parts(&candidate)?
                .into_iter()
                .find(|(part_name, _, _)| part_name == source_part)
                .ok_or_else(|| Error::Other("stale legacy form story part".to_owned()))?;
            let mut story = legacy_story_paragraphs(&xml, kind.package_kind())?;
            let mut remaining = ordinal;
            let mut changed = false;
            for paragraph in &mut story {
                if set_nth_legacy_form_in_paragraph(
                    &mut paragraph.paragraph,
                    &mut remaining,
                    value.clone(),
                )? {
                    changed = true;
                    break;
                }
            }
            if !changed {
                for paragraph in &mut story {
                    if set_nth_legacy_textbox_in_paragraph(
                        &mut paragraph.paragraph,
                        &mut remaining,
                        value.clone(),
                    )? {
                        changed = true;
                        break;
                    }
                }
            }
            if !changed {
                return Err(Error::Other("stale legacy form identity".to_owned()));
            }
            let updated = patch_legacy_story_field_sources(&xml, &story, false)?;
            candidate.package.set_part(source_part, updated);
        }

        // The changed field must remain the same accepted owner. An ordinal
        // alone can select its neighbor if publication loses a projection.
        // Ordinary result children may legitimately disappear on literal edits,
        // so compare only the legacy owners, after validating the requested edit.
        let expected_owners = expected_owners?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let actual_owners = reopened.legacy_form_fields()?;
        if expected_owners.len() != actual_owners.len()
            || expected_owners
                .iter()
                .zip(&actual_owners)
                .any(|(before, after)| {
                    before.source_part != after.source_part
                        || before.ordinal != after.ordinal
                        || before.name != after.name
                        || before.kind != after.kind
                })
        {
            return Err(Error::Other(
                "legacy form identity did not survive reopen".to_owned(),
            ));
        }
        let result = actual_owners
            .into_iter()
            .find(|field| field.source_part == source_part && field.ordinal == ordinal)
            .ok_or_else(|| {
                Error::Other("legacy form identity did not survive reopen".to_owned())
            })?;
        if result.value != value {
            return Err(Error::Other(
                "legacy form value did not survive reopen".to_owned(),
            ));
        }
        self.commit_staged_mutation(reopened);
        Ok(result)
    }
}

pub(super) fn field_story_parts(
    document: &Document,
    include_comments: bool,
) -> Vec<(String, PackageStoryKind, Vec<u8>)> {
    let mut parts = Vec::new();
    for (is_header, kind) in [
        (true, PackageStoryKind::Header),
        (false, PackageStoryKind::Footer),
    ] {
        parts.extend(
            referenced_header_footer_parts(document, is_header, true)
                .into_iter()
                .map(|(part, xml)| (part, kind, xml)),
        );
    }
    for (relationship, kind) in [
        (rel_types::FOOTNOTES, PackageStoryKind::Footnotes),
        (rel_types::ENDNOTES, PackageStoryKind::Endnotes),
    ] {
        parts.extend(
            relationship_parts(document, relationship)
                .into_iter()
                .map(|(part, xml)| (part, kind, xml)),
        );
    }
    if include_comments {
        parts.extend(
            relationship_parts(document, rel_types::COMMENTS)
                .into_iter()
                .map(|(part, xml)| (part, PackageStoryKind::Comments, xml)),
        );
    }
    parts
}

fn legacy_story_parts(document: &Document) -> Result<Vec<(String, LegacyStoryKind, Vec<u8>)>> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Ok(Vec::new());
    };
    let story_relationship_types = [
        rel_types::HEADER,
        rel_types::FOOTER,
        rel_types::FOOTNOTES,
        rel_types::ENDNOTES,
        rel_types::COMMENTS,
    ];
    let mut relationship_ids = HashSet::new();
    if relationships
        .items
        .iter()
        .any(|relationship| story_relationship_types.contains(&relationship.rel_type.as_str()))
        && relationships
            .items
            .iter()
            .any(|relationship| !relationship_ids.insert(&relationship.id))
    {
        return Err(Error::Other(
            "duplicate legacy form story relationship id".to_owned(),
        ));
    }
    let mut parts = Vec::new();
    for (relationship_type, kind) in [
        (rel_types::HEADER, LegacyStoryKind::Header),
        (rel_types::FOOTER, LegacyStoryKind::Footer),
        (rel_types::FOOTNOTES, LegacyStoryKind::Footnotes),
        (rel_types::ENDNOTES, LegacyStoryKind::Endnotes),
    ] {
        let matching_relationships = relationships
            .items
            .iter()
            .filter(|relationship| {
                relationship.rel_type == relationship_type
                    && crate::document::relationship_is_internal(relationship)
            })
            .collect::<Vec<_>>();
        if matches!(kind, LegacyStoryKind::Footnotes | LegacyStoryKind::Endnotes)
            && matching_relationships.len() > 1
        {
            return Err(Error::Other(format!(
                "legacy form story has {} {relationship_type} relationships, expected at most one",
                matching_relationships.len()
            )));
        }
        for relationship in matching_relationships {
            crate::building_block::validate_internal_target(
                &document.doc_part_name,
                &relationship.target,
            )?;
            let part_name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if document.package.content_types.override_for(&part_name) != Some(kind.content_type())
            {
                return Err(Error::Other(
                    "legacy form story requires its exact content type override".to_owned(),
                ));
            }
            if parts.iter().any(|(existing_part, existing_kind, _)| {
                existing_part == &part_name && existing_kind != &kind
            }) {
                return Err(Error::Other(
                    "legacy form story part has conflicting relationship roles".to_owned(),
                ));
            }
            let xml = document.package.get_part(&part_name).ok_or_else(|| {
                Error::Other("legacy form story relationship target is missing".to_owned())
            })?;
            parts.push((part_name, kind, xml.to_vec()));
        }
    }
    parts.sort_by(|left, right| left.0.cmp(&right.0));
    parts.dedup_by(|left, right| left.0 == right.0);
    let mut comments = Vec::new();
    for relationship in relationships.items.iter().filter(|relationship| {
        relationship.rel_type == rel_types::COMMENTS
            && crate::document::relationship_is_internal(relationship)
    }) {
        crate::building_block::validate_internal_target(
            &document.doc_part_name,
            &relationship.target,
        )?;
        let part_name =
            OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
        if document.package.content_types.override_for(&part_name)
            != Some(LegacyStoryKind::Comments.content_type())
        {
            return Err(Error::Other(
                "legacy form story requires its exact content type override".to_owned(),
            ));
        }
        if parts
            .iter()
            .any(|(existing_part, _, _)| existing_part == &part_name)
        {
            return Err(Error::Other(
                "legacy form story part has conflicting relationship roles".to_owned(),
            ));
        }
        let xml = document.package.get_part(&part_name).ok_or_else(|| {
            Error::Other("legacy form story relationship target is missing".to_owned())
        })?;
        comments.push((part_name, LegacyStoryKind::Comments, xml.to_vec()));
    }
    comments.sort_by(|left, right| left.0.cmp(&right.0));
    comments.dedup_by(|left, right| left.0 == right.0);
    parts.extend(comments);
    Ok(parts)
}

pub(crate) struct LegacyFormAnchor {
    pub part: String,
    pub ordinal: usize,
    pub start: usize,
    pub end: usize,
}

fn append_legacy_form_infos(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    paragraphs: &[&CT_P],
    capture_source: bool,
) -> Result<()> {
    let mut ordinal = output
        .iter()
        .filter(|field| field.0.source_part == source_part)
        .count();
    let mut captured = Vec::new();
    for paragraph in paragraphs {
        append_legacy_form_infos_in_paragraph(
            output,
            source_part,
            &mut ordinal,
            paragraph,
            capture_source,
            &mut captured,
        )?;
    }
    Ok(())
}

pub(super) fn append_legacy_form_infos_in_paragraph(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    ordinal: &mut usize,
    paragraph: &CT_P,
    capture_source: bool,
    captured: &mut Vec<Vec<u8>>,
) -> Result<()> {
    for boundary in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter()
            .filter(|(position, _, _, _)| *position == boundary)
        {
            append_legacy_form_infos_in_control(
                output,
                source_part,
                ordinal,
                control,
                capture_source,
                captured,
            )?;
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            append_legacy_form_infos_in_run(
                output,
                source_part,
                ordinal,
                run,
                capture_source,
                captured,
            )?;
        }
    }
    Ok(())
}

fn append_legacy_form_infos_in_run(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    ordinal: &mut usize,
    run: &CT_R,
    capture_source: bool,
    captured: &mut Vec<Vec<u8>>,
) -> Result<()> {
    for content in &run.content {
        if let RunContent::Field(field) = content {
            append_legacy_form_field_info(
                output,
                source_part,
                ordinal,
                field,
                capture_source,
                captured,
            )?;
        }
    }
    Ok(())
}

fn append_legacy_form_infos_in_control(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    ordinal: &mut usize,
    control: &CT_Sdt,
    capture_source: bool,
    captured: &mut Vec<Vec<u8>>,
) -> Result<()> {
    let inline_fields = control.inline_legacy_form_fields_with_content_indices()?;
    let mut inline_index = 0usize;
    for (content_index, content) in control.content.iter().enumerate() {
        while inline_fields
            .get(inline_index)
            .is_some_and(|(position, _)| *position == content_index)
        {
            append_one_legacy_form_field_info(
                output,
                source_part,
                ordinal,
                &inline_fields[inline_index].1,
                capture_source,
                captured,
            )?;
            inline_index += 1;
        }
        match content {
            SdtContent::Run(_) => {}
            SdtContent::ContentControl(control) => append_legacy_form_infos_in_control(
                output,
                source_part,
                ordinal,
                control,
                capture_source,
                captured,
            )?,
            SdtContent::Paragraph(paragraph) => append_legacy_form_infos_in_paragraph(
                output,
                source_part,
                ordinal,
                paragraph,
                capture_source,
                captured,
            )?,
            SdtContent::Table(_)
            | SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::RawXml(_) => {}
        }
    }
    if inline_index != inline_fields.len() {
        return Err(Error::Other(
            "inline legacy form source boundary is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn append_legacy_form_field_info(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    ordinal: &mut usize,
    field: &Field,
    capture_source: bool,
    captured: &mut Vec<Vec<u8>>,
) -> Result<()> {
    field.validate_legacy_form_owner()?;
    append_one_legacy_form_field_info(
        output,
        source_part,
        ordinal,
        field,
        capture_source,
        captured,
    )?;
    for nested in field.nested_fields_in_source_order() {
        append_legacy_form_field_info(
            output,
            source_part,
            ordinal,
            nested,
            capture_source,
            captured,
        )?;
    }
    Ok(())
}

fn append_one_legacy_form_field_info(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    ordinal: &mut usize,
    field: &Field,
    capture_source: bool,
    captured: &mut Vec<Vec<u8>>,
) -> Result<()> {
    if let Some(value) = field.legacy_form_value().ok().flatten() {
        let Some(form) = &field.legacy_form else {
            return Ok(());
        };
        let default = field.legacy_form_default_value().ok().flatten();
        if capture_source {
            let Some((raw, _)) = field.source_replacement()? else {
                return Err(Error::Other(
                    "legacy form field has no parsed physical source".to_owned(),
                ));
            };
            captured.push(raw.to_vec());
        }
        output.push((
            LegacyFormFieldInfo {
                source_part: source_part.to_owned(),
                ordinal: *ordinal,
                name: form.name.clone(),
                enabled: form.enabled,
                calculate_on_exit: form.calculate_on_exit,
                kind: form.kind,
                value,
                choices: form.choices.clone(),
            },
            default,
        ));
        *ordinal += 1;
    }
    Ok(())
}

fn append_legacy_textbox_fields(
    output: &mut Vec<(LegacyFormFieldInfo, Option<LegacyFormFieldValue>)>,
    source_part: &str,
    paragraphs: &[&CT_P],
    capture_source: bool,
) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for paragraph in paragraphs {
        seen.insert(std::ptr::from_ref(*paragraph) as usize);
    }
    let mut extra = Vec::new();
    for paragraph in paragraphs {
        collect_unseen_textbox_paragraphs(paragraph, &mut seen, &mut extra);
    }
    append_legacy_form_infos(output, source_part, &extra, capture_source)
}

fn collect_unseen_textbox_paragraphs<'a>(
    paragraph: &'a CT_P,
    seen: &mut std::collections::HashSet<usize>,
    extra: &mut Vec<&'a CT_P>,
) {
    for path in paragraph.accepted_run_paths() {
        let Some(run) = paragraph.accepted_run(&path) else {
            continue;
        };
        collect_drawings_in_run(run, seen, extra);
    }
}

fn collect_drawings_in_run<'a>(
    run: &'a CT_R,
    seen: &mut std::collections::HashSet<usize>,
    extra: &mut Vec<&'a CT_P>,
) {
    for content in &run.content {
        if let RunContent::Drawing(drawing) = content
            && let Some(shape) = drawing
                .anchor
                .as_ref()
                .and_then(|anchor| anchor.shape.as_ref())
        {
            if let Some(body) = &shape.text_body {
                let mut nested = Vec::new();
                collect_body_paragraphs(body, &mut nested, None);
                for nested in nested {
                    push_unseen_paragraph(nested, seen, extra);
                }
            } else {
                for nested in &shape.text {
                    push_unseen_paragraph(nested, seen, extra);
                }
            }
        }
    }
}

fn push_unseen_paragraph<'a>(
    paragraph: &'a CT_P,
    seen: &mut std::collections::HashSet<usize>,
    extra: &mut Vec<&'a CT_P>,
) {
    let identity = std::ptr::from_ref(paragraph) as usize;
    if seen.insert(identity) {
        extra.push(paragraph);
        collect_unseen_textbox_paragraphs(paragraph, seen, extra);
    }
}

fn set_nth_legacy_textbox_in_body(
    body: &mut CT_Body,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for content in &mut body.content {
        let changed = match content {
            BodyContent::Paragraph(paragraph) => {
                set_nth_legacy_textbox_in_paragraph(paragraph, remaining, value.clone())?
            }
            BodyContent::Table(table) => {
                set_nth_legacy_textbox_in_table(table, remaining, value.clone())?
            }
            BodyContent::ContentControl(control) => {
                set_nth_legacy_textbox_in_control(control, remaining, value.clone())?
            }
            BodyContent::RawXml(_) => false,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_textbox_in_table(
    table: &mut CT_Tbl,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for row in &mut table.rows {
        for cell in &mut row.cells {
            for content in &mut cell.content {
                let changed = match content {
                    CellContent::Paragraph(paragraph) => {
                        set_nth_legacy_textbox_in_paragraph(paragraph, remaining, value.clone())?
                    }
                    CellContent::Table(table) => {
                        set_nth_legacy_textbox_in_table(table, remaining, value.clone())?
                    }
                    CellContent::ContentControl(control) => {
                        set_nth_legacy_textbox_in_control(control, remaining, value.clone())?
                    }
                };
                if changed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn set_nth_legacy_textbox_in_control(
    control: &mut CT_Sdt,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for content in &mut control.content {
        let changed = match content {
            SdtContent::Paragraph(paragraph) => {
                set_nth_legacy_textbox_in_paragraph(paragraph, remaining, value.clone())?
            }
            SdtContent::Table(table) => {
                set_nth_legacy_textbox_in_table(table, remaining, value.clone())?
            }
            SdtContent::ContentControl(control) => {
                set_nth_legacy_textbox_in_control(control, remaining, value.clone())?
            }
            SdtContent::Run(run) => set_nth_legacy_textbox_in_run(run, remaining, value.clone())?,
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::RawXml(_) => false,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_textbox_in_paragraph(
    paragraph: &mut CT_P,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    let paths = paragraph.accepted_run_paths();
    for path in paths {
        let Some(run) = paragraph.accepted_run(&path) else {
            continue;
        };
        let mut replacement = run.clone();
        if !set_nth_legacy_textbox_in_run(&mut replacement, remaining, value.clone())? {
            continue;
        }
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other(
                "accepted text box owner disappeared".to_owned(),
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn set_nth_legacy_textbox_in_run(
    run: &mut CT_R,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    let mut patched = false;
    for content in &mut run.content {
        if let RunContent::Drawing(drawing) = content
            && mutate_drawing_textbox(drawing, remaining, value.clone())?
        {
            patch_drawing_retained_field_sources(drawing)?;
            patched = true;
            break;
        }
    }
    if !patched {
        return Ok(false);
    }
    // AlternateContent serialises from the run's retained bytes.
    // A direct drawing has no second copy, so a missing span is left alone.
    let groups = run
        .content
        .iter()
        .filter_map(|content| match content {
            RunContent::Drawing(drawing) => Some(shape_textbox_paragraph_groups(drawing)),
            _ => None,
        })
        .collect::<Result<Vec<_>>>()?;
    let groups = groups.into_iter().flatten().collect::<Vec<_>>();
    for raw in &mut run.extra_xml {
        if word_txbx_paragraph_spans(raw, &[])?.len() == groups.len() && !groups.is_empty() {
            splice_textbox_paragraph_groups(raw, 0, &groups, &[])?;
        }
    }
    Ok(true)
}

fn mutate_drawing_textbox(
    drawing: &mut CT_Drawing,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    let Some(shape) = drawing
        .anchor
        .as_mut()
        .and_then(|anchor| anchor.shape.as_mut())
    else {
        return Ok(false);
    };
    if let Some(body) = &mut shape.text_body {
        return set_nth_legacy_form_in_shape_body(body, remaining, value);
    }
    for nested in &mut shape.text {
        if set_nth_legacy_form_in_paragraph(nested, remaining, value.clone())? {
            return Ok(true);
        }
        if set_nth_legacy_textbox_in_paragraph(nested, remaining, value.clone())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn patch_drawing_retained_field_sources(drawing: &mut CT_Drawing) -> Result<()> {
    let groups = shape_textbox_paragraph_groups(drawing)?;
    if groups.iter().all(|group| group.is_empty()) {
        return Ok(());
    }
    let mut patched = false;
    if drawing
        .anchor
        .as_ref()
        .is_some_and(|anchor| anchor.raw_xml.is_some())
    {
        let bindings = drawing
            .anchor
            .as_ref()
            .map(|anchor| anchor.source_namespace_bindings.clone())
            .unwrap_or_default();
        let raw = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.raw_xml.as_mut())
            .expect("raw");
        splice_textbox_paragraph_groups(raw, 0, &groups, &bindings)?;
        patched = true;
    }
    if drawing
        .inline
        .as_ref()
        .is_some_and(|inline| inline.raw_xml.is_some())
    {
        let raw = drawing
            .inline
            .as_mut()
            .and_then(|inline| inline.raw_xml.as_mut())
            .expect("raw");
        splice_textbox_paragraph_groups(raw, 0, &groups, &[])?;
        patched = true;
    }
    if patched {
        Ok(())
    } else {
        Err(Error::Other(
            "text box field has no retained drawing owner".to_owned(),
        ))
    }
}

type TextBoxFieldSources = Vec<Vec<(Vec<u8>, Vec<u8>)>>;

fn shape_textbox_paragraph_groups(drawing: &CT_Drawing) -> Result<TextBoxFieldSources> {
    let Some(shape) = drawing
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.shape.as_ref())
    else {
        return Ok(Vec::new());
    };
    let mut paragraphs = Vec::new();
    if let Some(body) = &shape.text_body {
        collect_body_paragraphs(body, &mut paragraphs, None);
    } else {
        paragraphs.extend(shape.text.iter());
    }
    let mut groups = Vec::new();
    for paragraph in paragraphs {
        groups.push(staged_paragraph_field_source_replacements(paragraph)?);
        append_nested_textbox_groups(paragraph, &mut groups)?;
    }
    Ok(groups)
}

pub(super) fn append_nested_textbox_groups(
    paragraph: &CT_P,
    groups: &mut TextBoxFieldSources,
) -> Result<()> {
    for path in paragraph.accepted_run_paths() {
        let Some(run) = paragraph.accepted_run(&path) else {
            continue;
        };
        for content in &run.content {
            if let RunContent::Drawing(drawing) = content {
                groups.extend(shape_textbox_paragraph_groups(drawing)?);
            }
        }
    }
    Ok(())
}

fn splice_textbox_paragraph_groups(
    xml: &mut Vec<u8>,
    origin: usize,
    groups: &TextBoxFieldSources,
    inherited: &[(String, String)],
) -> Result<()> {
    let spans = word_txbx_paragraph_spans(xml, inherited)?;
    if spans.len() != groups.len() {
        return Err(Error::Other(
            "text box paragraph boundaries do not match the typed owner".to_owned(),
        ));
    }
    let mut edits = Vec::new();
    for ((start, end), group) in spans.into_iter().zip(groups) {
        let mut search_start = 0usize;
        for (source, replacement) in group {
            let Some(relative) =
                find_bound_field_source(xml, start, end, source, search_start, inherited)?
            else {
                return Err(Error::Other(
                    "text box field source was not found at its text box paragraph boundary"
                        .to_owned(),
                ));
            };
            let at = origin + start + relative;
            if source != replacement {
                edits.push(FieldSourceEdit {
                    start: at,
                    end: at + source.len(),
                    replacement: replacement.clone(),
                });
            }
            search_start = relative + source.len();
        }
    }
    for edit in edits.into_iter().rev() {
        xml.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct TextBoxScanFrame {
    reject: bool,
    opened_txbx: bool,
    is_paragraph: bool,
}

pub(super) fn word_txbx_paragraph_spans(
    xml: &[u8],
    inherited: &[(String, String)],
) -> Result<Vec<(usize, usize)>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scopes = vec![inherited.to_vec()];
    let mut frames = vec![TextBoxScanFrame {
        reject: false,
        opened_txbx: false,
        is_paragraph: false,
    }];
    let mut txbx_depth = 0i32;
    let mut open = Vec::new();
    let mut spans = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid text box drawing XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let frame =
                    push_textbox_scan_frame(&element, &mut scopes, &mut frames, &mut txbx_depth)?;
                if frame.is_paragraph && !frame.reject && txbx_depth > 0 {
                    open.push(spans.len());
                    spans.push((before, before));
                }
            }
            Event::Empty(element) => {
                let frame =
                    push_textbox_scan_frame(&element, &mut scopes, &mut frames, &mut txbx_depth)?;
                if frame.is_paragraph && !frame.reject && txbx_depth > 0 {
                    spans.push((before, before));
                }
                pop_textbox_scan_frame(&mut scopes, &mut frames, &mut txbx_depth);
            }
            Event::End(_) => {
                if frames
                    .last()
                    .is_some_and(|frame| frame.is_paragraph && !frame.reject)
                    && txbx_depth > 0
                    && let Some(index) = open.pop()
                {
                    spans[index].1 = before;
                }
                pop_textbox_scan_frame(&mut scopes, &mut frames, &mut txbx_depth);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(spans)
}

fn push_textbox_scan_frame(
    element: &BytesStart<'_>,
    scopes: &mut Vec<Vec<(String, String)>>,
    frames: &mut Vec<TextBoxScanFrame>,
    txbx_depth: &mut i32,
) -> Result<TextBoxScanFrame> {
    let mut scope = Vec::new();
    push_xmlns_declarations(element, &mut scope)?;
    scopes.push(scope);
    let name = element.name().as_ref().to_vec();
    let namespace = bound_namespace(&name, scopes);
    let local = local_name(&name);
    let parent = frames.last().expect("scan root");
    let reject = parent.reject || textbox_owner_is_rejected(namespace, local);
    let opened_txbx = !reject && namespace == Some(W_NS) && local == b"txbxContent";
    if opened_txbx {
        *txbx_depth += 1;
    }
    let frame = TextBoxScanFrame {
        reject,
        opened_txbx,
        is_paragraph: namespace == Some(W_NS) && local == b"p",
    };
    frames.push(frame);
    Ok(frame)
}

fn pop_textbox_scan_frame(
    scopes: &mut Vec<Vec<(String, String)>>,
    frames: &mut Vec<TextBoxScanFrame>,
    txbx_depth: &mut i32,
) {
    if frames.pop().is_some_and(|frame| frame.opened_txbx) {
        *txbx_depth -= 1;
    }
    scopes.pop();
}

fn textbox_owner_is_rejected(namespace: Option<&str>, local: &[u8]) -> bool {
    const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
    const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    const WPS_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
    match namespace {
        Some(W_NS) => !word_accepted_form_container(local),
        Some(WP_NS | A_NS | WPS_NS) => false,
        // AlternateContent is layout-only. Choice, Fallback, and every other
        // foreign or unbound wrapper are outside the accepted form inventory.
        _ => true,
    }
}

pub(crate) fn word_accepted_form_container(local: &[u8]) -> bool {
    // accepted_run_paths exposes sdt, sdtContent, hyperlink, ins, and moveTo.
    // Story and body collection expose the package roots, tables, and cells.
    // smartTag, customXml, and fldSimple stay out because that walk does not
    // expose a drawing inside them.
    matches!(
        local,
        b"document"
            | b"body"
            | b"hdr"
            | b"ftr"
            | b"footnotes"
            | b"footnote"
            | b"endnotes"
            | b"endnote"
            | b"comments"
            | b"comment"
            | b"tbl"
            | b"tr"
            | b"tc"
            | b"p"
            | b"r"
            | b"drawing"
            | b"txbxContent"
            | b"sdt"
            | b"sdtContent"
            | b"hyperlink"
            | b"ins"
            | b"moveTo"
    )
}

fn bound_namespace<'a>(name: &[u8], scopes: &'a [Vec<(String, String)>]) -> Option<&'a str> {
    let prefix = match name
        .split(|byte| *byte == b':')
        .collect::<Vec<_>>()
        .as_slice()
    {
        [prefix, _] => String::from_utf8_lossy(prefix).into_owned(),
        [_] => String::new(),
        _ => return None,
    };
    scopes.iter().rev().find_map(|scope| {
        scope
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == &prefix)
            .map(|(_, uri)| uri.as_str())
    })
}

fn push_xmlns_declarations(
    element: &BytesStart<'_>,
    scope: &mut Vec<(String, String)>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("invalid text box namespace declaration: {error}"))
        })?;
        let key = attribute.key.as_ref();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| Error::Other(format!("invalid text box namespace value: {error}")))?;
        let value = value.into_owned();
        if key == b"xmlns" {
            scope.push((String::new(), value));
        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            scope.push((String::from_utf8_lossy(prefix).into_owned(), value));
        }
    }
    Ok(())
}

fn bound_name_is_word(name: &[u8], scopes: &[Vec<(String, String)>]) -> bool {
    let prefix = match name
        .split(|byte| *byte == b':')
        .collect::<Vec<_>>()
        .as_slice()
    {
        [prefix, _] => String::from_utf8_lossy(prefix).into_owned(),
        [_] => String::new(),
        _ => return false,
    };
    scopes
        .iter()
        .rev()
        .find_map(|scope| {
            scope
                .iter()
                .rev()
                .find(|(candidate, _)| candidate == &prefix)
                .map(|(_, uri)| uri.as_str() == W_NS)
        })
        .unwrap_or(false)
}

pub(super) fn find_bound_field_source(
    xml: &[u8],
    paragraph_start: usize,
    paragraph_end: usize,
    source: &[u8],
    search_start: usize,
    inherited: &[(String, String)],
) -> Result<Option<usize>> {
    let paragraph_xml = &xml[paragraph_start..paragraph_end];
    let mut candidate_start = search_start;
    while let Some(relative) = find_bytes(&paragraph_xml[candidate_start..], source) {
        let candidate = candidate_start + relative;
        if bound_field_source_has_typed_ancestors(
            xml,
            paragraph_start + candidate,
            paragraph_start,
            inherited,
        )? {
            return Ok(Some(candidate));
        }
        candidate_start = candidate + source.len().max(1);
    }
    Ok(None)
}

fn bound_field_source_has_typed_ancestors(
    xml: &[u8],
    source_start: usize,
    paragraph_start: usize,
    inherited: &[(String, String)],
) -> Result<bool> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scopes = vec![inherited.to_vec()];
    let mut ancestors = Vec::<(usize, bool, Vec<u8>)>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid text box field XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let mut scope = Vec::new();
                push_xmlns_declarations(&element, &mut scope)?;
                scopes.push(scope);
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let word = bound_name_is_word(&owned, &scopes);
                if before == source_start {
                    let local = local_name(&owned);
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
                ancestors.push((before, word, local_name(&owned).to_vec()));
            }
            Event::Empty(element) => {
                let mut scope = Vec::new();
                push_xmlns_declarations(&element, &mut scope)?;
                scopes.push(scope);
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let word = bound_name_is_word(&owned, &scopes);
                if before == source_start {
                    let local = local_name(&owned);
                    scopes.pop();
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
                scopes.pop();
            }
            Event::End(_) => {
                scopes.pop();
                ancestors.pop();
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn set_nth_legacy_form_in_shape_body(
    body: &mut CT_Body,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for content in &mut body.content {
        let changed = match content {
            BodyContent::Paragraph(paragraph) => {
                set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
                    || set_nth_legacy_textbox_in_paragraph(paragraph, remaining, value.clone())?
            }
            BodyContent::Table(table) => {
                set_nth_legacy_form_in_shape_table(table, remaining, value.clone())?
            }
            BodyContent::ContentControl(control) => set_nth_legacy_form_in_shape_control(
                control,
                LegacyBlockOwner::Root,
                remaining,
                value.clone(),
            )?,
            BodyContent::RawXml(_) => false,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_shape_table(
    table: &mut CT_Tbl,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(position, _, _)| *position == boundary)
        {
            if set_nth_legacy_form_in_shape_control(
                control,
                LegacyBlockOwner::Table,
                remaining,
                value.clone(),
            )? {
                return Ok(true);
            }
        }
        if let Some(row) = table.rows.get_mut(boundary)
            && set_nth_legacy_form_in_shape_row(row, remaining, value.clone())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_shape_row(
    row: &mut CT_Row,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(position, _, _)| *position == boundary)
        {
            if set_nth_legacy_form_in_shape_control(
                control,
                LegacyBlockOwner::Row,
                remaining,
                value.clone(),
            )? {
                return Ok(true);
            }
        }
        if let Some(cell) = row.cells.get_mut(boundary) {
            for content in &mut cell.content {
                let changed = match content {
                    CellContent::Paragraph(paragraph) => {
                        set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
                            || set_nth_legacy_textbox_in_paragraph(
                                paragraph,
                                remaining,
                                value.clone(),
                            )?
                    }
                    CellContent::Table(table) => {
                        set_nth_legacy_form_in_shape_table(table, remaining, value.clone())?
                    }
                    CellContent::ContentControl(control) => set_nth_legacy_form_in_shape_control(
                        control,
                        LegacyBlockOwner::Cell,
                        remaining,
                        value.clone(),
                    )?,
                };
                if changed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_shape_control(
    control: &mut CT_Sdt,
    owner: LegacyBlockOwner,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for content in &mut control.content {
        let changed = match (owner, content) {
            (LegacyBlockOwner::Root | LegacyBlockOwner::Cell, SdtContent::Paragraph(paragraph)) => {
                set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
                    || set_nth_legacy_textbox_in_paragraph(paragraph, remaining, value.clone())?
            }
            (LegacyBlockOwner::Root | LegacyBlockOwner::Cell, SdtContent::Table(table)) => {
                set_nth_legacy_form_in_shape_table(table, remaining, value.clone())?
            }
            (LegacyBlockOwner::Table, SdtContent::Row(row)) => {
                set_nth_legacy_form_in_shape_row(row, remaining, value.clone())?
            }
            (LegacyBlockOwner::Row, SdtContent::Cell(cell)) => {
                let mut row = CT_Row::new();
                row.cells.push(cell.clone());
                let changed = set_nth_legacy_form_in_shape_row(&mut row, remaining, value.clone())?;
                if changed {
                    *cell = row.cells.remove(0);
                }
                changed
            }
            (_, SdtContent::ContentControl(control)) => {
                set_nth_legacy_form_in_shape_control(control, owner, remaining, value.clone())?
            }
            _ => false,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_body(
    body: &mut CT_Body,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for content in &mut body.content {
        let changed = match content {
            BodyContent::Paragraph(paragraph) => {
                set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
            }
            BodyContent::Table(table) => {
                set_nth_legacy_form_in_table(table, remaining, value.clone())?
            }
            BodyContent::ContentControl(control) => {
                set_nth_legacy_form_in_control(control, remaining, value.clone())?
            }
            BodyContent::RawXml(_) => false,
        };
        if changed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_paragraph(
    paragraph: &mut CT_P,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for boundary in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter_mut()
            .filter(|(position, _, _, _)| *position == boundary)
        {
            if set_nth_legacy_form_in_control(control, remaining, value.clone())? {
                return Ok(true);
            }
        }
        if let Some(run) = paragraph.runs.get_mut(boundary) {
            for content in &mut run.content {
                if let RunContent::Field(field) = content
                    && set_nth_legacy_form_in_field(field, remaining, value.clone())?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_field(
    field: &mut Field,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    field
        .set_nth_legacy_form_value_in_source_order(remaining, &value)
        .map_err(Into::into)
}

fn set_nth_legacy_form_in_table(
    table: &mut CT_Tbl,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(position, _, _)| *position == boundary)
        {
            if set_nth_legacy_form_in_control(control, remaining, value.clone())? {
                return Ok(true);
            }
        }
        if let Some(row) = table.rows.get_mut(boundary)
            && set_nth_legacy_form_in_row(row, remaining, value.clone())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_row(
    row: &mut CT_Row,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(position, _, _)| *position == boundary)
        {
            if set_nth_legacy_form_in_control(control, remaining, value.clone())? {
                return Ok(true);
            }
        }
        if let Some(cell) = row.cells.get_mut(boundary) {
            for content in &mut cell.content {
                let changed = match content {
                    CellContent::Paragraph(paragraph) => {
                        set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
                    }
                    CellContent::Table(table) => {
                        set_nth_legacy_form_in_table(table, remaining, value.clone())?
                    }
                    CellContent::ContentControl(control) => {
                        set_nth_legacy_form_in_control(control, remaining, value.clone())?
                    }
                };
                if changed {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn set_nth_legacy_form_in_control(
    control: &mut CT_Sdt,
    remaining: &mut usize,
    value: LegacyFormFieldValue,
) -> Result<bool> {
    let inline_positions = control
        .inline_legacy_form_fields_with_content_indices()?
        .into_iter()
        .map(|(content_index, _)| content_index)
        .collect::<Vec<_>>();
    let mut inline_index = 0usize;
    for content_index in 0..control.content.len() {
        while inline_positions
            .get(inline_index)
            .is_some_and(|position| *position == content_index)
        {
            if *remaining == 0 {
                return control
                    .set_inline_legacy_form_value(inline_index, value)
                    .map_err(Into::into);
            }
            *remaining -= 1;
            inline_index += 1;
        }
        let content = &mut control.content[content_index];
        let changed = match content {
            SdtContent::Paragraph(paragraph) => {
                set_nth_legacy_form_in_paragraph(paragraph, remaining, value.clone())?
            }
            SdtContent::Table(table) => {
                set_nth_legacy_form_in_table(table, remaining, value.clone())?
            }
            SdtContent::Row(row) => {
                let mut table = CT_Tbl::new();
                table.rows.push(row.clone());
                let changed = set_nth_legacy_form_in_table(&mut table, remaining, value.clone())?;
                if changed {
                    *row = table.rows.remove(0);
                }
                changed
            }
            SdtContent::Cell(cell) => {
                let mut row = CT_Row::new();
                row.cells.push(cell.clone());
                let mut table = CT_Tbl::new();
                table.rows.push(row);
                let changed = set_nth_legacy_form_in_table(&mut table, remaining, value.clone())?;
                if changed {
                    *cell = table.rows.remove(0).cells.remove(0);
                }
                changed
            }
            SdtContent::Run(_) => false,
            SdtContent::ContentControl(control) => {
                set_nth_legacy_form_in_control(control, remaining, value.clone())?
            }
            SdtContent::RawXml(_) => false,
        };
        if changed {
            return Ok(true);
        }
    }
    if inline_index != inline_positions.len() {
        return Err(Error::Other(
            "inline legacy form source boundary is invalid".to_owned(),
        ));
    }
    Ok(false)
}
