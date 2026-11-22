//! The field model: Field and its instruction, cached results and their validation.

use super::*;

impl Field {
    /// Construct a field that will be written in the simple field form.
    pub fn new(instruction: &str, cached_result: &str) -> Self {
        let instruction = parse_field_instruction(instruction);
        Self {
            template_word_receipt: None,
            source: Arc::new(FieldSource::New {
                original_instruction: instruction.clone(),
                form: FieldForm::Simple,
                cached_runs: None,
                original_cached_result: cached_result.to_owned(),
                cached_segments: Vec::new(),
            }),
            instruction,
            cached_result: cached_result.to_owned(),
            dirty: None,
            locked: None,
            legacy_form: None,
            legacy_form_parse_error: false,
            legacy_reset_default: None,
            legacy_text_default: None,
            legacy_result_published: false,
            derived_parent_cache: cached_result.to_owned(),
            legacy_author: None,
            nested_order: Vec::new(),
            cached_fields: Vec::new(),
            simple_cached_runs: None,
            typed_cached_runs: None,
            checked_payload_source_bytes: (0, 0),
            typed_cached_comment_ranges: Vec::new(),
            parsed_cached_comment_ranges: Vec::new(),
            parsed_cached_runs: None,
            ordered_result: None,
            span: None,
        }
    }

    /// Construct a checked field from a producer-style instruction.
    pub fn from_raw(instruction: &str, form: FieldForm, cached_runs: Vec<CT_R>) -> Result<Self> {
        validate_raw_field_instruction(instruction)?;
        let parsed = parse_field_instruction(instruction);
        let mut field = Self::from_instruction(parsed, form, cached_runs)?;
        field.instruction.raw = instruction.trim().to_owned();
        if let FieldSource::New {
            original_instruction,
            ..
        } = Arc::make_mut(&mut field.source)
        {
            *original_instruction = field.instruction.clone();
        }
        Ok(field)
    }

    /// Construct a checked field with typed operands and ordered cached runs.
    pub fn from_instruction(
        instruction: FieldInstruction,
        form: FieldForm,
        cached_runs: Vec<CT_R>,
    ) -> Result<Self> {
        let instruction = FieldInstruction::new(
            &instruction.name,
            instruction.arguments,
            instruction.switches,
        )?;
        if form == FieldForm::Simple && instruction_contains_nested(&instruction) {
            return Err(OxmlError::InvalidValue(
                "simple field instructions cannot contain nested fields".into(),
            ));
        }
        let source_bytes = validate_cached_field_runs(&cached_runs)?;
        let cached_segments = cached_runs
            .iter()
            .enumerate()
            .map(|(run_index, run)| CachedDisplaySegment {
                run_index,
                text: cached_run_display(run),
                properties: run.properties.clone(),
            })
            .collect::<Vec<_>>();
        let cached_result = cached_segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>();
        let mut field = Self::new(&instruction.raw, &cached_result);
        field.instruction = instruction.clone();
        field.checked_payload_source_bytes.0 = source_bytes;
        field.source = Arc::new(FieldSource::New {
            original_instruction: instruction,
            form,
            cached_runs: Some(cached_runs),
            original_cached_result: cached_result,
            cached_segments,
        });
        Ok(field)
    }

    /// Author one complex legacy form field with a result region.
    pub fn from_legacy_form(
        options: LegacyFormFieldOptions,
        definition: LegacyFormFieldDefinition,
    ) -> Result<Self> {
        validate_legacy_form_authoring(&options, &definition)?;
        let (instruction_name, display, data, author, reset_default, text_default) =
            legacy_form_author_parts(&options, &definition)?;
        let instruction = FieldInstruction::new(instruction_name, Vec::new(), Vec::new())?;
        let mut field =
            Self::from_instruction(instruction, FieldForm::Complex, vec![CT_R::new(&display)])?;
        field.legacy_form = Some(data);
        field.legacy_reset_default = reset_default;
        field.legacy_text_default = text_default;
        field.legacy_result_published = true;
        field.legacy_author = Some(author);
        field.validate_for_attachment()?;
        Ok(field)
    }

    /// Replace a field's result with checked typed runs while retaining its source controls.
    #[doc(hidden)]
    pub fn set_cached_runs(&mut self, runs: Vec<CT_R>) -> Result<()> {
        let source_bytes = self.checked_cached_run_replacement(&runs)?;
        let mut work = WordOperationWork::model_field(self)?;
        self.set_cached_runs_in(runs, source_bytes, &mut work)
    }

    fn checked_cached_run_replacement(&self, runs: &[CT_R]) -> Result<usize> {
        if self.locked == Some(true) {
            return Err(OxmlError::InvalidValue(
                "a locked field cannot replace its cached runs".into(),
            ));
        }
        let source_bytes = validate_cached_field_runs(runs)?;
        crate::document::admit_word_model(crate::document::WordOwner::Field(self))?;
        self.normalization_work_budget()?;
        Ok(source_bytes)
    }

    fn set_cached_runs_in(
        &mut self,
        runs: Vec<CT_R>,
        source_bytes: usize,
        work: &mut WordOperationWork,
    ) -> Result<()> {
        if self.ordered_result.is_some() {
            let original_context = match self.source.as_ref() {
                FieldSource::Parsed { word_prefixes, .. } => field_scope_bindings(word_prefixes),
                FieldSource::New { .. } => Vec::new(),
            };
            let source_context = Some(original_context.as_slice());
            let mut candidate = self.clone();
            let owner = candidate.ordered_result.as_mut().expect("retained result");
            replace_ordered_result_runs(Arc::make_mut(&mut owner.paragraph), runs, source_context)?;
            owner.pending = true;
            owner.pending_payload = true;
            owner.pending_payload_source_bytes = source_bytes;
            candidate.cached_result = owner.paragraph.accepted_text();
            candidate
                .derived_parent_cache
                .clone_from(&candidate.cached_result);
            candidate.normalize_cached_result_owner_in(work)?;
            *self = candidate;
            return Ok(());
        }
        let mut candidate = self.clone();
        candidate.cached_result = runs.iter().map(cached_run_display).collect();
        candidate.derived_parent_cache = candidate.cached_result.clone();
        candidate.typed_cached_runs = Some(runs);
        candidate.checked_payload_source_bytes.1 = source_bytes;
        if let Some(form) = candidate.legacy_form.as_mut()
            && form.kind == LegacyFormFieldKind::TextInput
        {
            form.value = LegacyFormFieldValue::Text(candidate.cached_result.clone());
            candidate.legacy_result_published = true;
        }
        candidate.typed_cached_comment_ranges.clear();
        candidate.simple_cached_runs = None;
        candidate.cached_fields.clear();
        let mut writer = Writer::new(Vec::new());
        write_run_field(&mut writer, &candidate, None, None, None, work)?;
        let source = writer.into_inner();
        let mut paragraph = FIELD_CACHE_VALIDATION_SCOPE.to_vec();
        paragraph.extend(source);
        paragraph.extend_from_slice(b"</w:p>");
        CT_P::from_xml_fragment(&paragraph)?;
        *self = candidate;
        Ok(())
    }

