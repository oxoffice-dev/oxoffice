//! Source snapshots: the original XML of every compared owner, and its publication.

use super::*;

impl SourceSnapshots {
    fn inherit_owner_path(&mut self, parent: usize, child: usize, role: u8, index: usize) {
        let mut path = self.owner_paths.get(&parent).cloned().unwrap_or_default();
        path.push((role, index));
        self.owner_paths.insert(child, path);
        if self.unproved_result_owners.contains(&parent) {
            self.unproved_result_owners.insert(child);
        }
        if let Some(ancestors) = self.result_ancestors.get(&parent).cloned() {
            self.result_ancestors.insert(child, ancestors);
        }
    }

    fn inherit_part_origin(
        &mut self,
        parent: usize,
        child: usize,
        source: &str,
        child_start: usize,
    ) -> Result<()> {
        let Some((base, opening)) = self.part_origins.get(&parent).copied() else {
            return Ok(());
        };
        let (raw, _) = self.roots.get(&parent).ok_or_else(|| {
            Error::Other("physical child origin has no actual parent source".into())
        })?;
        let source_opening = element_opening(source)?.0;
        let raw_opening = element_opening(raw)?.0;
        if source[source_opening..] != raw[raw_opening..] {
            return Err(Error::Other(
                "physical parent closure changed descendant source".into(),
            ));
        }
        let growth = source_opening
            .checked_sub(raw_opening)
            .ok_or_else(|| Error::Other("physical parent closure shortened its opening".into()))?;
        let start = child_start
            .checked_sub(growth)
            .and_then(|start| start.checked_sub(opening))
            .and_then(|start| base.checked_add(start))
            .ok_or_else(|| Error::Other("physical child part origin overflow".into()))?;
        self.part_origins.insert(child, (start, 0));
        Ok(())
    }

    fn capture_field_occurrence(
        &mut self,
        owner: usize,
        field: &oxoffice_docx_oxml::text::Field,
        slot: usize,
        source: &str,
        bounds: Range<usize>,
        scope: NamespaceBindings,
    ) -> Result<()> {
        if self.unproved_result_owners.contains(&owner) {
            return Ok(());
        }
        if bounds.end > source.len() || bounds.start >= bounds.end {
            return Err(Error::Other(
                "field occurrence has unproved actual source bounds".into(),
            ));
        }
        let part_span = self.part_origins.get(&owner).and_then(|(base, opening)| {
            let offset = self.budget_offsets.get(&owner).copied().unwrap_or(0);
            let start = bounds
                .start
                .checked_add(offset)?
                .checked_sub(*opening)?
                .checked_add(*base)?;
            let end = bounds
                .end
                .checked_add(offset)?
                .checked_sub(*opening)?
                .checked_add(*base)?;
            let mut delta = 0i128;
            for (physical, projected) in &self.part_projection {
                if start < projected.end && end > projected.start {
                    return None;
                }
                if projected.end <= start {
                    delta = delta.checked_add(physical.len() as i128 - projected.len() as i128)?;
                }
            }
            let start = usize::try_from((start as i128).checked_add(delta)?).ok()?;
            let end = usize::try_from((end as i128).checked_add(delta)?).ok()?;
            (start < end && self.part_len.is_some_and(|length| end <= length)).then_some(start..end)
        });
        let actual_source = source[bounds.clone()].to_owned();
        let mut hole_path = self.owner_paths.get(&owner).cloned().unwrap_or_default();
        hole_path.push((11, slot));
        let offset = self.budget_offsets.get(&owner).copied().unwrap_or(0);
        let hole = bounds
            .start
            .checked_add(offset)
            .zip(bounds.end.checked_add(offset))
            .map(|(start, end)| start..end)
            .ok_or_else(|| Error::Other("source-budget field interval overflow".into()))?;
        self.budget_holes
            .entry(owner)
            .or_default()
            .push((hole_path, 11, hole, scope.clone()));
        let bounds = if let Some((result, opening)) = self.result_origins.get(&owner) {
            let start = bounds
                .start
                .checked_sub(*opening)
                .and_then(|offset| result.start.checked_add(offset));
            let end = bounds
                .end
                .checked_sub(*opening)
                .and_then(|offset| result.start.checked_add(offset));
            start
                .zip(end)
                .filter(|(start, end)| *start >= result.start && *end <= result.end)
                .map(|(start, end)| start..end)
                .ok_or_else(|| {
                    Error::Other(
                        "nested Field occurrence leaves its actual outer result interval".into(),
                    )
                })?
        } else {
            bounds
        };
        self.field_occurrences.insert(
            std::ptr::from_ref(field) as usize,
            FieldSourceOccurrence {
                input_role: self.input_role,
                story: self.story.clone(),
                owner_path: self.owner_paths.get(&owner).cloned().unwrap_or_default(),
                field_slot: slot,
                result_budget: None,
                carrier_budget: None,
                part_span,
                part_hash: self.part_hash,
                result_ancestors: self
                    .result_ancestors
                    .get(&owner)
                    .cloned()
                    .unwrap_or_default(),
                source: actual_source,
                bounds,
                scope,
            },
        );
        let occurrence = self
            .field_occurrences
            .get(&(std::ptr::from_ref(field) as usize))
            .cloned()
            .expect("captured field occurrence");
        self.capture_result_descendants(field, &occurrence)?;
        Ok(())
    }

