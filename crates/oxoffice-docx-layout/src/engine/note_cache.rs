//! Note references and the runs of cached notes.

use super::*;

/// Project each current stored owner once, retaining actual inner view marks.
/// Source association is qualified by the outer source and actual accepted
/// run/content slot before any prepared address is used for consumption.
pub(super) fn stored_note_cache_runs(
    field: &oxoffice_docx_oxml::text::Field,
    inherited: Option<&CT_RPr>,
    view: RevisionView,
    source: Option<FieldSource>,
    inherited_locked: bool,
    input: Option<&LayoutInput>,
    cached_child: bool,
) -> Result<Vec<StoredFieldRun>> {
    let occurrence = source.map(|source| WordFieldOccurrence {
        source,
        owner_id: field.source_owner_id(),
        paint_scope: 0,
    });
    let locked = inherited_locked || field.locked() == Some(true);
    let field_kind = if cached_child && !locked && source.is_some() {
        let instruction = field.effective_instruction();
        if validate_page_field_instruction(&instruction).is_err() {
            None
        } else {
            match instruction.name.as_str() {
                "PAGE" => Some(FieldKind::Page),
                "NUMPAGES" => Some(FieldKind::NumPages),
                "SECTION" => Some(FieldKind::Section),
                "SECTIONPAGES" => Some(FieldKind::SectionPages),
                "PAGEREF" if !page_reference_keeps_stored_display(&field.cached_result) => input
                    .and_then(|input| {
                        field_text_argument(&instruction, 0)
                            .and_then(|name| page_ref_id(input, name))
                    })
                    .map(FieldKind::TargetPage),
                _ => None,
            }
        }
    } else {
        None
    };
    let prepared = field.cached_result_owner_projection().map_err(|error| {
        LayoutError::Layout(format!("stored field preparation failed: {error}"))
    })?;
    let runs = if let Some(paragraph) = &prepared {
        project_paragraph_runs(paragraph, view)
    } else if let Some(runs) = field.cached_result_runs() {
        runs.iter()
            .enumerate()
            .map(|(boundary, run)| ProjectedRun {
                control: None,
                run,
                boundary,
                raw_order: RawOrder::BeforeRaw,
                ordinary_run_index: Some(boundary),
                hyperlink_index: None,
                force_underline: false,
                force_strike: false,
                revision: None,
            })
            .collect()
    } else {
        let mut run = CT_R::new(&field.effective_display());
        run.properties = inherited.cloned();
        return Ok(vec![StoredFieldRun {
            occurrence,
            run,
            force_underline: false,
            force_strike: false,
            revision: None,
            source,
            field_kind,
        }]);
    };
    let accepted = if let Some(paragraph) = &prepared {
        paragraph
            .accepted_run_paths()
            .into_iter()
            .map(|path| paragraph.accepted_run(&path).expect("fresh accepted path"))
            .collect::<Vec<_>>()
    } else {
        field
            .cached_result_runs()
            .expect("borrowed cache selected")
            .iter()
            .collect()
    };
    let children = field.cached_fields_in_source_order();
    let mut next = source
        .map(|source| source.index)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| LayoutError::Layout("stored field source index overflow".into()))?;
    for instruction_child in field.nested_fields_in_source_order() {
        let mut pending = vec![instruction_child];
        while let Some(child) = pending.pop() {
            next = next
                .checked_add(1)
                .ok_or_else(|| LayoutError::Layout("stored field source index overflow".into()))?;
            pending.extend(child.all_nested_fields_in_source_order());
        }
    }
    let mut slots = HashMap::new();
    let mut ordinal = 0usize;
    for run in accepted {
        for (slot, content) in run.content.iter().enumerate() {
            let RunContent::Field(child) = content else {
                continue;
            };
            let original = children.get(ordinal).ok_or_else(|| {
                LayoutError::Layout("stored accepted field occurrence is extra".into())
            })?;
            if original.source_owner_id() != child.source_owner_id()
                || original.form() != child.form()
            {
                return Err(LayoutError::Layout(
                    "stored accepted field source slot changed".into(),
                ));
            }
            if slots
                .insert(
                    (std::ptr::from_ref(run), slot),
                    source.map(|parent| FieldSource {
                        node: parent.node,
                        index: next,
                    }),
                )
                .is_some()
            {
                return Err(LayoutError::Layout(
                    "stored accepted field source slot is ambiguous".into(),
                ));
            }
            ordinal += 1;
            let mut pending = vec![*original];
            while let Some(child) = pending.pop() {
                next = next.checked_add(1).ok_or_else(|| {
                    LayoutError::Layout("stored field source index overflow".into())
                })?;
                pending.extend(child.all_nested_fields_in_source_order());
            }
        }
    }
    if ordinal != children.len() {
        return Err(LayoutError::Layout(
            "stored accepted field occurrence is missing".into(),
        ));
    }
    let mut result = Vec::new();
    for projected in runs {
        let run = projected.run;
        let mut segment = run.clone();
        segment.content.clear();
        if segment.properties.is_none() {
            segment.properties = inherited.cloned();
        }
        for (slot, content) in run.content.iter().enumerate() {
            if let RunContent::Field(child) = content {
                if !segment.content.is_empty() || !segment.extra_xml.is_empty() {
                    result.push(StoredFieldRun {
                        occurrence,
                        run: segment.clone(),
                        force_underline: projected.force_underline,
                        force_strike: projected.force_strike,
                        revision: projected.revision,
                        source,
                        field_kind,
                    });
                    segment.content.clear();
                    segment.extra_xml.clear();
                    segment.extra_xml_positions.clear();
                }
                let child_source = slots
                    .get(&(std::ptr::from_ref(run), slot))
                    .copied()
                    .flatten();
                for mut child in stored_note_cache_runs(
                    child,
                    run.properties.as_ref().or(inherited),
                    view,
                    child_source,
                    locked,
                    input,
                    true,
                )? {
                    child.force_underline |= projected.force_underline;
                    child.force_strike |= projected.force_strike;
                    child.revision = child.revision.or(projected.revision);
                    result.push(child);
                }
            } else {
                segment.content.push(content.clone());
            }
        }
        if !segment.content.is_empty() || !segment.extra_xml.is_empty() {
            result.push(StoredFieldRun {
                occurrence,
                run: segment,
                force_underline: projected.force_underline,
                force_strike: projected.force_strike,
                revision: projected.revision,
                source,
                field_kind,
            });
        }
    }
    if result.is_empty() && source.is_some() {
        let mut run = CT_R::new("");
        run.properties = inherited.cloned();
        result.push(StoredFieldRun {
            occurrence,
            run,
            force_underline: false,
            force_strike: false,
            revision: None,
            source,
            field_kind,
        });
    }
    Ok(result)
}