    /// Replace a checked result and its sibling annotation range markers atomically.
    #[doc(hidden)]
    pub fn set_cached_runs_with_comment_ranges(
        &mut self,
        runs: Vec<CT_R>,
        ranges: Vec<CommentRangeMarker>,
    ) -> Result<()> {
        validate_cached_comment_ranges(&runs, &ranges)?;
        if ranges.is_empty() {
            return self.set_cached_runs(runs);
        }
        let mut work = WordOperationWork::model_field(self)?;
        if self.ordered_result.is_some() {
            if self.locked == Some(true) {
                return Err(OxmlError::InvalidValue(
                    "a locked field cannot replace its cached runs".into(),
                ));
            }
            let source_bytes = validate_cached_field_runs(&runs)?
                .checked_add(
                    ranges
                        .len()
                        .checked_mul(crate::namespace::W_NS.len() + 64)
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("annotation source byte overflow".into())
                        })?,
                )
                .ok_or_else(|| OxmlError::InvalidValue("annotation source byte overflow".into()))?;
            crate::document::admit_word_model(crate::document::WordOwner::Field(self))?;
            self.normalization_work_budget()?;
            let original_context = match self.source.as_ref() {
                FieldSource::Parsed { word_prefixes, .. } => field_scope_bindings(word_prefixes),
                FieldSource::New { .. } => Vec::new(),
            };
            let source_context = Some(original_context.as_slice());
            let mut candidate = self.clone();
            let owner = candidate.ordered_result.as_mut().expect("retained result");
            let paragraph = Arc::make_mut(&mut owner.paragraph);
            let anchor = paragraph
                .accepted_run_paths()
                .first()
                .map(|path| path.segments().to_vec())
                .unwrap_or_else(|| vec![AcceptedRunPathSegment::Run(0)]);
            replace_ordered_result_runs(paragraph, runs, source_context)?;
            if !paragraph.insert_ordered_result_comment_ranges(&anchor, &ranges)? {
                return Err(OxmlError::InvalidValue(
                    "annotation replacement lost its first accepted owner".into(),
                ));
            }
            owner.pending = true;
            owner.pending_payload = true;
            owner.pending_payload_source_bytes = source_bytes;
            candidate.cached_result = owner.paragraph.accepted_text();
            candidate
                .derived_parent_cache
                .clone_from(&candidate.cached_result);
            candidate.normalize_cached_result_owner_in(&mut work)?;
            *self = candidate;
            return Ok(());
        }
        let mut candidate = self.clone();
        let source_bytes = candidate.checked_cached_run_replacement(&runs)?;
        candidate.set_cached_runs_in(runs, source_bytes, &mut work)?;
        candidate.typed_cached_comment_ranges = ranges;
        let mut writer = Writer::new(Vec::new());
        write_run_field(&mut writer, &candidate, None, None, None, &mut work)?;
        let source = writer.into_inner();
        candidate.checked_payload_source_bytes.1 = source.len();
        let mut paragraph = FIELD_CACHE_VALIDATION_SCOPE.to_vec();
        paragraph.extend(source);
        paragraph.extend_from_slice(b"</w:p>");
        let reparsed = CT_P::from_xml_fragment(&paragraph)?;
        let owner = reparsed
            .runs()
            .into_iter()
            .flat_map(|run| &run.content)
            .find_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
            .ok_or_else(|| {
                OxmlError::InvalidValue("annotation result did not reopen as a typed field".into())
            })?;
        if owner.effective_instruction() != candidate.effective_instruction()
            || owner.cached_result != candidate.cached_result
            || owner.cached_result_comment_ranges().len()
                != candidate.typed_cached_comment_ranges.len()
        {
            return Err(OxmlError::InvalidValue(
                "annotation result reopened with a different field owner or range graph".into(),
            ));
        }
        if let Some(runs) = owner.cached_result_runs() {
            validate_cached_comment_ranges(runs, owner.cached_result_comment_ranges())?;
        } else {
            return Err(OxmlError::InvalidValue(
                "annotation result has no typed cache after reopen".into(),
            ));
        }
        *self = candidate;
        Ok(())
    }

    /// Return sibling annotation boundaries owned by this field's stored result.
    #[doc(hidden)]
    pub fn cached_result_comment_ranges(&self) -> &[CommentRangeMarker] {
        if self.typed_cache().is_some() {
            &self.typed_cached_comment_ranges
        } else {
            &self.parsed_cached_comment_ranges
        }
    }

    /// Prepare an owned current result without publishing pending low-level edits.
    /// A direct cache assignment is pending until `normalize_cached_result_owner`.
    #[doc(hidden)]
    pub fn cached_result_owner_projection(&self) -> Result<Option<CT_P>> {
        if self.ordered_result.is_none() {
            return Ok(None);
        }
        crate::document::admit_word_model(crate::document::WordOwner::Field(self))?;
        self.normalization_work_budget()?;
        let mut work = WordOperationWork::model_field(self)?;
        let mut staged = match self.ordered_normalization_plan()? {
            Some(plan) => self.normalized_from_plan(plan, &mut work)?,
            None => self.clone(),
        };
        let owner = staged
            .ordered_result
            .take()
            .expect("prepared ordered result");
        crate::document::admit_word_model(crate::document::WordOwner::Paragraph(&owner.paragraph))?;
        let FieldSource::Parsed { word_prefixes, .. } = staged.source.as_ref() else {
            return Err(OxmlError::MissingElement(
                "prepared field source scope".into(),
            ));
        };
        crate::document::admit_word_xml(&owner.normalized_xml, word_prefixes, Some(b"p"))?;
        let mut paragraph = Arc::unwrap_or_clone(owner.paragraph);
        crate::revision::prepare_semantic_paragraph_projection(&mut paragraph)?;
        Ok(Some(paragraph))
    }

    /// Complete pending cached-child/cache assignments atomically on this field.
    /// For example, after `field.cached_result = value`, call this before requiring
    /// normalized borrowed revision/raw readers. Failure retains that pending value.
    #[doc(hidden)]
    pub fn normalize_cached_result_owner(&mut self) -> Result<()> {
        let Some(plan) = self.ordered_normalization_plan()? else {
            if let Some(owner) = &mut self.ordered_result {
                owner.pending = false;
            }
            return Ok(());
        };
        let mut work = WordOperationWork::model_field(self)?;
        let candidate = self.normalized_from_plan(plan, &mut work)?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn normalize_cached_result_owner_in(
        &mut self,
        work: &mut WordOperationWork,
    ) -> Result<()> {
        let Some(plan) = self.ordered_normalization_plan()? else {
            if let Some(owner) = &mut self.ordered_result {
                owner.pending = false;
            }
            return Ok(());
        };
        let candidate = self.normalized_from_plan(plan, work)?;
        *self = candidate;
        Ok(())
    }

    fn ordered_normalization_plan(&self) -> Result<Option<OrderedNormalizationPlan<'_>>> {
        let Some(stored) = &self.ordered_result else {
            return Ok(None);
        };
        crate::document::admit_word_model(crate::document::WordOwner::Field(self))?;
        let work = self.normalization_work_budget()?;
        let field_changes = stored.paragraph.has_changed_field_owners();
        let cache_override = self.cached_result != self.derived_parent_cache;
        if !field_changes
            && !cache_override
            && !stored.pending_payload
            && stored.paragraph.accepted_text() == stored.normalized_cache
        {
            return Ok(None);
        }
        self.validate_for_attachment()?;
        let source_context = match self.source.as_ref() {
            FieldSource::Parsed { word_prefixes, .. } => field_scope_bindings(word_prefixes),
            FieldSource::New { .. } => Vec::new(),
        };
        Ok(Some(OrderedNormalizationPlan {
            work,
            field_changes,
            cache_override,
            requested: &self.cached_result,
            source_context,
        }))
    }

    fn normalized_from_plan(
        &self,
        plan: OrderedNormalizationPlan<'_>,
        work: &mut WordOperationWork,
    ) -> Result<Self> {
        let normalization_work = plan.work;
        let field_changes = plan.field_changes;
        let source_context = Some(plan.source_context.as_slice());
        let mut candidate = self.clone();
        work.template_generated_field_cache(&mut candidate)?;
        let explicit_cache = plan.cache_override;
        let requested = plan.requested.to_owned();
        let _requested_input = work.template_field_scalar(self, &requested);
        let mut produced_texts = Vec::new();
        let owner = candidate.ordered_result.as_mut().expect("retained result");
        let paragraph = Arc::make_mut(&mut owner.paragraph);
        paragraph.normalize_field_owners_in(work)?;
        if explicit_cache && paragraph.accepted_text() != requested {
            replace_ordered_result_text(
                paragraph,
                &requested,
                source_context,
                normalization_work,
                work,
                Some(&mut produced_texts),
            )?;
            paragraph.normalize_field_owners_in(work)?;
        }
        let cache = paragraph.accepted_text();
        if explicit_cache && cache != requested {
            return Err(OxmlError::InvalidValue(
                "ordered field result did not publish the requested complete display".into(),
            ));
        }
        let changed = owner.pending_payload || field_changes || cache != owner.normalized_cache;
        if changed {
            let FieldSource::Parsed { word_prefixes, .. } = candidate.source.as_ref() else {
                return Err(OxmlError::MissingElement(
                    "ordered field source scope".into(),
                ));
            };
            let source_context = field_scope_bindings(word_prefixes);
            let capture = work.template_word_capture.clone();
            let text_lease =
                TemplateParagraphTextLease::new(paragraph, produced_texts, capture.as_ref());
            let (xml, output_map) = ordered_paragraph_mapped_xml_in(
                paragraph,
                Some(&source_context),
                work,
                Some(normalization_work),
            )?;
            drop(text_lease);
            if xml.len() > normalization_work {
                return Err(OxmlError::InvalidValue(
                    "normalized result exceeds its preflight source work".into(),
                ));
            }
            let FieldSource::Parsed { word_prefixes, .. } = candidate.source.as_ref() else {
                return Err(OxmlError::MissingElement(
                    "ordered field source scope".into(),
                ));
            };
            let mapped = crate::document::WordFieldInput::produced(
                output_map.clone(),
                Arc::clone(&work.field_origins),
            );
            let mut normalized = parse_ordered_field_result(&xml, word_prefixes, Some(&mapped))?;
            retain_ordered_field_source_identities_in(
                paragraph,
                &mut normalized,
                Some(&work.field_origins),
            )?;
            owner.paragraph = Arc::new(normalized);
            owner.normalized_xml = Arc::new(xml);
            work.field_origins
                .lock()
                .expect("field origin registry poisoned")
                .register_result(&owner.normalized_xml, output_map)?;
        }
        owner.pending = false;
        owner.pending_payload = false;
        owner.pending_payload_source_bytes = 0;
        owner.normalized_cache.clone_from(&cache);
        candidate.cached_result.clone_from(&cache);
        candidate.derived_parent_cache = cache;
        candidate.simple_cached_runs = None;
        candidate.parsed_cached_runs = None;
        candidate.cached_fields.clear();
        candidate.parsed_cached_comment_ranges.clear();
        owner
            .paragraph
            .append_ordered_comment_ranges(&mut 0, &mut candidate.parsed_cached_comment_ranges);
        Ok(candidate)
    }

    pub(super) fn normalization_work_budget(&self) -> Result<usize> {
        let (coefficient, offset) = self.normalization_work_summary()?;
        coefficient.checked_add(offset).ok_or_else(|| {
            OxmlError::InvalidValue("normalization affected-source work overflow".into())
        })
    }

    /// Exact work at ancestry a is coefficient * a + offset. An edge is
    /// charged on every occurrence, while its immutable child is summarized
    /// once per call using its actual allocation, never its shared source ID.
    pub(super) fn normalization_work_summary(&self) -> Result<(usize, usize)> {
        let mut summaries = std::collections::HashMap::<*const Field, (usize, usize)>::new();
        let mut pending = vec![(self, None)];
        while let Some((field, children)) = pending.pop() {
            let key = std::ptr::from_ref(field);
            if summaries.contains_key(&key) {
                continue;
            }
            let Some(children) = children else {
                let children = field.normalization_children()?;
                pending.push((field, Some(children.clone())));
                pending.extend(children.into_iter().rev().map(|(child, _)| (child, None)));
                continue;
            };
            let mut coefficient = field.normalization_local_work_bytes()?;
            let mut offset = 0usize;
            for (child, ancestry_delta) in children {
                // Fields own their operands and immutable result owners. Safe
                // construction cannot install a back edge into these owners.
                let (child_coefficient, child_offset) = summaries
                    .get(&std::ptr::from_ref(child))
                    .copied()
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("normalization source path is stale".into())
                    })?;
                coefficient = coefficient.checked_add(child_coefficient).ok_or_else(|| {
                    OxmlError::InvalidValue("normalization affected-source work overflow".into())
                })?;
                offset = child_coefficient
                    .checked_mul(ancestry_delta)
                    .and_then(|value| value.checked_add(child_offset))
                    .and_then(|value| value.checked_add(offset))
                    .ok_or_else(|| {
                        OxmlError::InvalidValue(
                            "normalization affected-source work overflow".into(),
                        )
                    })?;
            }
            summaries.insert(key, (coefficient, offset));
        }
        summaries
            .get(&std::ptr::from_ref(self))
            .copied()
            .ok_or_else(|| OxmlError::InvalidValue("normalization source path is stale".into()))
    }

    pub(super) fn normalization_input_work(&self) -> Result<NormalizationInputWork> {
        let (retained_source, scope) = match self.source.as_ref() {
            FieldSource::Parsed {
                raw_xml,
                word_prefixes,
                ..
            } => (raw_xml.len(), checked_field_scope_bytes(word_prefixes)?),
            FieldSource::New { .. } => (0, 0),
        };
        // Current public values may diverge from both retained source and
        // original input. Preserve their actual terms before projection/COW.
        let current = legacy_field_work_bytes(
            initial_field_work_bytes(&self.instruction, &self.cached_result, 0),
            self.legacy_form.as_ref(),
            self.legacy_reset_default.as_ref(),
            self.legacy_text_default.as_ref(),
            self.legacy_author.as_ref(),
        )
        .ok_or_else(|| {
            OxmlError::InvalidValue("normalization current-value byte overflow".into())
        })?;
        Ok(NormalizationInputWork {
            retained_source,
            scope,
            current,
            checked_payload: [
                self.checked_payload_source_bytes.0,
                self.checked_payload_source_bytes.1,
            ],
            stored_result: self
                .ordered_result
                .as_ref()
                .map_or(0, |owner| owner.normalized_xml.len()),
            pending_payload: self
                .ordered_result
                .as_ref()
                .map_or(0, |owner| owner.pending_payload_source_bytes),
        })
    }

    fn normalization_local_work_bytes(&self) -> Result<usize> {
        let inputs = self.normalization_input_work()?;
        let dynamic = inputs.dynamic_work().ok_or_else(|| {
            OxmlError::InvalidValue("normalization current-value byte overflow".into())
        })?;
        inputs
            .source_work(dynamic)
            .ok_or_else(|| OxmlError::InvalidValue("normalization source byte overflow".into()))
    }

    pub(super) fn completed_result_publication_summary(
        &self,
        paragraph: &CT_P,
        normalized_bytes: usize,
        cache: &str,
    ) -> Result<ParsedFieldSummary> {
        let (source_bytes, scope_bytes) = match self.source.as_ref() {
            FieldSource::Parsed {
                raw_xml,
                word_prefixes,
                ..
            } => (raw_xml.len(), checked_field_scope_bytes(word_prefixes)?),
            FieldSource::New { .. } => (0, 0),
        };
        let current = legacy_field_work_bytes(
            initial_field_work_bytes(&self.instruction, cache, scope_bytes),
            self.legacy_form.as_ref(),
            self.legacy_reset_default.as_ref(),
            self.legacy_text_default.as_ref(),
            self.legacy_author.as_ref(),
        );
        let local = current
            .and_then(affected_source_dynamic_work)
            .and_then(|bytes| bytes.checked_add(source_bytes))
            .and_then(|bytes| bytes.checked_add(self.checked_payload_source_bytes.0))
            .and_then(|bytes| bytes.checked_add(self.checked_payload_source_bytes.1))
            .and_then(|bytes| bytes.checked_add(normalized_bytes))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalization completed-result source overflow".into())
            })?;
        let parent_cost = usize::from(self.simple_source_form());
        let children = self.resource_owner_children_with_result(Some(paragraph), None);
        let maxima = crate::document::word_model_children_maxima(&children, parent_cost)?;
        let mut result = ParsedFieldSummary {
            affine: (local, 0),
            maxima: (
                crate::document::word_owner_cost(maxima.0, parent_cost)?,
                maxima.1,
            ),
        };
        for (child, delta) in ordered_normalization_children(paragraph)? {
            result.add_child(
                ParsedFieldSummary {
                    affine: child.normalization_work_summary()?,
                    maxima: (0, 0),
                },
                delta,
            )?;
        }
        for argument in self.instruction.arguments.iter().chain(
            self.instruction
                .switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            if let FieldArgument::Nested(child) = argument {
                result.add_child(
                    ParsedFieldSummary {
                        affine: child.normalization_work_summary()?,
                        maxima: (0, 0),
                    },
                    0,
                )?;
            }
        }
        result
            .affine
            .0
            .checked_add(result.affine.1)
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalization completed-result source overflow".into())
            })?;
        Ok(result)
    }

    pub(super) fn source_publication_summary(
        &self,
        raw_bytes: usize,
        span: Option<&[Option<CT_R>]>,
    ) -> Result<ParsedFieldSummary> {
        let mut affine = self.normalization_work_summary()?;
        if let FieldSource::Parsed { raw_xml, .. } = self.source.as_ref() {
            affine.0 = affine
                .0
                .checked_sub(raw_xml.len())
                .and_then(|work| work.checked_add(raw_bytes))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("normalization grouped source work overflow".into())
                })?;
        }
        affine.0.checked_add(affine.1).ok_or_else(|| {
            OxmlError::InvalidValue("normalization grouped source work overflow".into())
        })?;
        let parent_cost = usize::from(self.has_ordered_result_owner());
        let children = self.resource_owner_children_with_result(None, span);
        let maxima = crate::document::word_model_children_maxima(&children, parent_cost)?;
        Ok(ParsedFieldSummary {
            affine,
            maxima: (
                crate::document::word_owner_cost(maxima.0, parent_cost)?,
                maxima.1,
            ),
        })
    }

    pub(super) fn normalization_children(&self) -> Result<Vec<(&Field, usize)>> {
        let mut children = Vec::new();
        if let Some(owner) = &self.ordered_result {
            children.extend(ordered_normalization_children(&owner.paragraph)?);
        } else {
            children.extend(
                self.cached_fields_in_source_order()
                    .into_iter()
                    .map(|child| (child, 1)),
            );
        }
        for argument in self.instruction.arguments.iter().chain(
            self.instruction
                .switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            if let FieldArgument::Nested(child) = argument {
                children.push((child.as_ref(), 0));
            }
        }
        Ok(children)
    }

    pub(crate) fn source_has_pending_changes(&self) -> bool {
        !self.is_unchanged()
    }

    pub(crate) fn has_ordered_result_owner(&self) -> bool {
        self.ordered_result.is_some() && self.simple_source_form()
    }

    fn simple_source_form(&self) -> bool {
        matches!(
            self.source.as_ref(),
            FieldSource::Parsed {
                form: FieldForm::Simple,
                ..
            } | FieldSource::New {
                form: FieldForm::Simple,
                ..
            }
        )
    }

    /// Borrow the stored ordered paragraph without validating or projecting it.
    #[doc(hidden)]
    pub fn stored_ordered_result(&self) -> Option<&CT_P> {
        self.ordered_result
            .as_ref()
            .map(|owner| owner.paragraph.as_ref())
    }

    /// Return the ordered typed result runs without replaying cached fields.
    #[doc(hidden)]
    pub fn cached_result_runs(&self) -> Option<&[CT_R]> {
        if let Some(owner) = &self.ordered_result {
            return contiguous_ordered_result_runs(&owner.paragraph).filter(|runs| {
                runs.iter().map(cached_run_display).collect::<String>() == self.cached_result
            });
        }
        self.typed_cache()
            .or(self
                .simple_cached_runs
                .as_deref()
                .filter(|runs| cached_runs_need_typed_projection(runs)))
            .or(self
                .parsed_cached_runs
                .as_deref()
                .filter(|runs| cached_runs_need_typed_projection(runs)))
            .or(match self.source.as_ref() {
                FieldSource::New { cached_runs, .. } => cached_runs.as_deref(),
                _ => None,
            })
            .filter(|runs| {
                runs.iter().map(cached_run_display).collect::<String>() == self.cached_result
            })
    }

    /// Borrow current physical cache facts independently of legacy display text.
    /// Opaque paired elements and deletion text retain their original typed/raw
    /// representation. A scalar override cannot reuse the original cache facts.
    #[doc(hidden)]
    pub fn cached_result_physical_runs(&self) -> Option<&[CT_R]> {
        if self.ordered_result.is_some() {
            return self.cached_result_runs();
        }
        if let Some(runs) = self.typed_cache() {
            return Some(runs);
        }
        if self.cached_result != self.original_cached_result() {
            return None;
        }
        self.simple_cached_runs
            .as_deref()
            .or(self.parsed_cached_runs.as_deref())
            .or(match self.source.as_ref() {
                FieldSource::New { cached_runs, .. } => cached_runs.as_deref(),
                _ => None,
            })
    }

    pub(super) fn typed_cache(&self) -> Option<&[CT_R]> {
        self.typed_cached_runs
            .as_deref()
            .filter(|runs| cached_runs_display_eq(runs, &self.cached_result))
    }

    pub(super) fn new_result_runs(&self) -> Option<&[CT_R]> {
        let FieldSource::New {
            cached_runs: Some(runs),
            original_cached_result,
            ..
        } = self.source.as_ref()
        else {
            return None;
        };
        if self.cached_result == *original_cached_result {
            return Some(runs);
        }
        // Keep the actual projection precedence used by nested_cached_projection.
        // The ordered owner is not a flat run cache.
        if let Some(owner) = &self.ordered_result {
            return (owner.paragraph.accepted_text() == self.cached_result).then_some(runs);
        }
        let projected = self
            .typed_cached_runs
            .as_deref()
            .or(self.simple_cached_runs.as_deref().filter(|runs| {
                runs.iter()
                    .flat_map(|run| &run.content)
                    .any(|content| matches!(content, RunContent::Field(_)))
            }))
            .or(self.parsed_cached_child_runs())
            .unwrap_or(runs);
        cached_runs_display_eq(projected, &self.cached_result).then_some(runs)
    }

    /// Return the representation this field writes.
    pub fn form(&self) -> FieldForm {
        if instruction_contains_nested(&self.effective_instruction()) {
            return FieldForm::Complex;
        }
        match self.source.as_ref() {
            FieldSource::New { form, .. } | FieldSource::Parsed { form, .. } => *form,
        }
    }

    /// The field-local lock state, including an absent toggle.
    pub fn locked(&self) -> Option<bool> {
        self.locked
    }

    /// Set or remove the field-local lock without changing its instruction or cache.
    pub fn set_locked(&mut self, value: Option<bool>) {
        self.locked = value;
    }

    /// Validate the current mutable field before attaching it to a document.
    #[doc(hidden)]
    pub fn validate_for_attachment(&self) -> Result<()> {
        crate::document::admit_word_model(crate::document::WordOwner::Field(self))?;
        self.normalization_work_budget()?;
        // Imported programs can be opaque while their retained source is valid XML.
        // Select the emitted program before checking operands. Retired values
        // remain covered by the model and resource admission above.
        // Raw selection can trim whitespace during parsing. Validate the active
        // untrimmed raw value, while a structured edit retires that spelling.
        if instruction_structure_eq(&self.instruction, self.original_instruction()) {
            oxoffice_core::xml::reject_non_xml_characters(
                "field instruction",
                &self.instruction.raw,
            )?;
        }
        let instruction = self.effective_instruction();
        oxoffice_core::xml::reject_non_xml_characters("field instruction", &instruction.raw)?;
        for argument in instruction.arguments.iter().chain(
            instruction
                .switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            match argument {
                FieldArgument::Text(value) => {
                    oxoffice_core::xml::reject_non_xml_characters("field argument", value)?
                }
                FieldArgument::Nested(child) => child.validate_for_attachment()?,
            }
        }
        let preserved = matches!(self.source.as_ref(), FieldSource::Parsed { original_instruction, .. }
            if self.instruction.raw == original_instruction.raw
                && instruction_structure_eq(&self.instruction, original_instruction)
                && instruction_source_identity_eq(&self.instruction, original_instruction));
        if !preserved {
            validate_raw_field_instruction(&instruction.raw)?;
            validate_authored_instruction_parts(
                &instruction.name,
                &instruction.arguments,
                &instruction.switches,
            )?;
        }
        if let Some(runs) = self.typed_cache() {
            validate_cached_field_runs(runs)?;
        }
        if let FieldSource::New {
            cached_runs: Some(runs),
            ..
        } = self.source.as_ref()
        {
            validate_cached_field_runs(runs)?;
        }
        let original_cached_result = match self.source.as_ref() {
            FieldSource::New {
                original_cached_result,
                cached_runs: Some(_),
                ..
            }
            | FieldSource::Parsed {
                original_cached_result,
                ..
            } => Some(original_cached_result),
            _ => None,
        };
        if original_cached_result != Some(&self.cached_result) {
            oxoffice_core::xml::reject_non_xml_characters("field result", &self.cached_result)?;
        }
        if let Some(form) = &self.legacy_form {
            let instruction = self.effective_instruction();
            let matches_kind = matches!(
                (form.kind, instruction.name.as_str()),
                (LegacyFormFieldKind::TextInput, "FORMTEXT")
                    | (LegacyFormFieldKind::CheckBox, "FORMCHECKBOX")
                    | (LegacyFormFieldKind::DropDownList, "FORMDROPDOWN")
            );
            if !matches_kind {
                return Err(OxmlError::InvalidValue(
                    "legacy form metadata contradicts its field instruction".into(),
                ));
            }
            let kind_matches_value = matches!(
                (&form.kind, &form.value),
                (
                    LegacyFormFieldKind::TextInput,
                    LegacyFormFieldValue::Text(_)
                ) | (
                    LegacyFormFieldKind::CheckBox,
                    LegacyFormFieldValue::Checked(_)
                ) | (
                    LegacyFormFieldKind::DropDownList,
                    LegacyFormFieldValue::SelectedIndex(_)
                )
            );
            if !kind_matches_value {
                return Err(OxmlError::InvalidValue(
                    "legacy form value kind does not match the field kind".into(),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn parsed(
        instruction: FieldInstruction,
        cached_result: String,
        cached_segments: Vec<CachedDisplaySegment>,
        dirty: Option<bool>,
        form: FieldForm,
        source: (Vec<u8>, Vec<String>),
        metadata: Option<ParsedFieldMetadata>,
    ) -> Self {
        let (raw_xml, word_prefixes) = source;
        let locked = field_source_lock(&raw_xml, form, &word_prefixes)
            .ok()
            .flatten();
        let source_id = NEXT_FIELD_SOURCE_ID.fetch_add(1, Ordering::Relaxed);
        let original_instruction = instruction.clone();
        let original_cached_result = cached_result.clone();
        let metadata = metadata.unwrap_or_else(|| {
            ParsedFieldMetadata::read(
                crate::document::WordXmlInput::Contiguous(&raw_xml),
                form,
                &word_prefixes,
                &instruction.name,
                &cached_result,
            )
        });
        let ParsedFieldMetadata {
            legacy_form,
            reset_default: legacy_reset_default,
            text_default: legacy_text_default,
            parse_error: legacy_form_parse_error,
            result_published,
            form: metadata_form,
        } = metadata;
        debug_assert_eq!(metadata_form, form);
        let derived_parent_cache = cached_result.clone();
        Self {
            instruction,
            cached_result,
            dirty,
            locked,
            legacy_form: legacy_form.clone(),
            legacy_form_parse_error,
            legacy_reset_default,
            legacy_text_default,
            legacy_result_published: result_published,
            derived_parent_cache,
            legacy_author: None,
            nested_order: Vec::new(),
            cached_fields: Vec::new(),
            simple_cached_runs: None,
            typed_cached_runs: None,
            checked_payload_source_bytes: (0, 0),
            typed_cached_comment_ranges: Vec::new(),
            parsed_cached_comment_ranges: Vec::new(),
            parsed_cached_runs: None,
            ordered_result: None,
            span: None,
            template_word_receipt: None,
            source: Arc::new(FieldSource::Parsed {
                source_id,
                owner_id: source_id,
                form,
                publication_owned: (form == FieldForm::Simple)
                    .then(|| std::iter::once(0..raw_xml.len()).collect()),
                raw_xml,
                original_instruction,
                original_cached_result,
                cached_segments,
                original_dirty: dirty,
                original_locked: locked,
                original_legacy_form: legacy_form,
                original_result_published: result_published,
                word_prefixes,
            }),
        }
    }

    /// Inspect stored owners without invoking recursive display selection.
    pub(crate) fn resource_owner_children(&self) -> Vec<crate::document::WordOwner<'_>> {
        self.resource_owner_children_with_result(None, None)
    }

    fn resource_owner_children_with_result<'a>(
        &'a self,
        replacement: Option<&'a CT_P>,
        span_replacement: Option<&'a [Option<CT_R>]>,
    ) -> Vec<crate::document::WordOwner<'a>> {
        use crate::document::WordOwner;
        let mut children = Vec::new();
        if let Some(paragraph) = replacement.or_else(|| {
            self.ordered_result
                .as_ref()
                .map(|owner| owner.paragraph.as_ref())
        }) {
            children.push(WordOwner::Paragraph(paragraph));
        }
        for runs in [
            &self.simple_cached_runs,
            &self.typed_cached_runs,
            if replacement.is_none() {
                &self.parsed_cached_runs
            } else {
                &None
            },
        ]
        .into_iter()
        .flatten()
        {
            children.extend(runs.iter().map(WordOwner::Run));
        }
        if replacement.is_none() {
            for (_, field) in &self.cached_fields {
                children.push(WordOwner::Field(field));
            }
        }
        for instruction in [&self.instruction, self.original_instruction()] {
            for argument in instruction.arguments.iter().chain(
                instruction
                    .switches
                    .iter()
                    .filter_map(|s| s.argument.as_ref()),
            ) {
                if let FieldArgument::Nested(field) = argument {
                    children.push(WordOwner::Field(field));
                }
            }
        }
        if let FieldSource::New {
            cached_runs: Some(runs),
            ..
        } = self.source.as_ref()
        {
            children.extend(runs.iter().map(WordOwner::Run));
        }
        if let Some(runs) =
            span_replacement.or_else(|| self.span.as_ref().map(|span| span.runs.as_slice()))
        {
            children.extend(runs.iter().flatten().map(WordOwner::Run));
        }
        children
    }

    /// Return nested fields in their original instruction order.
    #[doc(hidden)]
    pub fn nested_fields_in_source_order(&self) -> Vec<&Field> {
        let original = self.original_instruction();
        let structured_unchanged = instruction_structure_eq(&self.instruction, original);
        if structured_unchanged && self.instruction.raw != original.raw {
            return Vec::new();
        }
        self.nested_fields_from_instruction(&self.instruction, structured_unchanged)
    }

    pub(super) fn parsed_cached_child_runs(&self) -> Option<&[CT_R]> {
        self.parsed_cached_runs.as_deref().filter(|runs| {
            self.cached_fields.is_empty()
                && runs
                    .iter()
                    .flat_map(|run| &run.content)
                    .any(|content| matches!(content, RunContent::Field(_)))
        })
    }

    /// Direct nested fields in the ordered cached result, without flattening it.
    #[doc(hidden)]
    pub fn cached_fields_in_source_order(&self) -> Vec<&Field> {
        if let Some(owner) = &self.ordered_result {
            return owner
                .paragraph
                .accepted_run_paths()
                .iter()
                .filter_map(|path| owner.paragraph.accepted_run(path))
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
                .collect();
        }
        if let Some(runs) = self
            .typed_cache()
            .or(self.simple_cached_runs.as_deref())
            .or(self.parsed_cached_child_runs())
        {
            return runs
                .iter()
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
                .collect();
        }
        match self.source.as_ref() {
            FieldSource::New {
                cached_runs: Some(runs),
                ..
            } => runs
                .iter()
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
                .collect(),
            _ => self.cached_fields.iter().map(|(_, field)| field).collect(),
        }
    }

    /// Direct nested operands followed by nested cached-result fields.
    #[doc(hidden)]
    pub fn all_nested_fields_in_source_order(&self) -> Vec<&Field> {
        let mut fields = self.nested_fields_in_source_order();
        fields.extend(self.cached_fields_in_source_order());
        fields
    }

    /// Mutate a direct cached-result field by its physical encounter index.
    /// Preserve a parsed child's identity. Complete checked authored replacements
    /// are supported. An incomplete replacement that drops required parsed cache
    /// ownership is rejected during checked serialization.
    #[doc(hidden)]
    pub fn cached_field_mut(&mut self, index: usize) -> Option<&mut Field> {
        if self.ordered_result.is_some() {
            crate::document::admit_word_model(crate::document::WordOwner::Field(self)).ok()?;
            self.normalization_work_budget().ok()?;
            let owner = self.ordered_result.as_mut()?;
            let mut remaining = index;
            let selected = owner
                .paragraph
                .accepted_run_paths()
                .into_iter()
                .find_map(|path| {
                    let run = owner.paragraph.accepted_run(&path)?;
                    for (slot, content) in run.content.iter().enumerate() {
                        if matches!(content, RunContent::Field(_)) {
                            if remaining == 0 {
                                return Some((path, slot));
                            }
                            remaining -= 1;
                        }
                    }
                    None
                })?;
            owner.pending = true;
            let run = Arc::make_mut(&mut owner.paragraph)
                .accepted_run_segments_mut(selected.0.segments())?;
            return match run.content.get_mut(selected.1)? {
                RunContent::Field(field) => Some(field),
                _ => None,
            };
        }
        let parsed_children = self.parsed_cached_child_runs().is_some();
        let runs = self
            .typed_cached_runs
            .as_mut()
            .or(self.simple_cached_runs.as_mut())
            .or(if parsed_children {
                self.parsed_cached_runs.as_mut()
            } else {
                None
            })
            .or(match Arc::make_mut(&mut self.source) {
                FieldSource::New { cached_runs, .. } => cached_runs.as_mut(),
                _ => None,
            });
        if let Some(runs) = runs {
            return runs
                .iter_mut()
                .flat_map(|run| &mut run.content)
                .filter_map(|content| {
                    if let RunContent::Field(field) = content {
                        Some(field)
                    } else {
                        None
                    }
                })
                .nth(index);
        }
        self.cached_fields.get_mut(index).map(|(_, field)| field)
    }

    /// Refresh the display projection after nested cached fields were edited.
    #[doc(hidden)]
    pub fn refresh_cached_field_projection(&mut self) {
        if let Some(value) = self.nested_cached_projection() {
            self.cached_result = value;
            self.derived_parent_cache = self.cached_result.clone();
        }
    }

    pub(super) fn nested_cached_projection(&self) -> Option<String> {
        if let Some(owner) = &self.ordered_result {
            return Some(owner.paragraph.accepted_text());
        }
        if let Some(runs) = self
            .typed_cached_runs
            .as_deref()
            .or(self.simple_cached_runs.as_deref().filter(|runs| {
                runs.iter()
                    .flat_map(|run| &run.content)
                    .any(|content| matches!(content, RunContent::Field(_)))
            }))
            .or(self.parsed_cached_child_runs())
        {
            return Some(runs.iter().map(cached_run_display).collect());
        }
        match self.source.as_ref() {
            FieldSource::New {
                cached_runs: Some(runs),
                ..
            } => Some(runs.iter().map(cached_run_display).collect()),
            FieldSource::Parsed {
                original_cached_result,
                ..
            } if !self.cached_fields.is_empty() => {
                let mut value = original_cached_result.clone();
                for (range, field) in self.cached_fields.iter().rev() {
                    if !value.is_char_boundary(range.start)
                        || !value.is_char_boundary(range.end)
                        || range.end > value.len()
                    {
                        return None;
                    }
                    value.replace_range(range.clone(), &field.cached_result);
                }
                Some(value)
            }
            _ => None,
        }
    }

    /// Reject a malformed legacy-form owner retained by this field.
    #[doc(hidden)]
    pub fn validate_legacy_form_owner(&self) -> Result<()> {
        if self.legacy_form_parse_error {
            return Err(OxmlError::InvalidValue(
                "malformed legacy form owner".to_owned(),
            ));
        }
        Ok(())
    }

    /// Replace the selected legacy form while retaining original nested-field order.
    #[doc(hidden)]
    pub fn set_nth_legacy_form_value_in_source_order(
        &mut self,
        remaining: &mut usize,
        value: &LegacyFormFieldValue,
    ) -> Result<bool> {
        self.set_nth_legacy_form_value_with_work(remaining, value, None)
    }

    pub(crate) fn set_nth_legacy_form_value_with_work(
        &mut self,
        remaining: &mut usize,
        value: &LegacyFormFieldValue,
        mut work: Option<&mut WordOperationWork>,
    ) -> Result<bool> {
        self.validate_legacy_form_owner()?;
        if self.legacy_form.is_some() {
            if *remaining == 0 {
                self.set_legacy_form_value_with_work(value.clone(), work)?;
                return Ok(true);
            }
            *remaining -= 1;
        }

        let original = self.original_instruction().clone();
        let structured_unchanged = instruction_structure_eq(&self.instruction, &original);
        if structured_unchanged && self.instruction.raw != original.raw {
            return Ok(false);
        }
        let ordered = if structured_unchanged {
            self.nested_order.clone()
        } else {
            Vec::new()
        };
        for position in &ordered {
            let nested = match *position {
                NestedFieldPosition::Argument(index) => self.instruction.arguments.get_mut(index),
                NestedFieldPosition::Switch(index) => self
                    .instruction
                    .switches
                    .get_mut(index)
                    .and_then(|field_switch| field_switch.argument.as_mut()),
            };
            if let Some(FieldArgument::Nested(field)) = nested
                && field.set_nth_legacy_form_value_with_work(
                    remaining,
                    value,
                    work.as_deref_mut(),
                )?
            {
                return Ok(true);
            }
        }
        for (index, argument) in self.instruction.arguments.iter_mut().enumerate() {
            if ordered.contains(&NestedFieldPosition::Argument(index)) {
                continue;
            }
            if let FieldArgument::Nested(field) = argument
                && field.set_nth_legacy_form_value_with_work(
                    remaining,
                    value,
                    work.as_deref_mut(),
                )?
            {
                return Ok(true);
            }
        }
        for (index, field_switch) in self.instruction.switches.iter_mut().enumerate() {
            if ordered.contains(&NestedFieldPosition::Switch(index)) {
                continue;
            }
            if let Some(FieldArgument::Nested(field)) = &mut field_switch.argument
                && field.set_nth_legacy_form_value_with_work(
                    remaining,
                    value,
                    work.as_deref_mut(),
                )?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Return the instruction selected by the public raw-versus-structured edit rules.
    #[doc(hidden)]
    pub fn effective_instruction(&self) -> FieldInstruction {
        instruction_for_write(self)
    }

    /// Return the effective instruction spelling without cloning nested field trees.
    #[doc(hidden)]
    pub fn effective_instruction_text(&self) -> String {
        let original = self.original_instruction();
        let structured_changed = !instruction_structure_eq(&self.instruction, original);
        if self.instruction.raw != original.raw && !structured_changed {
            self.instruction.raw.trim().to_owned()
        } else if structured_changed {
            canonical_instruction_text(&self.instruction)
        } else {
            self.instruction.raw.clone()
        }
    }

    /// Return nested fields from an effective instruction in evaluation order.
    #[doc(hidden)]
    pub fn effective_nested_fields_in_source_order<'a>(
        &self,
        instruction: &'a FieldInstruction,
    ) -> Vec<&'a Field> {
        self.nested_fields_from_instruction(
            instruction,
            instruction_structure_eq(&self.instruction, self.original_instruction()),
        )
    }

    fn original_instruction(&self) -> &FieldInstruction {
        match self.source.as_ref() {
            FieldSource::New {
                original_instruction,
                ..
            }
            | FieldSource::Parsed {
                original_instruction,
                ..
            } => original_instruction,
        }
    }

    fn nested_fields_from_instruction<'a>(
        &self,
        instruction: &'a FieldInstruction,
        preserve_source_order: bool,
    ) -> Vec<&'a Field> {
        let mut fields = Vec::new();
        if preserve_source_order {
            for position in &self.nested_order {
                let argument = match *position {
                    NestedFieldPosition::Argument(index) => instruction.arguments.get(index),
                    NestedFieldPosition::Switch(index) => instruction
                        .switches
                        .get(index)
                        .and_then(|switch| switch.argument.as_ref()),
                };
                if let Some(FieldArgument::Nested(field)) = argument {
                    fields.push(field.as_ref());
                }
            }
        }
        for argument in instruction.arguments.iter().chain(
            instruction
                .switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            let FieldArgument::Nested(field) = argument else {
                continue;
            };
            if !fields
                .iter()
                .any(|ordered| std::ptr::eq(*ordered, field.as_ref()))
            {
                fields.push(field.as_ref());
            }
        }
        fields
    }

    pub(super) fn is_unchanged(&self) -> bool {
        if let Some(owner) = &self.ordered_result {
            if owner.pending || owner.pending_payload || owner.paragraph.has_changed_field_owners()
            {
                return false;
            }
            if let FieldSource::Parsed { raw_xml, .. } = self.source.as_ref()
                && raw_xml.get(owner.source_range.clone()) != Some(owner.normalized_xml.as_slice())
            {
                return false;
            }
        }
        if self.typed_cached_runs.is_some() {
            return false;
        }
        match self.source.as_ref() {
            FieldSource::New { .. } => false,
            FieldSource::Parsed {
                original_instruction,
                original_cached_result,
                ..
            } => {
                self.instruction == *original_instruction
                    && instruction_source_identity_eq(&self.instruction, original_instruction)
                    && self.cached_result == *original_cached_result
                    && self.source_metadata_unchanged()
            }
        }
    }

    fn source_metadata_unchanged(&self) -> bool {
        let FieldSource::Parsed {
            original_dirty,
            original_locked,
            ..
        } = self.source.as_ref()
        else {
            return false;
        };
        self.source_content_metadata_unchanged()
            && self.dirty == *original_dirty
            && self.locked == *original_locked
    }

    pub(super) fn source_content_metadata_unchanged(&self) -> bool {
        let FieldSource::Parsed {
            original_legacy_form,
            original_result_published,
            ..
        } = self.source.as_ref()
        else {
            return false;
        };
        self.typed_cached_runs.is_none()
            && self.legacy_form == *original_legacy_form
            && self.legacy_result_published == *original_result_published
            && self
                .cached_fields_in_source_order()
                .iter()
                .all(|field| field.is_unchanged())
    }

    pub(super) fn is_parsed_complex(&self) -> bool {
        matches!(
            self.source.as_ref(),
            FieldSource::Parsed {
                form: FieldForm::Complex,
                ..
            }
        )
    }

    /// Whether this field writes a complex `w:fldChar` sequence.
    #[doc(hidden)]
    pub fn is_complex(&self) -> bool {
        self.form() == FieldForm::Complex
    }

    /// Return the text this field contributes to [`CT_R::text`].
    #[doc(hidden)]
    pub fn projected_text(&self) -> Option<&str> {
        (self.is_parsed_complex()
            || matches!(
                self.source.as_ref(),
                FieldSource::New {
                    form: FieldForm::Complex,
                    ..
                }
            ))
        .then_some(self.cached_result.as_str())
    }

    /// Whether the retained field source carries semantic attributes outside
    /// the modeled instruction, type, and dirty-state projection.
    pub fn has_unmodeled_semantic_attributes(&self) -> bool {
        let FieldSource::Parsed {
            form,
            raw_xml,
            word_prefixes,
            ..
        } = self.source.as_ref()
        else {
            return false;
        };

        field_source_has_unmodeled_semantic_attributes(raw_xml, *form, word_prefixes)
            .unwrap_or(true)
    }

    /// Return the stored display split by its original result-run formatting.
    #[doc(hidden)]
    pub fn cached_display_segments(&self) -> Vec<(&str, Option<&CT_RPr>)> {
        self.cached_display_field_segments()
            .into_iter()
            .map(|(text, properties, _)| (text, properties))
            .collect()
    }

    /// Return the stored display split by its original result-run formatting.
    #[doc(hidden)]
    pub fn cached_display_field_segments(&self) -> Vec<(&str, Option<&CT_RPr>, &Field)> {
        if let Some(owner) = &self.ordered_result
            && self.cached_result == self.derived_parent_cache
        {
            return owner
                .paragraph
                .accepted_run_paths()
                .iter()
                .filter_map(|path| owner.paragraph.accepted_run(path))
                .flat_map(|run| {
                    run.content.iter().flat_map(|content| match content {
                        RunContent::Field(child) => child.cached_display_field_segments(),
                        RunContent::Break(BreakType::Page, _) => {
                            vec![("\u{000c}", run.properties.as_ref(), self)]
                        }
                        RunContent::Break(BreakType::Column, _) => {
                            vec![("\u{000b}", run.properties.as_ref(), self)]
                        }
                        _ => vec![(CT_R::content_text(content), run.properties.as_ref(), self)],
                    })
                })
                .collect();
        }
        let (original_cached_result, cached_segments) = match self.source.as_ref() {
            FieldSource::Parsed {
                original_cached_result,
                cached_segments,
                ..
            }
            | FieldSource::New {
                original_cached_result,
                cached_segments,
                ..
            } => (original_cached_result, cached_segments),
        };
        if self.nested_cached_projection().as_deref() == Some(self.cached_result.as_str()) {
            let runs = self
                .typed_cache()
                .or(self.simple_cached_runs.as_deref())
                .or(self.parsed_cached_child_runs())
                .or(match self.source.as_ref() {
                    FieldSource::New {
                        cached_runs: Some(runs),
                        ..
                    } => Some(runs.as_slice()),
                    _ => None,
                });
            if let Some(runs) = runs {
                return runs
                    .iter()
                    .flat_map(|run| {
                        run.content.iter().flat_map(|content| match content {
                            RunContent::Field(child) => child.cached_display_field_segments(),
                            RunContent::Break(BreakType::Page, _) => {
                                vec![("\u{000c}", run.properties.as_ref(), self)]
                            }
                            RunContent::Break(BreakType::Column, _) => {
                                vec![("\u{000b}", run.properties.as_ref(), self)]
                            }
                            _ => vec![(CT_R::content_text(content), run.properties.as_ref(), self)],
                        })
                    })
                    .collect();
            }
            if !self.cached_fields.is_empty() {
                let mut segments = Vec::new();
                let mut start = 0;
                for (range, child) in &self.cached_fields {
                    segments.extend(cached_display_range(
                        cached_segments,
                        start..range.start,
                        self,
                    ));
                    segments.extend(child.cached_display_field_segments());
                    start = range.end;
                }
                segments.extend(cached_display_range(
                    cached_segments,
                    start..original_cached_result.len(),
                    self,
                ));
                return segments;
            }
        }
        if !cached_segments.is_empty() {
            if self.cached_result == *original_cached_result {
                return cached_segments
                    .iter()
                    .map(|segment| (segment.text.as_str(), segment.properties.as_ref(), self))
                    .collect();
            }
            return vec![(
                self.cached_result.as_str(),
                cached_segments
                    .first()
                    .and_then(|segment| segment.properties.as_ref()),
                self,
            )];
        }
        vec![(self.cached_result.as_str(), None, self)]
    }

    /// Whether this cached display owner is protected by any enclosing field.
    /// An owner outside this field's cached tree is conservatively locked.
    #[doc(hidden)]
    pub fn cached_display_owner_is_locked(&self, owner: &Field) -> bool {
        fn find(field: &Field, owner: &Field, inherited: bool) -> Option<bool> {
            let locked = inherited || field.locked() == Some(true);
            if std::ptr::eq(field, owner) {
                return Some(locked);
            }
            field
                .cached_fields_in_source_order()
                .into_iter()
                .find_map(|child| find(child, owner, locked))
        }
        find(self, owner, false).unwrap_or(true)
    }

    /// Return the parsed source fragment and its cache-aware replacement.
    #[doc(hidden)]
    pub fn source_replacement(&self) -> Result<Option<(&[u8], Vec<u8>)>> {
        if !matches!(self.source.as_ref(), FieldSource::Parsed { .. }) {
            return Ok(None);
        }
        let mut work = WordOperationWork::model_field(self)?;
        self.source_replacement_in(&mut work)
    }

    pub(crate) fn source_replacement_in(
        &self,
        work: &mut WordOperationWork,
    ) -> Result<Option<(&[u8], Vec<u8>)>> {
        let FieldSource::Parsed { raw_xml, .. } = self.source.as_ref() else {
            return Ok(None);
        };
        let mut writer = Writer::new(Vec::new());
        write_field(&mut writer, self, None, work)?;
        Ok(Some((raw_xml, writer.into_inner())))
    }

    /// Return what this field writes on its own when it was read from a run
    /// that also held content outside it.
    #[doc(hidden)]
    pub fn detached_source(&self) -> Result<Option<Vec<u8>>> {
        if self.span.is_none() {
            return Ok(None);
        }
        let mut work = WordOperationWork::model_field(self)?;
        self.detached_source_in(&mut work)
    }

    pub(crate) fn detached_source_in(
        &self,
        work: &mut WordOperationWork,
    ) -> Result<Option<Vec<u8>>> {
        if self.span.is_none() {
            return Ok(None);
        }
        let mut writer = Writer::new(Vec::new());
        write_run_field(&mut writer, self, None, None, None, work)?;
        Ok(Some(writer.into_inner()))
    }

    /// Return the retained physical XML owner identity for a parsed field.
    #[doc(hidden)]
    pub fn source_owner_id(&self) -> Option<u64> {
        match self.source.as_ref() {
            FieldSource::Parsed { owner_id, .. } => Some(*owner_id),
            FieldSource::New { .. } => None,
        }
    }

    /// Read the authoritative current value. `Ok(None)` is a non-legacy field.
    #[doc(hidden)]
    pub fn legacy_form_value(&self) -> Result<Option<LegacyFormFieldValue>> {
        self.validate_legacy_form_owner()?;
        Ok(self.legacy_form.as_ref().map(|form| match form.kind {
            LegacyFormFieldKind::TextInput => {
                LegacyFormFieldValue::Text(self.effective_display().into_owned())
            }
            LegacyFormFieldKind::CheckBox | LegacyFormFieldKind::DropDownList => form.value.clone(),
        }))
    }

    /// Read explicit reset-default metadata. `None` means that metadata is absent.
    pub fn legacy_form_default_value(&self) -> Result<Option<LegacyFormFieldValue>> {
        self.validate_legacy_form_owner()?;
        if self.legacy_form.is_none() {
            return Ok(None);
        }
        Ok(self.legacy_reset_default.clone())
    }

    /// Change the typed current value and cache. The reset default stays put.
    ///
    /// Candidate bytes decide whether the edit is a no-op. Typed equality does not.
    #[doc(hidden)]
    pub fn set_legacy_form_value(&mut self, value: LegacyFormFieldValue) -> Result<()> {
        self.set_legacy_form_value_with_work(value, None)
    }

    fn set_legacy_form_value_with_work(
        &mut self,
        value: LegacyFormFieldValue,
        inherited: Option<&mut WordOperationWork>,
    ) -> Result<()> {
        self.validate_legacy_form_owner()?;
        let form = self
            .legacy_form
            .clone()
            .ok_or_else(|| OxmlError::MissingElement("w:ffData".to_owned()))?;
        let display = legacy_form_canonical_display(&form, &value)?;
        let mut standalone;
        let work = match inherited {
            Some(work) => work,
            None => {
                standalone = WordOperationWork::model_field(self)?;
                &mut standalone
            }
        };
        let mut candidate = self.clone();
        let region = self.legacy_result_published;
        let force_region = !region && (display != self.cached_result || value != form.value);
        if force_region {
            candidate.legacy_result_published = true;
        }
        if region || force_region {
            candidate.cached_result = display;
        }
        if let Some(form) = candidate.legacy_form.as_mut() {
            form.value = value;
        }
        if field_publication_bytes(self, work)? == field_publication_bytes(&candidate, work)? {
            return Ok(());
        }
        *self = candidate;
        Ok(())
    }

    /// Semantic display. An observable parent cache wins over nested children.
    pub fn effective_display(&self) -> std::borrow::Cow<'_, str> {
        let nested = self.nested_effective_segments();
        let nested_text = joined_nested_text(nested.as_deref());
        if self.observable_parent_cache_override(nested_text.as_deref()) {
            return std::borrow::Cow::Borrowed(self.cached_result.as_str());
        }
        if let Some(nested_text) = nested_text {
            return std::borrow::Cow::Owned(nested_text);
        }
        self.own_effective_display()
    }

    /// Semantic projection with the same complex-field visibility as [`Self::projected_text`].
    pub fn effective_projected_text(&self) -> Option<std::borrow::Cow<'_, str>> {
        let visible = self.is_parsed_complex()
            || matches!(
                self.source.as_ref(),
                FieldSource::New {
                    form: FieldForm::Complex,
                    ..
                }
            );
        visible.then(|| self.effective_display())
    }

    /// Semantic display segments. Nested rows keep the child owner and its `CT_RPr`.
    pub fn effective_display_field_segments(
        &self,
    ) -> Vec<(std::borrow::Cow<'_, str>, Option<&CT_RPr>, &Field)> {
        let nested = self.nested_effective_segments();
        let nested_text = joined_nested_text(nested.as_deref());
        if self.observable_parent_cache_override(nested_text.as_deref()) {
            return vec![(
                std::borrow::Cow::Borrowed(self.cached_result.as_str()),
                self.first_cached_properties(),
                self,
            )];
        }
        if let Some(segments) = nested {
            return segments;
        }
        if self.text_result_absent() {
            return vec![(
                std::borrow::Cow::Borrowed(self.legacy_text_default.as_deref().unwrap_or("")),
                self.first_cached_properties(),
                self,
            )];
        }
        self.cached_display_field_segments()
            .into_iter()
            .map(|(text, properties, owner)| (std::borrow::Cow::Borrowed(text), properties, owner))
            .collect()
    }

    fn text_current(&self) -> &str {
        if self.legacy_result_published
            || self.typed_cached_runs.is_some()
            || self.cached_result != self.original_cached_result()
        {
            return self.cached_result.as_str();
        }
        self.legacy_text_default.as_deref().unwrap_or("")
    }

    fn text_result_absent(&self) -> bool {
        self.legacy_form
            .as_ref()
            .is_some_and(|form| form.kind == LegacyFormFieldKind::TextInput)
            && !self.legacy_result_published
            && self.typed_cached_runs.is_none()
            && self.cached_result == self.original_cached_result()
    }

    fn first_cached_properties(&self) -> Option<&CT_RPr> {
        match self.source.as_ref() {
            FieldSource::Parsed {
                cached_segments, ..
            }
            | FieldSource::New {
                cached_segments, ..
            } => cached_segments
                .first()
                .and_then(|segment| segment.properties.as_ref()),
        }
    }

    fn own_effective_display(&self) -> std::borrow::Cow<'_, str> {
        if self
            .legacy_form
            .as_ref()
            .is_some_and(|form| form.kind == LegacyFormFieldKind::TextInput)
        {
            return std::borrow::Cow::Borrowed(self.text_current());
        }
        std::borrow::Cow::Borrowed(self.cached_result.as_str())
    }

    fn original_cached_result(&self) -> &str {
        match self.source.as_ref() {
            FieldSource::Parsed {
                original_cached_result,
                ..
            }
            | FieldSource::New {
                original_cached_result,
                ..
            } => original_cached_result,
        }
    }

    fn observable_parent_cache_override(&self, nested_text: Option<&str>) -> bool {
        let Some(nested_text) = nested_text else {
            return false;
        };
        if self.cached_result == nested_text {
            return false;
        }
        self.cached_result != self.derived_parent_cache
    }

    fn nested_effective_segments(&self) -> Option<Vec<NestedEffectiveSegment<'_>>> {
        if let Some(owner) = &self.ordered_result {
            let mut segments = Vec::new();
            for path in owner.paragraph.accepted_run_paths() {
                let Some(run) = owner.paragraph.accepted_run(&path) else {
                    continue;
                };
                for content in &run.content {
                    match content {
                        RunContent::Field(child) => {
                            segments.extend(child.effective_display_field_segments())
                        }
                        RunContent::Break(BreakType::Page, _) => segments.push((
                            Cow::Borrowed("\u{000c}"),
                            run.properties.as_ref(),
                            self,
                        )),
                        RunContent::Break(BreakType::Column, _) => segments.push((
                            Cow::Borrowed("\u{000b}"),
                            run.properties.as_ref(),
                            self,
                        )),
                        other => segments.push((
                            Cow::Borrowed(CT_R::content_text(other)),
                            run.properties.as_ref(),
                            self,
                        )),
                    }
                }
            }
            return Some(segments);
        }
        // Unfiltered typed runs stay visible after a child edit. The filtered
        // physical helper remains the source for borrowed cache reads.
        let runs = self
            .typed_cached_runs
            .as_deref()
            .or(self.simple_cached_runs.as_deref())
            .or(self.parsed_cached_runs.as_deref())
            .or(match self.source.as_ref() {
                FieldSource::New { cached_runs, .. } => cached_runs.as_deref(),
                _ => None,
            });
        if let Some(runs) = runs {
            let has_field = runs.iter().any(|run| {
                run.content
                    .iter()
                    .any(|content| matches!(content, RunContent::Field(_)))
            });
            if has_field {
                let mut segments = Vec::new();
                for run in runs {
                    for content in &run.content {
                        match content {
                            RunContent::Field(child) => {
                                segments.extend(child.effective_display_field_segments());
                            }
                            RunContent::Break(BreakType::Page, _) => {
                                segments.push((
                                    std::borrow::Cow::Borrowed("\u{000c}"),
                                    run.properties.as_ref(),
                                    self,
                                ));
                            }
                            RunContent::Break(BreakType::Column, _) => {
                                segments.push((
                                    std::borrow::Cow::Borrowed("\u{000b}"),
                                    run.properties.as_ref(),
                                    self,
                                ));
                            }
                            other => {
                                let text = CT_R::content_text(other);
                                segments.push((
                                    std::borrow::Cow::Borrowed(text),
                                    run.properties.as_ref(),
                                    self,
                                ));
                            }
                        }
                    }
                }
                return Some(segments);
            }
        }
        if self.cached_fields.is_empty() {
            return None;
        }
        let (original_cached_result, cached_segments) = match self.source.as_ref() {
            FieldSource::Parsed {
                original_cached_result,
                cached_segments,
                ..
            }
            | FieldSource::New {
                original_cached_result,
                cached_segments,
                ..
            } => (original_cached_result.as_str(), cached_segments.as_slice()),
        };
        let mut segments = Vec::new();
        let mut start = 0usize;
        for (range, child) in &self.cached_fields {
            segments.extend(
                cached_display_range(cached_segments, start..range.start, self)
                    .into_iter()
                    .map(|(text, properties, owner)| {
                        (std::borrow::Cow::Borrowed(text), properties, owner)
                    }),
            );
            segments.extend(child.effective_display_field_segments());
            start = range.end;
        }
        segments.extend(
            cached_display_range(cached_segments, start..original_cached_result.len(), self)
                .into_iter()
                .map(|(text, properties, owner)| {
                    (std::borrow::Cow::Borrowed(text), properties, owner)
                }),
        );
        Some(segments)
    }
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        let same_owner = match (&self.ordered_result, &other.ordered_result) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                Arc::ptr_eq(&left.paragraph, &right.paragraph) || left.paragraph == right.paragraph
            }
            // Global Field equality retains its established cross-form
            // projection semantics. Exact source/form checks belong to the
            // actual comparison and cache-authority boundary.
            _ => true,
        };
        same_owner
            && self.instruction == other.instruction
            && self.cached_result == other.cached_result
            && self.dirty == other.dirty
            && self.locked == other.locked
            && self.legacy_form == other.legacy_form
            && self.legacy_form_parse_error == other.legacy_form_parse_error
            && self.cached_result_runs() == other.cached_result_runs()
            && self.cached_result_comment_ranges() == other.cached_result_comment_ranges()
    }
}