    fn graph_runs(
        &mut self,
        runs: &[&CT_R],
        graphs: &mut Vec<(String, String)>,
        nested_fields: bool,
    ) -> Result<()> {
        for run in runs {
            for content in &run.content {
                match content {
                    RunContent::Drawing(drawing) => {
                        let slot = graphs.len();
                        if let Some(inline) = &drawing.inline {
                            if !inline.embed_id.is_empty() {
                                graphs.push((
                                    format!("drawing[{slot}]/inline/image"),
                                    inline.embed_id.clone(),
                                ));
                            }
                            if let Some(id) = &inline.link_id {
                                graphs.push((format!("drawing[{slot}]/inline/link"), id.clone()));
                            }
                            if let Some(id) = &inline.chart_rel_id {
                                graphs.push((format!("drawing[{slot}]/inline/chart"), id.clone()));
                            }
                        }
                        if let Some(anchor) = &drawing.anchor {
                            if !anchor.embed_id.is_empty() {
                                graphs.push((
                                    format!("drawing[{slot}]/anchor/image"),
                                    anchor.embed_id.clone(),
                                ));
                            }
                            if let Some(id) = &anchor.link_id {
                                graphs.push((format!("drawing[{slot}]/anchor/link"), id.clone()));
                            }
                            if let Some(id) = &anchor.chart_rel_id {
                                graphs.push((format!("drawing[{slot}]/anchor/chart"), id.clone()));
                            }
                            if let Some(body) = anchor
                                .shape
                                .as_ref()
                                .and_then(|shape| shape.text_body.as_ref())
                            {
                                self.graph_body(&body.content, graphs)?;
                            }
                        }
                    }
                    RunContent::Field(field) if nested_fields => {
                        self.graph_tree(field)?;
                        fn append(
                            field: &oxoffice_docx_oxml::text::Field,
                            sources: &SourceSnapshots,
                            prefix: &str,
                            graphs: &mut Vec<(String, String)>,
                        ) {
                            if let Some(owned) = sources
                                .field_graphs
                                .get(&(std::ptr::from_ref(field) as usize))
                            {
                                graphs.extend(
                                    owned
                                        .iter()
                                        .map(|(path, id)| (format!("{prefix}/{path}"), id.clone())),
                                );
                            }
                            for (slot, child) in field
                                .nested_fields_in_source_order()
                                .into_iter()
                                .enumerate()
                            {
                                append(
                                    child,
                                    sources,
                                    &format!("{prefix}/instruction[{slot}]"),
                                    graphs,
                                );
                            }
                            for (slot, child) in field
                                .cached_fields_in_source_order()
                                .into_iter()
                                .enumerate()
                            {
                                append(child, sources, &format!("{prefix}/result[{slot}]"), graphs);
                            }
                        }
                        let slot = graphs.len();
                        append(field, self, &format!("textbox-field[{slot}]"), graphs);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn graph_links(&self, paragraph: &CT_P, graphs: &mut Vec<(String, String)>) {
        use oxoffice_docx_oxml::text::{AcceptedRunPathSegment, hyperlink_revision_index};
        let paths = paragraph.accepted_run_paths();
        for (slot, link) in paragraph.hyperlinks.iter().enumerate() {
            let live = paths.iter().any(|path| match path.segments().first() {
                Some(AcceptedRunPathSegment::Run(index)) => {
                    link.run_start <= *index && *index < link.run_end
                }
                Some(AcceptedRunPathSegment::Revision(index)) => paragraph
                    .revisions
                    .get(*index)
                    .is_some_and(|(_, encoded, _)| {
                        hyperlink_revision_index(*encoded) == Some(slot)
                    }),
                _ => false,
            });
            if live && let Some(id) = &link.rel_id {
                graphs.push((format!("hyperlink[{}]", graphs.len()), id.clone()));
            }
        }
        for (_, _, revision) in &paragraph.revisions {
            if matches!(
                revision.kind(),
                oxoffice_docx_oxml::revision::RevisionKind::Insertion
                    | oxoffice_docx_oxml::revision::RevisionKind::MoveTo
            ) && let Some(owner) = revision.content_paragraph()
            {
                self.graph_links(owner, graphs);
            }
        }
        for (_, _, _, control) in &paragraph.content_controls {
            self.graph_control_links(control, graphs);
        }
    }

    fn graph_control_links(&self, control: &CT_Sdt, graphs: &mut Vec<(String, String)>) {
        for child in &control.content {
            match child {
                SdtContent::Paragraph(paragraph) => self.graph_links(paragraph, graphs),
                SdtContent::ContentControl(control) => self.graph_control_links(control, graphs),
                _ => {}
            }
        }
        for (_, revision) in control.revisions() {
            if matches!(
                revision.kind(),
                oxoffice_docx_oxml::revision::RevisionKind::Insertion
                    | oxoffice_docx_oxml::revision::RevisionKind::MoveTo
            ) && let Some(owner) = revision.content_paragraph()
            {
                self.graph_links(owner, graphs);
            }
        }
    }

    fn graph_paragraph(
        &mut self,
        paragraph: &CT_P,
        graphs: &mut Vec<(String, String)>,
        nested_fields: bool,
    ) -> Result<()> {
        let runs = paragraph
            .accepted_run_paths()
            .iter()
            .filter_map(|path| paragraph.accepted_run(path))
            .collect::<Vec<_>>();
        self.graph_runs(&runs, graphs, nested_fields)?;
        self.graph_links(paragraph, graphs);
        Ok(())
    }

    fn graph_cell(&mut self, cell: &CT_Tc, graphs: &mut Vec<(String, String)>) -> Result<()> {
        for child in &cell.content {
            match child {
                CellContent::Paragraph(paragraph) => {
                    self.graph_paragraph(paragraph, graphs, true)?
                }
                CellContent::Table(table) => self.graph_table(table, graphs)?,
                CellContent::ContentControl(control) => self.graph_control(control, graphs)?,
            }
        }
        Ok(())
    }

    fn graph_table(&mut self, table: &CT_Tbl, graphs: &mut Vec<(String, String)>) -> Result<()> {
        for row in &table.rows {
            for cell in &row.cells {
                self.graph_cell(cell, graphs)?;
            }
            for (_, _, control) in &row.content_controls {
                self.graph_control(control, graphs)?;
            }
        }
        for (_, _, control) in &table.content_controls {
            self.graph_control(control, graphs)?;
        }
        Ok(())
    }

    fn graph_control(
        &mut self,
        control: &CT_Sdt,
        graphs: &mut Vec<(String, String)>,
    ) -> Result<()> {
        for (slot, child) in control.content.iter().enumerate() {
            match child {
                SdtContent::Run(run) => self.graph_runs(&[run], graphs, true)?,
                SdtContent::Paragraph(paragraph) => {
                    self.graph_paragraph(paragraph, graphs, true)?
                }
                SdtContent::Table(table) => self.graph_table(table, graphs)?,
                SdtContent::Row(row) => {
                    for cell in &row.cells {
                        self.graph_cell(cell, graphs)?;
                    }
                }
                SdtContent::Cell(cell) => self.graph_cell(cell, graphs)?,
                SdtContent::ContentControl(control) => self.graph_control(control, graphs)?,
                SdtContent::RawXml(_) => {
                    if let Some((_, revision)) =
                        control.revisions().iter().find(|(at, _)| *at == slot)
                        && matches!(
                            revision.kind(),
                            oxoffice_docx_oxml::revision::RevisionKind::Insertion
                                | oxoffice_docx_oxml::revision::RevisionKind::MoveTo
                        )
                        && let Some(owner) = revision.content_paragraph()
                    {
                        self.graph_paragraph(owner, graphs, true)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn graph_body(
        &mut self,
        content: &[BodyContent],
        graphs: &mut Vec<(String, String)>,
    ) -> Result<()> {
        for child in content {
            match child {
                BodyContent::Paragraph(paragraph) => {
                    self.graph_paragraph(paragraph, graphs, true)?
                }
                BodyContent::Table(table) => self.graph_table(table, graphs)?,
                BodyContent::ContentControl(control) => self.graph_control(control, graphs)?,
                BodyContent::RawXml(_) => {}
            }
        }
        Ok(())
    }

    fn graph_owner(&mut self, field: &oxoffice_docx_oxml::text::Field) -> Result<Option<CT_P>> {
        let key = std::ptr::from_ref(field) as usize;
        if self.field_graphs.contains_key(&key) {
            return Ok(None);
        }
        self.field_graphs.insert(key, Vec::new());
        let owner = field.cached_result_owner_projection()?;
        let mut graphs = Vec::new();
        if let Some(owner) = &owner {
            self.graph_paragraph(owner, &mut graphs, false)?;
        } else if let Some(runs) = field.cached_result_physical_runs() {
            self.graph_runs(&runs.iter().collect::<Vec<_>>(), &mut graphs, false)?;
        }
        self.field_graphs.insert(key, graphs);
        for child in field.nested_fields_in_source_order() {
            self.graph_tree(child)?;
        }
        Ok(owner)
    }

    fn graph_tree(&mut self, field: &oxoffice_docx_oxml::text::Field) -> Result<()> {
        if self
            .field_graphs
            .contains_key(&(std::ptr::from_ref(field) as usize))
        {
            return Ok(());
        }
        let _owner = self.graph_owner(field)?;
        for child in field.cached_fields_in_source_order() {
            self.graph_tree(child)?;
        }
        Ok(())
    }

    fn capture_result_descendants(
        &mut self,
        field: &oxoffice_docx_oxml::text::Field,
        outer: &FieldSourceOccurrence,
    ) -> Result<()> {
        let graph_owner = if self.guard_graphs {
            self.graph_owner(field)?
        } else {
            None
        };
        if field.cached_fields_in_source_order().is_empty() {
            return Ok(());
        }
        let Ok(interval) = actual_field_result_range(&outer.source, &outer.scope) else {
            // Keep the existing strict outer source/semantic guard for legacy
            // unproved geometry. It gains no nested conversion authority.
            if self.guard_graphs {
                for child in field.cached_fields_in_source_order() {
                    self.graph_tree(child)?;
                }
            }
            return Ok(());
        };
        let result = &outer.source[interval.clone()];
        let prepared = if let Some(owner) = graph_owner {
            owner
        } else {
            prepared_field_result_owner(field, result)?
        };
        let source = field_result_source_paragraph(result, &outer.scope)?;
        let key = std::ptr::from_ref(&prepared) as usize;
        let mut frame = SourceSnapshots {
            guard_graphs: self.guard_graphs,
            input_role: outer.input_role,
            story: outer.story.clone(),
            ..Default::default()
        };
        let mut path = outer.owner_path.clone();
        path.push((7, outer.field_slot));
        frame.owner_paths.insert(key, path);
        frame.part_hash = outer.part_hash;
        frame.part_len = self.part_len;
        if let Some(base) = outer
            .part_span
            .as_ref()
            .and_then(|span| span.start.checked_add(interval.start))
        {
            frame
                .part_origins
                .insert(key, (base, element_opening(&source)?.0));
        }
        let mut ancestors = outer.result_ancestors.clone();
        ancestors.push(interval.clone());
        frame.result_ancestors.insert(key, ancestors);
        frame
            .result_origins
            .insert(key, (interval, element_opening(&source)?.0));
        frame
            .roots
            .insert(key, (source.clone(), outer.scope.clone()));
        frame.paragraph(&prepared, &source)?;
        let budget = frame.result_source_budget()?;
        let carrier = field_carrier_source_budget(&outer.source, &outer.scope)?;
        if let Some(occurrence) = self
            .field_occurrences
            .get_mut(&(std::ptr::from_ref(field) as usize))
        {
            occurrence.result_budget = budget;
            occurrence.carrier_budget = Some(carrier);
        }
        let prepared_fields = prepared
            .accepted_run_paths()
            .iter()
            .filter_map(|path| prepared.accepted_run(path))
            .flat_map(|run| &run.content)
            .filter_map(|content| {
                if let RunContent::Field(field) = content {
                    Some(field)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let originals = field.cached_fields_in_source_order();
        if originals.len() != prepared_fields.len() {
            return Err(Error::Other(
                "prepared result changed actual accepted child slots".into(),
            ));
        }
        fn bind(
            original: &oxoffice_docx_oxml::text::Field,
            prepared: &oxoffice_docx_oxml::text::Field,
            frame: &SourceSnapshots,
            output: &mut SourceSnapshots,
        ) -> Result<()> {
            if original.is_complex() != prepared.is_complex()
                || original.instruction.name != prepared.instruction.name
                || original.cached_result != prepared.cached_result
            {
                return Err(Error::Other(
                    "prepared result child slot changed its actual source kind or current value"
                        .into(),
                ));
            }
            let from = std::ptr::from_ref(prepared) as usize;
            let to = std::ptr::from_ref(original) as usize;
            let Some(occurrence) = frame.field_occurrences.get(&from).cloned() else {
                return Ok(());
            };
            output.field_occurrences.insert(to, occurrence);
            if let Some(graphs) = frame.field_graphs.get(&from) {
                output.field_graphs.insert(to, graphs.clone());
            }
            for (original, prepared) in original
                .nested_fields_in_source_order()
                .into_iter()
                .zip(prepared.nested_fields_in_source_order())
            {
                fn instruction_graphs(
                    original: &oxoffice_docx_oxml::text::Field,
                    prepared: &oxoffice_docx_oxml::text::Field,
                    frame: &SourceSnapshots,
                    output: &mut SourceSnapshots,
                ) {
                    if let Some(graphs) = frame
                        .field_graphs
                        .get(&(std::ptr::from_ref(prepared) as usize))
                    {
                        output
                            .field_graphs
                            .insert(std::ptr::from_ref(original) as usize, graphs.clone());
                    }
                    for (original, prepared) in original
                        .nested_fields_in_source_order()
                        .into_iter()
                        .zip(prepared.nested_fields_in_source_order())
                    {
                        instruction_graphs(original, prepared, frame, output);
                    }
                    for (original, prepared) in original
                        .cached_fields_in_source_order()
                        .into_iter()
                        .zip(prepared.cached_fields_in_source_order())
                    {
                        instruction_graphs(original, prepared, frame, output);
                    }
                }
                instruction_graphs(original, prepared, frame, output);
            }
            if let Some(scope) = frame.field_scopes.get(&from) {
                output.field_scopes.insert(to, scope.clone());
            }
            if let Some(facts) = frame.fields.get(&from) {
                output.fields.insert(to, facts.clone());
            }
            let left = original.cached_fields_in_source_order();
            let right = prepared.cached_fields_in_source_order();
            if left.len() != right.len() {
                return Err(Error::Other(
                    "prepared nested child slots changed cardinality".into(),
                ));
            }
            for (original, prepared) in left.into_iter().zip(right) {
                bind(original, prepared, frame, output)?;
            }
            Ok(())
        }
        for (original, prepared) in originals.into_iter().zip(prepared_fields) {
            bind(original, prepared, &frame, self)?;
        }
        Ok(())
    }

    fn result_source_budget(&self) -> Result<Option<SourceBudget>> {
        let mut output = Vec::new();
        for (key, (source, scope)) in &self.budget_sources {
            let Some(path) = self.owner_paths.get(key) else {
                return Ok(None);
            };
            let (lexical, bindings) = raw_occurrence_representation(source, scope)?;
            let growth = element_opening(source)?
                .0
                .checked_sub(element_opening(&lexical)?.0)
                .ok_or_else(|| {
                    Error::Other("source-budget namespace projection grew its opening".into())
                })?;
            let mut holes = self.budget_holes.get(key).cloned().unwrap_or_default();
            holes.sort_by_key(|(_, _, interval, _)| (interval.start, interval.end));
            let mut facts = vec![format!(
                "scope:{:?}",
                conversion_namespace_facts(bindings, source)?
            )];
            let mut cursor = 0usize;
            let mut seen = HashSet::new();
            for (ordinal, (child, kind, interval, context)) in holes.into_iter().enumerate() {
                if interval.start >= interval.end
                    || interval.end > source.len()
                    || !seen.insert((child.clone(), kind))
                {
                    return Ok(None);
                }
                let start = interval.start.checked_sub(growth);
                let end = interval.end.checked_sub(growth);
                let Some((start, end)) = start
                    .zip(end)
                    .filter(|(start, end)| *start >= cursor && *end <= lexical.len())
                else {
                    return Ok(None);
                };
                if kind != 11
                    && !self
                        .budget_sources
                        .keys()
                        .any(|key| self.owner_paths.get(key) == Some(&child))
                {
                    return Ok(None);
                }
                facts.push(format!("outside:{:?}", &lexical[cursor..start]));
                facts.push(format!(
                    "hole:{ordinal}:{child:?}:{kind}:{:?}",
                    conversion_namespace_facts(context, &source[interval.clone()])?
                ));
                cursor = end;
            }
            facts.push(format!("outside:{:?}", &lexical[cursor..]));
            output.push((path.clone(), facts));
        }
        output.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(Some(output))
    }

    fn revision_result(
        &mut self,
        revision: &oxoffice_docx_oxml::revision::CT_Revision,
        raw: &[u8],
        parent: usize,
        ordinal: usize,
        inherited: &NamespaceBindings,
    ) -> Result<()> {
        if !matches!(
            revision.kind(),
            oxoffice_docx_oxml::revision::RevisionKind::Insertion
                | oxoffice_docx_oxml::revision::RevisionKind::MoveTo
        ) {
            return Ok(());
        }
        let Some(paragraph) = revision.content_paragraph() else {
            return Ok(());
        };
        let raw = std::str::from_utf8(raw).map_err(utf8_error)?;
        let scope = actual_root_bindings(raw, inherited)?;
        let local = element_local_name(raw);
        let inner = element_inner_range_any_prefix(raw, local)?;
        let source = field_result_source_paragraph(&raw[inner.clone()], &scope)?;
        let key = std::ptr::from_ref(paragraph) as usize;
        self.inherit_owner_path(parent, key, 10, ordinal);
        self.result_origins
            .insert(key, (inner, element_opening(&source)?.0));
        self.roots.insert(key, (source.clone(), scope));
        self.paragraph(paragraph, &source)
    }

    pub(super) fn body(
        &mut self,
        document: &CT_Document,
        source: OriginalBodySource<'_>,
        held: &HeldRevisions,
    ) -> Result<()> {
        let (xml, spans, bindings) = source;
        for (index, (content, span)) in document.body.content.iter().zip(spans).enumerate() {
            if held.holds(Some(index)) {
                continue;
            }
            let key = match content {
                BodyContent::Paragraph(p) => p.as_ref() as *const _ as usize,
                BodyContent::Table(t) => t.as_ref() as *const _ as usize,
                BodyContent::ContentControl(c) => c.as_ref() as *const _ as usize,
                BodyContent::RawXml(_) => continue,
            };
            self.owner_paths.insert(key, vec![(0, index)]);
            if let Some(base) = self.part_base.and_then(|base| base.checked_add(span.start)) {
                self.part_origins.insert(key, (base, 0));
            }
            self.roots
                .insert(key, (xml[span.clone()].to_owned(), bindings.to_vec()));
            self.content(content, &source_fragment(&xml[span.clone()], bindings)?)?;
        }
        Ok(())
    }

    fn content(&mut self, content: &BodyContent, xml: &str) -> Result<()> {
        match content {
            BodyContent::Paragraph(p) => self.paragraph(p, xml),
            BodyContent::Table(t) => self.table(t, xml),
            BodyContent::ContentControl(c) => self.control(c, xml),
            BodyContent::RawXml(_) => Ok(()),
        }
    }

    pub(super) fn paragraph(&mut self, paragraph: &CT_P, xml: &str) -> Result<()> {
        self.paragraphs
            .insert(paragraph as *const CT_P as usize, xml.to_owned());
        let actual_parent_scope = self
            .roots
            .get(&(std::ptr::from_ref(paragraph) as usize))
            .map(|(raw, inherited)| actual_root_bindings(raw, inherited))
            .transpose()?
            .unwrap_or(actual_root_bindings(xml, &Vec::new())?);

        let (opening, closing) = paragraph_shell(Some(xml))?;
        let mut shared_fields = HashSet::new();
        let mut group_roots = Vec::new();
        let mut shared_index = 0;
        while shared_index < paragraph.runs.len() {
            if let Some((coverage, _)) = paragraph.field_source_run_fragments_at(shared_index)? {
                let mut owners = Vec::new();
                if paragraph.field_source_replacements_at(shared_index, &mut owners)? != coverage
                    || owners.len() != 1
                {
                    return Err(Error::Other(
                        "shared field root coverage is unproved".into(),
                    ));
                }
                let raw = std::str::from_utf8(&owners[0].0).map_err(utf8_error)?;
                let parent = format!("{opening}{raw}{closing}");
                let mut roots = Vec::new();
                for span in paragraph_run_spans(&parent)? {
                    roots.extend(source_root_metadata(
                        &source_child(&parent, span)?,
                        &Vec::new(),
                    )?);
                }
                if !roots.is_empty() {
                    group_roots.extend(roots);
                    for run in &paragraph.runs[shared_index..shared_index + coverage] {
                        self.shared_root_runs.insert(run as *const CT_R as usize);
                        for content in &run.content {
                            if let RunContent::Field(field) = content {
                                shared_fields.insert(field as *const _ as usize);
                            }
                        }
                    }
                }
                shared_index += coverage;
            } else {
                shared_index += 1;
            }
        }
        if !group_roots.is_empty() {
            self.shared_root_facts
                .insert(paragraph as *const CT_P as usize, group_roots);
        }
        let mut root_reader = Reader::from_str(&opening);
        let Event::Start(mut root) = root_reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        else {
            unreachable!()
        };
        let namespaces = root
            .attributes()
            .filter_map(|attribute| match attribute {
                Ok(attribute)
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:") =>
                {
                    Some(Ok((
                        attribute.key.as_ref().to_vec(),
                        attribute.value.as_ref().to_vec(),
                    )))
                }
                Ok(_) => None,
                Err(error) => Some(Err(Error::Other(error.to_string()))),
            })
            .collect::<Result<Vec<_>>>()?;
        root.clear_attributes();
        for (key, value) in &namespaces {
            root.push_attribute((key.as_slice(), value.as_slice()));
        }
        let mut parent_writer = Writer::new(Vec::new());
        parent_writer.write_event(Event::Start(root))?;
        let opening = String::from_utf8(parent_writer.into_inner()).map_err(utf8_error)?;
        let spans = modeled_paragraph_run_spans(paragraph, xml)?;
        let mut field_spans = spans.clone();
        // Legacy rendering keeps zero-length continuation placeholders. A source
        // witness instead retains the proven whole physical span at every actual
        // logical slot of that same shared group.
        let mut source_index = 0usize;
        while source_index < paragraph.runs.len() {
            if let Some((coverage, _)) = paragraph.field_source_run_fragments_at(source_index)? {
                let first = field_spans
                    .get(source_index)
                    .ok_or_else(|| {
                        Error::Other("shared field occurrence has no source start".into())
                    })?
                    .start;
                let end = field_spans
                    .get(source_index..source_index + coverage)
                    .and_then(|spans| spans.iter().map(|span| span.end).max())
                    .ok_or_else(|| {
                        Error::Other("shared field occurrence has no physical extent".into())
                    })?;
                if first >= end {
                    return Err(Error::Other("shared field physical extent is empty".into()));
                }
                for span in &mut field_spans[source_index..source_index + coverage] {
                    *span = first..end;
                }
                source_index += coverage;
            } else {
                source_index += 1;
            }
        }
        let physical_owner = self
            .roots
            .get(&(std::ptr::from_ref(paragraph) as usize))
            .map(|(raw, _)| raw.clone())
            .unwrap_or_else(|| xml.to_owned());
        self.budget_sources.insert(
            std::ptr::from_ref(paragraph) as usize,
            (physical_owner.clone(), actual_parent_scope.clone()),
        );
        let closure_growth = element_opening(xml)?
            .0
            .checked_sub(element_opening(&physical_owner)?.0)
            .ok_or_else(|| {
                Error::Other("field owner namespace closure changed its source boundary".into())
            })?;
        // One read of the paragraph answers the scope of every field.
        let field_starts = paragraph
            .runs
            .iter()
            .enumerate()
            .flat_map(|(run_index, run)| {
                run.content
                    .iter()
                    .filter(|content| matches!(content, RunContent::Field(_)))
                    .map(move |_| run_index)
            })
            .map_while(|run_index| field_spans.get(run_index).map(|span| span.start))
            .collect::<Vec<_>>();
        let mut field_parents = publication_parents_at(xml, &field_starts).into_iter();
        let mut field_slot = 0usize;
        for (run_index, run) in paragraph.runs.iter().enumerate() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    let span = field_spans.get(run_index).ok_or_else(|| {
                        Error::Other("field source run occurrence is missing".into())
                    })?;
                    let physical_span = span
                        .start
                        .checked_sub(closure_growth)
                        .zip(span.end.checked_sub(closure_growth))
                        .map(|(start, end)| start..end)
                        .ok_or_else(|| {
                            Error::Other("field source occurrence precedes its actual owner".into())
                        })?;
                    let actual_field_source = &physical_owner[physical_span.clone()];
                    let actual_scope = actual_root_bindings(
                        actual_field_source,
                        &field_parents.next().expect("one scope per field")?,
                    )?;
                    self.capture_field_occurrence(
                        std::ptr::from_ref(paragraph) as usize,
                        field,
                        field_slot,
                        &physical_owner,
                        physical_span,
                        actual_scope,
                    )?;
                    field_slot += 1;
                    let raw = if let Some(raw) = field.detached_source()? {
                        raw
                    } else {
                        field
                            .source_replacement()?
                            .map(|(raw, _)| raw.to_vec())
                            .unwrap_or_else(|| actual_field_source.as_bytes().to_vec())
                    };
                    let raw = std::str::from_utf8(&raw).map_err(utf8_error)?;
                    let scope = actual_root_bindings(raw, &actual_parent_scope)?;
                    self.field_scopes
                        .insert(std::ptr::from_ref(field) as usize, scope);
                    let view = source_without_text_properties(
                        &format!("{opening}{raw}{closing}"),
                        &Vec::new(),
                    )?;
                    let view = if shared_fields.contains(&(field as *const _ as usize)) {
                        source_without_run_root_attributes(&view)?
                    } else {
                        view
                    };
                    let facts = field_source_metadata(&view, field.is_complex())?;
                    self.fields.insert(field as *const _ as usize, facts);
                }
            }
        }

        if !self.properties_only {
            let (opening, closing) = paragraph_shell(Some(xml))?;
            // The runs the paragraph source holds as they are are captured in
            // one read after this walk. Each was read in turn before, so an
            // error capturing one still comes before any later error.
            let mut plain = Vec::new();
            let mut failure = None;
            let mut index = 0;
            'runs: while index < paragraph.runs.len() {
                let fragments = match paragraph.field_source_run_fragments_at(index) {
                    Ok(fragments) => fragments,
                    Err(error) => {
                        failure = Some(Error::from(error));
                        break;
                    }
                };
                if let Some((coverage, fragments)) = fragments {
                    for (run, fragment) in paragraph.runs[index..index + coverage]
                        .iter()
                        .zip(fragments)
                    {
                        if let Some(raw) = fragment {
                            let source =
                                String::from_utf8(raw).map_err(utf8_error).and_then(|raw| {
                                    let wrapped = format!("{opening}{raw}{closing}");
                                    let span = opening.len()..opening.len() + raw.len();
                                    RunSource::capture_all(&[(Some(run), span)], &wrapped).remove(0)
                                });
                            match source {
                                Ok(source) => {
                                    self.runs.insert(run as *const CT_R as usize, source);
                                }
                                Err(error) => {
                                    failure = Some(error);
                                    break 'runs;
                                }
                            }
                        }
                    }
                    index += coverage;
                    continue;
                }
                let run = &paragraph.runs[index];
                if !run_is_field(run) {
                    plain.push((Some(run), spans[index].clone()));
                }
                index += 1;
            }
            for ((run, _), source) in plain.iter().zip(RunSource::capture_all(&plain, xml)) {
                if let Some(run) = run {
                    self.runs.insert(*run as *const CT_R as usize, source?);
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
        }
        let controls = direct_word_element_spans(xml, "sdt")?;
        let mut scopes = child_scopes(xml, &controls);
        for (index, ((_, _, _, control), span)) in
            paragraph.content_controls.iter().zip(controls).enumerate()
        {
            let (parent, source) = scopes.next().expect("one scope per control span");
            self.inherit_owner_path(
                std::ptr::from_ref(paragraph) as usize,
                std::ptr::from_ref(control) as usize,
                1,
                index,
            );
            self.inherit_part_origin(
                std::ptr::from_ref(paragraph) as usize,
                std::ptr::from_ref(control) as usize,
                xml,
                span.start,
            )?;
            let path = self
                .owner_paths
                .get(&(std::ptr::from_ref(control) as usize))
                .cloned()
                .unwrap_or_default();
            let hole = span
                .start
                .checked_sub(closure_growth)
                .zip(span.end.checked_sub(closure_growth))
                .map(|(start, end)| start..end)
                .ok_or_else(|| {
                    Error::Other("control budget interval precedes physical owner".into())
                })?;
            let parent = parent?;
            self.budget_holes
                .entry(std::ptr::from_ref(paragraph) as usize)
                .or_default()
                .push((
                    path,
                    1,
                    hole,
                    actual_root_bindings(&xml[span.clone()], &parent)?,
                ));
            self.roots.insert(
                control as *const CT_Sdt as usize,
                (xml[span.clone()].to_owned(), parent),
            );
            self.control(control, &source?)?;
        }
        for (ordinal, (at, raw_before, revision)) in paragraph.revisions.iter().enumerate() {
            let raw = paragraph
                .extra_xml
                .iter()
                .filter(|(boundary, _)| boundary == at)
                .nth(*raw_before)
                .map(|(_, raw)| raw.as_slice())
                .ok_or_else(|| {
                    Error::Other("canonical revision has no actual owning raw slot".into())
                })?;
            self.revision_result(
                revision,
                raw,
                std::ptr::from_ref(paragraph) as usize,
                ordinal,
                &actual_parent_scope,
            )?;
        }
        Ok(())
    }

    fn table(&mut self, table: &CT_Tbl, xml: &str) -> Result<()> {
        self.tables
            .insert(table as *const CT_Tbl as usize, xml.to_owned());
        let rows = direct_word_element_spans(xml, "tr")?;
        let mut scopes = child_scopes(xml, &rows);
        for (index, (row, span)) in table.rows.iter().zip(rows).enumerate() {
            let (parent, source) = scopes.next().expect("one scope per row span");
            self.inherit_owner_path(
                std::ptr::from_ref(table) as usize,
                std::ptr::from_ref(row) as usize,
                2,
                index,
            );
            self.inherit_part_origin(
                std::ptr::from_ref(table) as usize,
                std::ptr::from_ref(row) as usize,
                xml,
                span.start,
            )?;
            self.roots.insert(
                row as *const CT_Row as usize,
                (xml[span].to_owned(), parent?),
            );
            self.row(row, &source?)?;
        }
        let controls = direct_word_element_spans(xml, "sdt")?;
        let mut scopes = child_scopes(xml, &controls);
        for (index, ((_, _, control), span)) in
            table.content_controls.iter().zip(controls).enumerate()
        {
            let (parent, source) = scopes.next().expect("one scope per control span");
            self.inherit_owner_path(
                std::ptr::from_ref(table) as usize,
                std::ptr::from_ref(control) as usize,
                4,
                index,
            );
            self.inherit_part_origin(
                std::ptr::from_ref(table) as usize,
                std::ptr::from_ref(control) as usize,
                xml,
                span.start,
            )?;
            self.roots.insert(
                control as *const CT_Sdt as usize,
                (xml[span].to_owned(), parent?),
            );
            self.control(control, &source?)?;
        }
        Ok(())
    }

    fn row(&mut self, row: &CT_Row, xml: &str) -> Result<()> {
        self.rows
            .insert(row as *const CT_Row as usize, xml.to_owned());
        let cells = direct_word_element_spans(xml, "tc")?;
        let mut scopes = child_scopes(xml, &cells);
        for (index, (cell, span)) in row.cells.iter().zip(cells).enumerate() {
            let (parent, source) = scopes.next().expect("one scope per cell span");
            self.inherit_owner_path(
                std::ptr::from_ref(row) as usize,
                std::ptr::from_ref(cell) as usize,
                3,
                index,
            );
            self.inherit_part_origin(
                std::ptr::from_ref(row) as usize,
                std::ptr::from_ref(cell) as usize,
                xml,
                span.start,
            )?;
            self.roots.insert(
                cell as *const CT_Tc as usize,
                (xml[span].to_owned(), parent?),
            );
            self.cell(cell, &source?)?;
        }
        let controls = direct_word_element_spans(xml, "sdt")?;
        let mut scopes = child_scopes(xml, &controls);
        for (index, ((_, _, control), span)) in
            row.content_controls.iter().zip(controls).enumerate()
        {
            let (parent, source) = scopes.next().expect("one scope per control span");
            self.inherit_owner_path(
                std::ptr::from_ref(row) as usize,
                std::ptr::from_ref(control) as usize,
                4,
                index,
            );
            self.inherit_part_origin(
                std::ptr::from_ref(row) as usize,
                std::ptr::from_ref(control) as usize,
                xml,
                span.start,
            )?;
            self.roots.insert(
                control as *const CT_Sdt as usize,
                (xml[span].to_owned(), parent?),
            );
            self.control(control, &source?)?;
        }
        Ok(())
    }

    fn cell(&mut self, cell: &CT_Tc, xml: &str) -> Result<()> {
        self.cells
            .insert(cell as *const CT_Tc as usize, xml.to_owned());
        let children = cell_child_spans(cell, xml, "source snapshot")?
            .into_iter()
            .map(|(_, span)| span)
            .collect::<Vec<_>>();
        let mut scopes = child_scopes(xml, &children);
        for (index, (child, span)) in cell.content.iter().zip(children).enumerate() {
            let (parent, source) = scopes.next().expect("one scope per cell child span");
            let child_key = match child {
                CellContent::Paragraph(p) => std::ptr::from_ref(p) as usize,
                CellContent::Table(t) => std::ptr::from_ref(t) as usize,
                CellContent::ContentControl(c) => std::ptr::from_ref(c) as usize,
            };
            self.inherit_owner_path(std::ptr::from_ref(cell) as usize, child_key, 5, index);
            self.inherit_part_origin(
                std::ptr::from_ref(cell) as usize,
                child_key,
                xml,
                span.start,
            )?;
            let source = source?;
            match child {
                CellContent::Paragraph(p) => {
                    self.roots
                        .insert(p as *const CT_P as usize, (xml[span].to_owned(), parent?));
                    self.paragraph(p, &source)?
                }
                CellContent::Table(t) => {
                    self.roots
                        .insert(t as *const CT_Tbl as usize, (xml[span].to_owned(), parent?));
                    self.table(t, &source)?
                }
                CellContent::ContentControl(c) => {
                    self.roots
                        .insert(c as *const CT_Sdt as usize, (xml[span].to_owned(), parent?));
                    self.control(c, &source)?
                }
            }
        }
        Ok(())
    }

    fn control(&mut self, control: &CT_Sdt, xml: &str) -> Result<()> {
        self.controls
            .insert(control as *const CT_Sdt as usize, xml.to_owned());
        let (budget_source, budget_scope) = self
            .roots
            .get(&(std::ptr::from_ref(control) as usize))
            .cloned()
            .unwrap_or_else(|| (xml.to_owned(), Vec::new()));
        let budget_scope = actual_root_bindings(&budget_source, &budget_scope)?;
        self.budget_sources.insert(
            std::ptr::from_ref(control) as usize,
            (budget_source, budget_scope),
        );
        let content = modeled_control_content(control);
        if content.is_empty() {
            return Ok(());
        }
        let spans = direct_word_element_spans(xml, "sdtContent")?;
        let span = spans
            .first()
            .ok_or_else(|| Error::Other("source control has no content".into()))?;
        let payload = source_child(xml, span.clone())?;
        let (raw_control, parent_scope) = self
            .roots
            .get(&(std::ptr::from_ref(control) as usize))
            .map(|(raw, scope)| (raw.clone(), scope.clone()))
            .unwrap_or_else(|| (xml.to_owned(), Vec::new()));
        let control_scope = actual_root_bindings(&raw_control, &parent_scope)?;
        let raw_content =
            direct_word_element_spans_in_scope(&raw_control, "sdtContent", &control_scope)?;
        if raw_content.len() != 1 {
            return Err(Error::Other(
                "control has no unique actual physical content owner".into(),
            ));
        }
        let raw_payload = &raw_control[raw_content[0].clone()];
        self.budget_offsets
            .insert(std::ptr::from_ref(control) as usize, raw_content[0].start);
        let closure_growth = element_opening(&payload)?
            .0
            .checked_sub(element_opening(raw_payload)?.0)
            .ok_or_else(|| {
                Error::Other("control content closure shortened its physical opening".into())
            })?;
        if payload[element_opening(&payload)?.0..] != raw_payload[element_opening(raw_payload)?.0..]
        {
            return Err(Error::Other(
                "control content closure changed physical descendants".into(),
            ));
        }
        let spans = modeled_control_source_spans(&content, &payload)?;
        // One read of the content answers every child. Each answer, an error
        // included, is met where a read for that child alone was made.
        let elements = content
            .iter()
            .zip(&spans)
            .enumerate()
            .filter(|(_, (child, _))| !matches!(child, SdtContent::RawXml(_)))
            .map(|(index, (_, span))| (index, span.clone()))
            .collect::<Vec<_>>();
        let element_spans = elements
            .iter()
            .map(|(_, span)| span.clone())
            .collect::<Vec<_>>();
        let element_starts = element_spans
            .iter()
            .map(|span| span.start)
            .collect::<Vec<_>>();
        let mut child_sources = content.iter().map(|_| None).collect::<Vec<_>>();
        let mut child_parents = content.iter().map(|_| None).collect::<Vec<_>>();
        for (((index, _), source), parent) in elements
            .iter()
            .zip(source_children(&payload, &element_spans))
            .zip(publication_parents_at(&payload, &element_starts))
        {
            child_sources[*index] = Some(source);
            child_parents[*index] = Some(parent);
        }
        let mut captured = content.iter().map(|_| None).collect::<Vec<_>>();
        if !self.properties_only {
            let plain = content
                .iter()
                .zip(&spans)
                .enumerate()
                .filter_map(|(index, (child, span))| match child {
                    SdtContent::Run(run)
                        if !run
                            .content
                            .iter()
                            .any(|content| matches!(content, RunContent::Field(_))) =>
                    {
                        Some((index, (Some(run), span.clone())))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let runs = plain.iter().map(|(_, run)| run.clone()).collect::<Vec<_>>();
            for ((index, _), source) in plain.iter().zip(RunSource::capture_all(&runs, &payload)) {
                captured[*index] = Some(source);
            }
        }
        let mut field_slot = 0usize;
        for (index, (child, span)) in content.into_iter().zip(spans).enumerate() {
            let child_key = match child {
                SdtContent::Paragraph(p) => Some(std::ptr::from_ref(p) as usize),
                SdtContent::Table(t) => Some(std::ptr::from_ref(t) as usize),
                SdtContent::Row(r) => Some(std::ptr::from_ref(r) as usize),
                SdtContent::Cell(c) => Some(std::ptr::from_ref(c) as usize),
                SdtContent::ContentControl(c) => Some(std::ptr::from_ref(c) as usize),
                _ => None,
            };
            if let Some(child_key) = child_key {
                self.inherit_owner_path(std::ptr::from_ref(control) as usize, child_key, 6, index);
                let start = span
                    .start
                    .checked_sub(closure_growth)
                    .and_then(|start| raw_content[0].start.checked_add(start))
                    .ok_or_else(|| {
                        Error::Other("control child source coordinate overflow".into())
                    })?;
                self.inherit_part_origin(
                    std::ptr::from_ref(control) as usize,
                    child_key,
                    &raw_control,
                    start,
                )?;
            }
            // Raw events participate in correlation but already own their bytes.
            // PI and comment events have no element opening to namespace-close.
            if matches!(child, SdtContent::RawXml(_)) {
                continue;
            }
            let source = child_sources[index]
                .take()
                .expect("an element child has its source")?;
            let mut parent = || {
                child_parents[index]
                    .take()
                    .expect("an element child has its parent scope")
            };
            match child {
                SdtContent::Paragraph(p) => {
                    self.roots.insert(
                        p as *const CT_P as usize,
                        (payload[span.clone()].to_owned(), parent()?),
                    );
                    self.paragraph(p, &source)?;
                }
                SdtContent::Table(t) => {
                    self.roots.insert(
                        t as *const CT_Tbl as usize,
                        (payload[span.clone()].to_owned(), parent()?),
                    );
                    self.table(t, &source)?;
                }
                SdtContent::Row(r) => {
                    self.roots.insert(
                        r as *const CT_Row as usize,
                        (payload[span.clone()].to_owned(), parent()?),
                    );
                    self.row(r, &source)?;
                }
                SdtContent::Cell(c) => {
                    self.roots.insert(
                        c as *const CT_Tc as usize,
                        (payload[span.clone()].to_owned(), parent()?),
                    );
                    self.cell(c, &source)?;
                }
                SdtContent::ContentControl(c) => {
                    let path = self
                        .owner_paths
                        .get(&(std::ptr::from_ref(c) as usize))
                        .cloned()
                        .unwrap_or_default();
                    let start = span
                        .start
                        .checked_sub(closure_growth)
                        .and_then(|offset| raw_content[0].start.checked_add(offset));
                    let end = span
                        .end
                        .checked_sub(closure_growth)
                        .and_then(|offset| raw_content[0].start.checked_add(offset));
                    let hole = start
                        .zip(end)
                        .map(|(start, end)| start..end)
                        .ok_or_else(|| {
                            Error::Other("nested control source-budget interval overflow".into())
                        })?;
                    let scope = parent()?;
                    self.budget_holes
                        .entry(std::ptr::from_ref(control) as usize)
                        .or_default()
                        .push((path, 6, hole, actual_root_bindings(&source, &scope)?));
                    self.roots.insert(
                        c as *const CT_Sdt as usize,
                        (payload[span.clone()].to_owned(), scope),
                    );
                    self.control(c, &source)?;
                }
                SdtContent::Run(r) => {
                    let fields = r
                        .content
                        .iter()
                        .filter_map(|content| match content {
                            RunContent::Field(field) => Some(field),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if fields.is_empty()
                        && let Some(source) = captured[index].take()
                    {
                        self.runs.insert(r as *const CT_R as usize, source?);
                    }
                    for field in fields {
                        let scope = actual_root_bindings(
                            &source,
                            &actual_root_bindings(&payload, &Vec::new())?,
                        )?;
                        self.capture_field_occurrence(
                            std::ptr::from_ref(control) as usize,
                            field,
                            field_slot,
                            raw_payload,
                            span.start
                                .checked_sub(closure_growth)
                                .zip(span.end.checked_sub(closure_growth))
                                .map(|(start, end)| start..end)
                                .ok_or_else(|| {
                                    Error::Other(
                                        "control field source precedes its actual content owner"
                                            .into(),
                                    )
                                })?,
                            scope.clone(),
                        )?;
                        field_slot += 1;
                        self.field_scopes
                            .insert(std::ptr::from_ref(field) as usize, scope.clone());
                        self.fields.insert(
                            std::ptr::from_ref(field) as usize,
                            field_source_metadata(
                                &source_without_text_properties(
                                    &field_result_source_paragraph(&source, &scope)?,
                                    &Vec::new(),
                                )?,
                                field.is_complex(),
                            )?,
                        );
                    }
                }
                SdtContent::RawXml(_) => {}
            }
        }
        let content_scope = actual_root_bindings(raw_payload, &control_scope)?;
        for (ordinal, (slot, revision)) in control.revisions().iter().enumerate() {
            let Some(SdtContent::RawXml(raw)) = control.content.get(*slot) else {
                return Err(Error::Other(
                    "control revision has no actual raw content slot".into(),
                ));
            };
            self.revision_result(
                revision,
                raw,
                std::ptr::from_ref(control) as usize,
                ordinal,
                &content_scope,
            )?;
        }
        Ok(())
    }
}

pub(super) struct FieldCloneBinding {
    frame: usize,
    key: usize,
    occurrence: Option<FieldSourceOccurrence>,
    scope: Option<NamespaceBindings>,
    facts: Option<Vec<String>>,
}

impl Metadata<'_> {
    fn bind_field_clone(
        &mut self,
        original: &oxoffice_docx_oxml::text::Field,
        cloned: &oxoffice_docx_oxml::text::Field,
        bindings: &mut Vec<FieldCloneBinding>,
    ) -> Result<()> {
        if std::ptr::eq(original, cloned) {
            return Ok(());
        }
        if original != cloned
            || original.form() != cloned.form()
            || original.source_owner_id() != cloned.source_owner_id()
        {
            return Err(Error::Other(
                "field clone changed its actual owner facts".into(),
            ));
        }
        let old_key = std::ptr::from_ref(original) as usize;
        let new_key = std::ptr::from_ref(cloned) as usize;
        let frame = self
            .source_frames
            .iter()
            .rposition(|frame| frame.field_occurrences.contains_key(&old_key))
            .ok_or_else(|| Error::Other("field clone has no actual original occurrence".into()))?;
        let data = &mut self.source_frames[frame];
        let actual = data.field_occurrences[&old_key].clone();
        let scope = data.field_scopes.get(&old_key).cloned();
        let facts = data.fields.get(&old_key).cloned();
        bindings.push(FieldCloneBinding {
            frame,
            key: new_key,
            occurrence: data.field_occurrences.insert(new_key, actual),
            scope: scope.and_then(|scope| data.field_scopes.insert(new_key, scope)),
            facts: facts.and_then(|facts| data.fields.insert(new_key, facts)),
        });
        let original = original.cached_fields_in_source_order();
        let cloned = cloned.cached_fields_in_source_order();
        if original.len() != cloned.len() {
            return Err(Error::Other(
                "field clone changed its actual result slots".into(),
            ));
        }
        for (original, cloned) in original.into_iter().zip(cloned) {
            // Unproved descendants gain no authority from the enclosing clone.
            if self.source_frames.iter().any(|frame| {
                frame
                    .field_occurrences
                    .contains_key(&(std::ptr::from_ref(original) as usize))
            }) {
                self.bind_field_clone(original, cloned, bindings)?;
            }
        }
        Ok(())
    }

    pub(super) fn bind_selected_run_fields(
        &mut self,
        original: &CT_R,
        cloned: &CT_R,
        selected: &[Option<TypedSourceSelection>],
    ) -> Result<Vec<FieldCloneBinding>> {
        let mut bindings = Vec::new();
        let result = (|| -> Result<()> {
            if cloned.content.len() != selected.len() {
                return Err(Error::Other(
                    "cloned selected run changed content slots".into(),
                ));
            }
            for (content, selected) in cloned.content.iter().zip(selected) {
                let RunContent::Field(cloned) = content else {
                    continue;
                };
                let selected = selected
                    .filter(|selected| selected.scalars.is_none())
                    .ok_or_else(|| {
                        Error::Other("cloned Field has no complete actual content slot".into())
                    })?;
                let Some(RunContent::Field(original)) = original.content.get(selected.ordinal)
                else {
                    return Err(Error::Other(
                        "cloned Field changed its actual source slot kind".into(),
                    ));
                };
                self.bind_field_clone(original, cloned, &mut bindings)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.unbind_cloned_control_runs(bindings)?;
            return Err(error);
        }
        Ok(bindings)
    }

    pub(super) fn bind_cloned_control_runs(
        &mut self,
        content: &[&SdtContent],
        cloned: &[CT_R],
    ) -> Result<Vec<FieldCloneBinding>> {
        let original = content
            .iter()
            .filter_map(|content| match content {
                SdtContent::Run(run) => Some(run),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut bindings = Vec::new();
        let result = (|| -> Result<()> {
            if original.len() != cloned.len() {
                return Err(Error::Other("control run clone cardinality changed".into()));
            }
            for (original, cloned) in original.into_iter().zip(cloned) {
                if original.content.len() != cloned.content.len() {
                    return Err(Error::Other(
                        "control run clone content slots changed".into(),
                    ));
                }
                for (old, new) in original.content.iter().zip(&cloned.content) {
                    if let (RunContent::Field(old), RunContent::Field(new)) = (old, new) {
                        self.bind_field_clone(old, new, &mut bindings)?;
                    } else if matches!(old, RunContent::Field(_))
                        || matches!(new, RunContent::Field(_))
                    {
                        return Err(Error::Other(
                            "control clone changed its actual field slot kind".into(),
                        ));
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.unbind_cloned_control_runs(bindings)?;
            return Err(error);
        }
        Ok(bindings)
    }

    pub(super) fn unbind_cloned_control_runs(
        &mut self,
        bindings: Vec<FieldCloneBinding>,
    ) -> Result<()> {
        for binding in bindings.into_iter().rev() {
            let frame = self.source_frames.get_mut(binding.frame).ok_or_else(|| {
                Error::Other("control field clone source frame disappeared".into())
            })?;
            match binding.occurrence {
                Some(value) => {
                    frame.field_occurrences.insert(binding.key, value);
                }
                None => {
                    frame.field_occurrences.remove(&binding.key);
                }
            }
            match binding.scope {
                Some(value) => {
                    frame.field_scopes.insert(binding.key, value);
                }
                None => {
                    frame.field_scopes.remove(&binding.key);
                }
            }
            match binding.facts {
                Some(value) => {
                    frame.fields.insert(binding.key, value);
                }
                None => {
                    frame.fields.remove(&binding.key);
                }
            }
        }
        Ok(())
    }

    pub(super) fn restore_owner_root(&self, owner: usize, output: String) -> Result<String> {
        let actual = self
            .source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.roots.get(&owner));
        let Some((raw, parent)) = actual else {
            return Ok(output);
        };
        // A paired publisher can emit multiple tracked owners for boundary
        // changes. Those do not have one retained root to restore here.
        let (wrapped, offset) = story_wrapper(&output, parent)?;
        let mut reader = Reader::from_reader(wrapped.as_slice());
        let mut depth = 0usize;
        let mut roots = 0usize;
        loop {
            let before = reader.buffer_position() as usize;
            match reader
                .read_event()
                .map_err(|error| Error::Other(error.to_string()))?
            {
                Event::Start(_) => {
                    if before >= offset && depth == 2 {
                        roots += 1;
                    }
                    depth += 1;
                }
                Event::Empty(_) => {
                    if before >= offset && depth == 2 {
                        roots += 1;
                    }
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Eof => break,
                _ => {}
            }
        }
        if roots != 1 {
            return Ok(output);
        }
        restore_extracted_root(&output, raw, parent)
    }

    pub(super) fn paragraph_source(&self, paragraph: &CT_P) -> Option<&str> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.paragraphs.get(&(paragraph as *const CT_P as usize)))
            .map(String::as_str)
    }
    pub(super) fn field_source_facts(
        &self,
        field: &oxoffice_docx_oxml::text::Field,
    ) -> Option<&Vec<String>> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.fields.get(&(std::ptr::from_ref(field) as usize)))
    }
    pub(super) fn run_source(&self, run: &CT_R) -> Option<&RunSource> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.runs.get(&(run as *const CT_R as usize)))
    }
    pub(super) fn table_source(&self, table: &CT_Tbl) -> Option<&str> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.tables.get(&(table as *const CT_Tbl as usize)))
            .map(String::as_str)
    }
    pub(super) fn row_source(&self, row: &CT_Row) -> Option<&str> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.rows.get(&(row as *const CT_Row as usize)))
            .map(String::as_str)
    }
    pub(super) fn cell_source(&self, cell: &CT_Tc) -> Option<&str> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.cells.get(&(cell as *const CT_Tc as usize)))
            .map(String::as_str)
    }
    pub(super) fn control_source(&self, control: &CT_Sdt) -> Option<&str> {
        self.source_frames
            .iter()
            .rev()
            .find_map(|frame| frame.controls.get(&(control as *const CT_Sdt as usize)))
            .map(String::as_str)
    }
}