fn append_cached_note_contents(
    field: &oxoffice_docx_oxml::text::Field,
    view: RevisionView,
    contents: &mut Vec<RunContent>,
) -> Result<()> {
    for stored in stored_note_cache_runs(field, None, view, None, false, None, false)? {
        contents.extend(stored.run.content.into_iter().filter(|content| {
            matches!(
                content,
                RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. }
            )
        }));
    }
    Ok(())
}

pub(super) fn content_note_references(
    content: &RunContent,
    view: RevisionView,
) -> Result<Vec<RunContent>> {
    let mut contents = Vec::new();
    match content {
        RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. } => {
            contents.push(content.clone())
        }
        RunContent::Field(field) => append_cached_note_contents(field, view, &mut contents)?,
        _ => {}
    }
    Ok(contents)
}

pub(super) fn paragraph_note_references(
    paragraph: &CT_P,
    view: RevisionView,
) -> Result<Vec<NoteRef>> {
    let mut result = Vec::new();
    for projected in project_paragraph_runs(paragraph, view) {
        for content in &projected.run.content {
            for content in content_note_references(content, view)? {
                match content {
                    RunContent::FootnoteRef { id, .. } => result.push(NoteRef {
                        stream: NoteStream::Footnote,
                        id,
                    }),
                    RunContent::EndnoteRef { id, .. } => result.push(NoteRef {
                        stream: NoteStream::Endnote,
                        id,
                    }),
                    _ => {}
                }
            }
        }
    }
    Ok(result)
}

pub(super) fn body_note_references(input: &LayoutInput) -> Result<Vec<NoteRef>> {
    let mut result = Vec::new();
    for content in &input.document.body.content {
        if let BodyContent::Paragraph(paragraph) = content {
            result.extend(paragraph_note_references(paragraph, input.revision_view)?);
        }
    }
    Ok(result)
}