/// The shared grammar for simple and complex field instructions.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInstruction {
    pub raw: String,
    pub name: String,
    pub arguments: Vec<FieldArgument>,
    pub switches: Vec<FieldSwitch>,
}

impl FieldInstruction {
    /// Whether the preserved instruction closes every unescaped quoted operand.
    #[doc(hidden)]
    pub fn quotes_are_balanced(&self) -> bool {
        let mut characters = self.raw.chars().peekable();
        let mut quoted = false;
        while let Some(character) = characters.next() {
            if quoted
                && character == '\\'
                && characters
                    .peek()
                    .is_some_and(|next| matches!(next, '"' | '\\'))
            {
                characters.next();
            } else if character == '"' {
                quoted = !quoted;
            }
        }
        !quoted
    }

    /// Build one canonical instruction from operands that cannot inject tokens.
    pub fn new(
        name: &str,
        arguments: Vec<FieldArgument>,
        switches: Vec<FieldSwitch>,
    ) -> Result<Self> {
        for argument in arguments.iter().chain(
            switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            if let FieldArgument::Nested(field) = argument {
                field.validate_for_attachment()?;
            }
        }
        validate_authored_instruction_parts(name, &arguments, &switches)?;
        let name = name.to_uppercase();
        let mut switches = switches;
        for switch in &mut switches {
            switch.name = switch.name.to_ascii_lowercase();
        }
        let mut instruction = Self {
            raw: String::new(),
            name,
            arguments,
            switches,
        };
        instruction.raw = canonical_instruction_text(&instruction);
        Ok(instruction)
    }
}

// Strict authoring applies to the whole operand program, including a parsed child
// inserted into a new parent. Borrow descendants rather than cloning each tree.
fn validate_authored_instruction_parts(
    name: &str,
    arguments: &[FieldArgument],
    switches: &[FieldSwitch],
) -> Result<()> {
    let mut pending = vec![(name, arguments, switches)];
    while let Some((name, arguments, switches)) = pending.pop() {
        if !valid_field_token(name) || name.starts_with('\\') {
            return Err(OxmlError::InvalidValue("invalid field name".into()));
        }
        let canonical_name = name.to_uppercase();
        for switch in switches {
            if !(valid_field_token(&switch.name) || switch.name == "!")
                || switch.name.contains('\\')
            {
                return Err(OxmlError::InvalidValue("invalid field switch name".into()));
            }
            if switch.argument.is_some()
                && switch_is_known_flag(&canonical_name, &switch.name.to_ascii_lowercase())
            {
                return Err(OxmlError::InvalidValue(format!(
                    "field switch {} does not take an argument",
                    switch.name
                )));
            }
        }
        for argument in arguments.iter().chain(
            switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            match argument {
                FieldArgument::Text(value) => {
                    oxoffice_core::xml::reject_non_xml_characters("field argument", value)?
                }
                FieldArgument::Nested(field) => {
                    validate_raw_field_instruction(&field.effective_instruction_text())?;
                    if field.instruction.raw != field.original_instruction().raw
                        && instruction_structure_eq(
                            &field.instruction,
                            field.original_instruction(),
                        )
                    {
                        // A raw-only assignment retires the old structured program.
                        let current = parse_field_instruction(&field.instruction.raw);
                        validate_authored_instruction_parts(
                            &current.name,
                            &current.arguments,
                            &current.switches,
                        )?;
                    } else {
                        pending.push((
                            &field.instruction.name,
                            &field.instruction.arguments,
                            &field.instruction.switches,
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn valid_field_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '=' | '*' | '#' | '@'))
}

fn validate_raw_field_instruction(value: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("field instruction", value)?;
    let mut quoted = false;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if quoted
            && character == '\\'
            && characters
                .peek()
                .is_some_and(|next| matches!(next, '"' | '\\'))
        {
            characters.next();
        } else if character == '"' {
            quoted = !quoted;
        }
    }
    if quoted {
        return Err(OxmlError::InvalidValue(
            "unbalanced field instruction quotes".into(),
        ));
    }
    let tokens = lex_field_text(value);
    let Some(InstructionToken {
        argument: InstructionOperand::Text(name),
        quoted: false,
        ..
    }) = tokens.first()
    else {
        return Err(OxmlError::InvalidValue(
            "field instruction must start with a field name".into(),
        ));
    };
    if !valid_field_token(name) {
        return Err(OxmlError::InvalidValue("invalid field name".into()));
    }
    for token in &tokens[1..] {
        if let InstructionOperand::Text(value) = &token.argument
            && !token.quoted
            && value.starts_with('\\')
            && !(valid_field_token(&value[1..]) || value == "\\!")
        {
            return Err(OxmlError::InvalidValue("invalid field switch token".into()));
        }
    }
    Ok(())
}

fn cached_display_range<'a>(
    segments: &'a [CachedDisplaySegment],
    range: std::ops::Range<usize>,
    owner: &'a Field,
) -> Vec<(&'a str, Option<&'a CT_RPr>, &'a Field)> {
    let mut offset = 0;
    let mut result = Vec::new();
    for segment in segments {
        let end = offset + segment.text.len();
        let start = range.start.max(offset);
        let stop = range.end.min(end);
        if start < stop
            && let Some(text) = segment.text.get(start - offset..stop - offset)
        {
            result.push((text, segment.properties.as_ref(), owner));
        }
        offset = end;
    }
    result
}

fn joined_nested_text(segments: Option<&[NestedEffectiveSegment<'_>]>) -> Option<String> {
    let segments = segments?;
    if segments.is_empty() {
        return None;
    }
    Some(segments.iter().map(|(text, _, _)| text.as_ref()).collect())
}

// Same result projection as cached_run_display, compared without constructing
// a second display string during source selection.
fn cached_runs_display_eq(runs: &[CT_R], expected: &str) -> bool {
    let mut remaining = expected;
    for content in runs.iter().flat_map(|run| &run.content) {
        let piece = match content {
            RunContent::Field(field) => field.cached_result.as_str(),
            RunContent::Break(BreakType::Page, _) => "\u{000c}",
            RunContent::Break(BreakType::Column, _) => "\u{000b}",
            _ => CT_R::content_text(content),
        };
        let Some(tail) = remaining.strip_prefix(piece) else {
            return false;
        };
        remaining = tail;
    }
    remaining.is_empty()
}

pub(super) fn cached_run_display(run: &CT_R) -> String {
    run.content
        .iter()
        .map(|content| match content {
            RunContent::Field(field) => field.cached_result.clone(),
            RunContent::Break(BreakType::Page, _) => "\u{000c}".into(),
            RunContent::Break(BreakType::Column, _) => "\u{000b}".into(),
            _ => CT_R::content_text(content).to_owned(),
        })
        .collect()
}

pub(super) const FIELD_CACHE_VALIDATION_SCOPE: &[u8] = concat!(
    "<w:p xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
    "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" ",
    "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ",
    "xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" ",
    "xmlns:v=\"urn:schemas-microsoft-com:vml\" ",
    "xmlns:o=\"urn:schemas-microsoft-com:office:office\" ",
    "xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" ",
    "xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" ",
    "xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" ",
    "xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">",
)
.as_bytes();

fn validate_cached_field_runs(runs: &[CT_R]) -> Result<usize> {
    let _validation_scope = raw_root_binding_scope(FIELD_CACHE_VALIDATION_SCOPE, true)?;
    let mut source_bytes = 0usize;
    for run in runs {
        for content in &run.content {
            match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => {
                    oxoffice_core::xml::reject_non_xml_characters("field result", &text.text)?
                }
                RunContent::Field(field) => field.validate_for_attachment()?,
                _ => {}
            }
        }
        let mut localized = Vec::new();
        let (raw_root, raw_bindings) =
            run_xml_root(run, None, &mut localized, XmlWriteRole::Publication)?;
        for (index, raw) in run.extra_xml.iter().enumerate() {
            if run
                .extra_xml_positions
                .get(index)
                .is_some_and(|position| CT_R::raw_child_is_root_attributes(*position))
            {
                continue;
            }
            validate_raw_cached_field_content(&raw_root, &raw_bindings, &localized, raw)?;
        }
        let mut writer = Writer::new(Vec::new());
        writer
            .get_mut()
            .extend_from_slice(FIELD_CACHE_VALIDATION_SCOPE);
        run.to_xml(&mut writer)?;
        writer.get_mut().extend_from_slice(b"</w:p>");
        let source = writer.into_inner();
        source_bytes = source_bytes
            .checked_add(source.len())
            .ok_or_else(|| OxmlError::InvalidValue("cached result source byte overflow".into()))?;
        oxoffice_core::xml::validate_strict_xml_1_0(&source)
            .map_err(|error| OxmlError::InvalidValue(format!("invalid cached XML: {error:?}")))?;
    }
    Ok(source_bytes)
}

fn validate_raw_cached_field_content(
    root: &BytesStart<'_>,
    raw_bindings: &[(String, String)],
    localized: &[(String, String)],
    raw: &[u8],
) -> Result<()> {
    let _validation_scope = raw_root_binding_scope(FIELD_CACHE_VALIDATION_SCOPE, true)?;
    let mut writer = Writer::new(FIELD_CACHE_VALIDATION_SCOPE.to_vec());
    writer.write_event(Event::Start(root.clone()))?;
    let mut context = RunRawContext {
        foreign_word_namespace: None,
        bindings: raw_bindings,
        localized,
        source_context: None,
        source_budget: None,
        emission: None,
    };
    write_run_raw_source(&mut writer, raw, &mut context)?;
    writer.get_mut().extend_from_slice(b"</w:r></w:p>");
    let scoped = writer.into_inner();
    oxoffice_core::xml::validate_strict_xml_1_0(&scoped)
        .map_err(|error| OxmlError::InvalidValue(format!("invalid cached XML: {error:?}")))?;
    let mut reader = NsReader::from_reader(scoped.as_slice());
    loop {
        let (namespace, event) = reader.read_resolved_event()?;
        match event {
            Event::Start(element) | Event::Empty(element)
                if namespace
                    == ResolveResult::Bound(Namespace(crate::namespace::W_NS.as_bytes()))
                    && matches!(
                        element.local_name().as_ref(),
                        b"fldChar"
                            | b"fldSimple"
                            | b"instrText"
                            | b"delInstrText"
                            | b"commentRangeStart"
                            | b"commentRangeEnd"
                    ) =>
            {
                return Err(OxmlError::InvalidValue(
                    "raw Word field delimiters are not cached display content".into(),
                ));
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}
