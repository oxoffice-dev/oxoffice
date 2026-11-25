//! Footnotes and endnotes: creating, marking, moving and removing them, and their parts; and the comments a copied fragment brings along.

use super::*;

impl Document {
    /// All footnotes as (id, plain text), in file order.
    ///
    /// Separator entries are excluded. They live in the same stream and are
    /// retained by the model so a round trip preserves them, but they are not
    /// notes and never were part of this listing.
    pub fn footnotes(&self) -> Vec<(i32, String)> {
        self.footnotes
            .footnotes
            .iter()
            .filter(|f| f.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
            .map(|f| {
                let text = f
                    .paragraphs
                    .iter()
                    .map(|p| p.accepted_text())
                    .collect::<Vec<_>>()
                    .join("\n");
                (f.id, text)
            })
            .collect()
    }

    /// Add a footnote with the given text; returns its id. Pair with
    /// `Paragraph::add_footnote_ref` to reference it from the body. A
    /// character XML 1.0 cannot carry is dropped.
    pub fn add_footnote(&mut self, text: &str) -> i32 {
        let mut candidate = self.clone_for_staging();
        candidate
            .reserve_footnotes_bundle()
            .expect("an in-memory document can allocate a footnotes part");
        candidate.invalidate_layout();
        use oxoffice_docx_oxml::footnotes::CT_Footnote;
        use oxoffice_docx_oxml::text::CT_P;
        let id = candidate
            .footnotes
            .footnotes
            .iter()
            .map(|f| f.id)
            .max()
            .unwrap_or(1)
            + 1;
        let mark_style = candidate
            .note_reference_style_id(StoryKind::Footnote)
            .expect("an in-memory document can publish its note style");
        let specials = missing_note_specials(
            &candidate.footnotes,
            candidate
                .footnotes_part_name
                .as_deref()
                .and_then(|part| candidate.package.get_part(part)),
            StoryKind::Footnote,
            Some(id),
        )
        .expect("an in-memory document can inspect its note specials");
        candidate.footnotes.footnotes.extend(specials);
        let mut p = CT_P::new();
        let mut marker = CT_R::new("");
        marker.properties = Some(CT_RPr {
            style_id: Some(mark_style.clone()),
            ..Default::default()
        });
        marker.content.clear();
        marker.extra_xml.push(b"<w:footnoteRef/>".to_vec());
        marker.extra_xml_positions.push(1);
        p.runs.push(marker);
        p.add_run(" ");
        p.add_run(&oxoffice_core::xml::strip_non_xml_characters(text));
        candidate.footnotes.footnotes.push(CT_Footnote {
            id,
            note_type: oxoffice_docx_oxml::footnotes::NoteType::Normal,
            paragraphs: vec![p],
        });
        candidate.footnotes_dirty = true;
        candidate
            .flush_dirty_related_story_models()
            .expect("an in-memory document can publish a typed footnote");
        candidate
            .refresh_related_story_caches()
            .expect("an in-memory document can refresh a typed footnote");
        self.commit_staged_mutation(candidate);
        id
    }

    /// Create a normal footnote and append its reference to a direct body paragraph.
    /// The note and reference are published together only after the staged package reopens.
    pub fn create_footnote(&mut self, reference: &ContentLocation, text: &str) -> Result<i32> {
        if reference.story.kind != StoryKind::Body || reference.index_path.len() != 1 {
            return Err(Error::Other(
                "footnote reference must identify a direct body paragraph".to_owned(),
            ));
        }
        let run_index = self.read_story_paragraph(reference, |paragraph| paragraph.run_count())?;
        self.create_footnote_at(
            &StoryRunPosition {
                location: reference.clone(),
                run_index,
            },
            text,
        )
    }

    /// Create a note and reference together at a checked body or table-cell run boundary.
    pub fn create_footnote_at(&mut self, position: &StoryRunPosition, text: &str) -> Result<i32> {
        self.create_note_at_staged(StoryKind::Footnote, position, text)
    }

    /// Create a normal endnote and append its reference to a direct body paragraph.
    /// The note and reference are published together only after the staged package reopens.
    pub fn create_endnote(&mut self, reference: &ContentLocation, text: &str) -> Result<i32> {
        if reference.story.kind != StoryKind::Body || reference.index_path.len() != 1 {
            return Err(Error::Other(
                "endnote reference must identify a direct body paragraph".to_owned(),
            ));
        }
        let run_index = self.read_story_paragraph(reference, |paragraph| paragraph.run_count())?;
        self.create_endnote_at(
            &StoryRunPosition {
                location: reference.clone(),
                run_index,
            },
            text,
        )
    }

    /// Create a note and reference together at a checked body or table-cell run boundary.
    pub fn create_endnote_at(&mut self, position: &StoryRunPosition, text: &str) -> Result<i32> {
        self.create_note_at_staged(StoryKind::Endnote, position, text)
    }

    /// Create a footnote or endnote whose reference goes before run
    /// `run_index` of the direct body paragraph at body slot `slot` (the
    /// paragraph's end at its run count), as [`Self::create_footnote_at`]
    /// and [`Self::create_endnote_at`] do, on the live document: the
    /// paragraph is edited in its typed model, the note is appended to its
    /// part, and nothing is serialized or reopened. `Ok(None)` changes
    /// nothing and leaves the call to those routes: `slot` holds no direct
    /// paragraph. A refusal leaves the document as it was.
    #[doc(hidden)]
    pub fn create_note_in_body_slot(
        &mut self,
        kind: StoryKind,
        slot: usize,
        run_index: usize,
        text: &str,
    ) -> Result<Option<i32>> {
        let note_unit = match kind {
            StoryKind::Footnote => EditUnit::Footnotes,
            StoryKind::Endnote => EditUnit::Endnotes,
            _ => {
                return Err(Error::Other(
                    "a note is a footnote or an endnote".to_owned(),
                ));
            }
        };
        if !matches!(
            self.document.body.content.get(slot),
            Some(BodyContent::Paragraph(_))
        ) {
            return Ok(None);
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let savable_before = self.body_known_savable();
        let result = self.edit_in_place(
            // The flush of typed footnotes may write pending comments; the
            // first note adds its part, relationship and override.
            &[
                EditUnit::BodyBlocks { slot, count: 1 },
                note_unit,
                EditUnit::Comments,
                EditUnit::Styles,
                EditUnit::Parts,
            ],
            |document| document.create_note_in_slot_here(kind, slot, run_index, text),
        );
        // The paragraph took a typed reference run: a later edit need not
        // check the whole main part again.
        if result.is_ok() {
            self.keep_body_savable_after_typed_insert(slot, savable_before);
        }
        #[cfg(debug_assertions)]
        {
            let location = before
                .direct_body_item_location(slot)
                .expect("the body reads")
                .expect("a direct paragraph has a location");
            let position = StoryRunPosition {
                location,
                run_index,
            };
            self.assert_in_place_matches_staged_declarations_aside(before, &result, |staged| {
                staged.create_note_at_staged(kind, &position, text)
            });
        }
        result.map(Some)
    }

    fn create_note_in_slot_here(
        &mut self,
        kind: StoryKind,
        slot: usize,
        run_index: usize,
        text: &str,
    ) -> Result<i32> {
        oxoffice_core::xml::reject_non_xml_characters("note text", text)?;
        let mark_style = self.note_reference_style_id(kind)?;
        let Some(BodyContent::Paragraph(paragraph)) = self.document.body.content.get(slot) else {
            unreachable!("the caller checked the slot");
        };
        paragraph
            .accepted_run_paths()
            .len()
            .checked_sub(run_index)
            .ok_or_else(|| {
                Error::Other("note reference run boundary is out of range".to_owned())
            })?;
        let id = self.push_note(kind, &mark_style, text)?;
        let Some(BodyContent::Paragraph(paragraph)) = self.document.body.content.get_mut(slot)
        else {
            unreachable!("the caller checked the slot");
        };
        paragraph
            .insert_accepted_run(run_index, note_reference_run(kind, id, &mark_style))
            .map_err(|error| Error::Other(format!("field insertion boundary: {error}")))?;
        self.invalidate_layout();
        Ok(id)
    }

    /// The staged route of the note creators: a copy is edited, saved and
    /// reopened. It is the debug oracle of [`Self::create_note_in_body_slot`].
    fn create_note_at_staged(
        &mut self,
        kind: StoryKind,
        position: &StoryRunPosition,
        text: &str,
    ) -> Result<i32> {
        if !matches!(
            position.location.story.kind(),
            StoryKind::Body | StoryKind::TableCell
        ) || position.location.story.part_name() != self.doc_part_name
        {
            return Err(Error::Other(
                "note references require body or body-table content".to_owned(),
            ));
        }
        // A character XML 1.0 cannot carry is refused before anything is
        // written, on both routes, as python-docx refuses such text.
        oxoffice_core::xml::reject_non_xml_characters("note text", text)?;
        let mut candidate = self.clone_for_staging();
        let mark_style = candidate.note_reference_style_id(kind)?;
        candidate
            .read_story_paragraph(&position.location, |paragraph| paragraph.run_count())?
            .checked_sub(position.run_index)
            .ok_or_else(|| {
                Error::Other("note reference run boundary is out of range".to_owned())
            })?;
        if kind == StoryKind::Endnote {
            candidate.flush_to_package()?;
        }
        let id = candidate.push_note(kind, &mark_style, text)?;
        candidate.insert_story_run_staged(position, note_reference_run(kind, id, &mark_style))?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// Append a normal note of `text` to the footnotes or endnotes part,
    /// with the special notes the part lacks, under the smallest free id
    /// from 2, and answer its id. The first note adds the part.
    fn push_note(&mut self, kind: StoryKind, mark_style: &str, text: &str) -> Result<i32> {
        let mut paragraph = CT_P::new();
        let mut note_marker = CT_R::new("");
        note_marker.properties = Some(CT_RPr {
            style_id: Some(mark_style.to_owned()),
            ..Default::default()
        });
        note_marker.content.clear();
        note_marker.extra_xml.push(if kind == StoryKind::Endnote {
            b"<w:endnoteRef/>".to_vec()
        } else {
            b"<w:footnoteRef/>".to_vec()
        });
        note_marker.extra_xml_positions.push(1);
        paragraph.runs.push(note_marker);
        paragraph.add_run(" ");
        paragraph.add_run(text);
        if kind == StoryKind::Footnote {
            let specials = missing_note_specials(
                &self.footnotes,
                self.footnotes_part_name
                    .as_deref()
                    .and_then(|part| self.package.get_part(part)),
                StoryKind::Footnote,
                None,
            )?;
            self.footnotes.footnotes.extend(specials);
            let occupied = self
                .footnotes
                .footnotes
                .iter()
                .map(|note| note.id)
                .collect::<HashSet<_>>();
            let id = smallest_free_note_id(&occupied, "footnote")?;
            self.reserve_footnotes_bundle()?;
            self.footnotes
                .footnotes
                .push(oxoffice_docx_oxml::footnotes::CT_Footnote {
                    id,
                    note_type: oxoffice_docx_oxml::footnotes::NoteType::Normal,
                    paragraphs: vec![paragraph],
                });
            self.footnotes_dirty = true;
            self.flush_dirty_related_story_models()?;
            self.refresh_related_story_caches()?;
            self.invalidate_layout();
            return Ok(id);
        }
        let existing_part = self.note_part_name(StoryKind::Endnote)?;
        let part_name = self.reserve_document_part_bundle(
            existing_part.as_deref(),
            "/word/endnotes.xml",
            rel_types::ENDNOTES,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
        )?;
        let part_exists = self.package.get_part(&part_name).is_some();
        let source = match self.package.get_part(&part_name) {
            Some(xml) => xml.to_vec(),
            None => oxoffice_docx_oxml::footnotes::CT_Footnotes::new().to_xml_endnotes()?,
        };
        let existing = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&source)?;
        let occupied = existing
            .footnotes
            .iter()
            .map(|note| note.id)
            .collect::<HashSet<_>>();
        let id = smallest_free_note_id(&occupied, "endnote")?;
        let note = oxoffice_docx_oxml::footnotes::CT_Footnote {
            id,
            note_type: oxoffice_docx_oxml::footnotes::NoteType::Normal,
            paragraphs: vec![paragraph],
        };
        let mut fragments = missing_note_specials(
            &existing,
            part_exists.then_some(source.as_slice()),
            StoryKind::Endnote,
            Some(id),
        )?
        .iter()
        .map(|note| serialized_note_fragment(note, StoryKind::Endnote))
        .collect::<Result<Vec<_>>>()?;
        fragments.push(serialized_note_fragment(&note, StoryKind::Endnote)?);
        let updated = append_story_fragments_to_root(&source, b"endnotes", b"endnote", &fragments)?;
        set_story_source_xml(self, &part_name, updated)?;
        self.invalidate_layout();
        Ok(id)
    }

    /// Create a footnote whose visible reference is an authored mark.
    pub fn create_footnote_with_mark(
        &mut self,
        reference: &ContentLocation,
        text: &str,
        mark: &str,
    ) -> Result<i32> {
        self.create_note_with_mark(NoteFamily::Footnote, reference, text, mark)
    }

    /// Create an endnote whose visible reference is an authored mark.
    pub fn create_endnote_with_mark(
        &mut self,
        reference: &ContentLocation,
        text: &str,
        mark: &str,
    ) -> Result<i32> {
        self.create_note_with_mark(NoteFamily::Endnote, reference, text, mark)
    }

    fn create_note_with_mark(
        &mut self,
        family: NoteFamily,
        reference: &ContentLocation,
        text: &str,
        mark: &str,
    ) -> Result<i32> {
        if mark.is_empty() {
            return Err(Error::Other(
                "custom note mark must not be empty".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        let (kind, id) = match family {
            NoteFamily::Footnote => (
                StoryKind::Footnote,
                candidate.create_footnote(reference, text)?,
            ),
            NoteFamily::Endnote => (
                StoryKind::Endnote,
                candidate.create_endnote(reference, text)?,
            ),
        };
        let paragraph_index = reference.index_path.first().copied().ok_or_else(|| {
            Error::Other("note reference must identify a direct body paragraph".to_owned())
        })?;
        let refreshed = candidate
            .paragraph_story_location(paragraph_index)?
            .ok_or_else(|| Error::Other("new note reference paragraph is missing".to_owned()))?;
        let paragraph = candidate.story_paragraph_mut(&refreshed)?;
        let marker = paragraph
            .runs
            .last_mut()
            .and_then(|run| run.content.first_mut())
            .ok_or_else(|| Error::Other("new note reference is missing".to_owned()))?;
        match marker {
            RunContent::FootnoteRef { custom_mark, .. }
            | RunContent::EndnoteRef { custom_mark, .. } => {
                *custom_mark = Some(mark.to_owned());
            }
            _ => return Err(Error::Other("new note reference is malformed".to_owned())),
        }
        candidate.invalidate_layout();
        candidate.flush_to_package()?;
        let story = candidate
            .note_story(kind, id)?
            .ok_or_else(|| Error::Other("new note story is missing".to_owned()))?;
        let part_name = story.part_name.clone();
        let mut xml = candidate
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other("new note part is missing".to_owned()))?
            .to_vec();
        let owner = scan_story_owners(&xml, kind)?
            .into_iter()
            .find(|owner| owner.kind == kind && owner.owner_index == story.owner_index)
            .ok_or_else(|| Error::Other("new note owner is missing".to_owned()))?;
        let mut paragraph = CT_P::new();
        let marker_run = CT_R::new(mark);
        paragraph.runs.push(marker_run);
        paragraph.add_run(" ");
        paragraph.add_run(text);
        let note = oxoffice_docx_oxml::footnotes::CT_Footnote {
            id,
            note_type: oxoffice_docx_oxml::footnotes::NoteType::Normal,
            paragraphs: vec![paragraph],
        };
        let fragment = serialized_note_fragment(&note, kind)?;
        xml.splice(owner.full, fragment);
        set_story_source_xml(&mut candidate, &part_name, xml)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// Create or replace an authored note separator or continuation record.
    /// The supplied paragraphs are written into the special note owner without
    /// changing other note owners or unmodelled XML in the part.
    pub fn set_note_special_record(
        &mut self,
        family: NoteFamily,
        record: NoteSpecialRecord,
        paragraphs: Vec<CT_P>,
    ) -> Result<i32> {
        if paragraphs.is_empty() {
            return Err(Error::Other(
                "special note record needs a paragraph".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (kind, part_name, root, item) = match family {
            NoteFamily::Footnote => {
                candidate.reserve_footnotes_bundle()?;
                (
                    StoryKind::Footnote,
                    candidate
                        .footnotes_part_name
                        .clone()
                        .ok_or_else(|| Error::Other("footnotes part is missing".to_owned()))?,
                    b"footnotes".as_slice(),
                    b"footnote".as_slice(),
                )
            }
            NoteFamily::Endnote => {
                let existing = candidate.note_part_name(StoryKind::Endnote)?;
                let part = candidate.reserve_document_part_bundle(
                    existing.as_deref(),
                    "/word/endnotes.xml",
                    rel_types::ENDNOTES,
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
                )?;
                (
                    StoryKind::Endnote,
                    part,
                    b"endnotes".as_slice(),
                    b"endnote".as_slice(),
                )
            }
        };
        let source = match candidate.package.get_part(&part_name) {
            Some(xml) => xml.to_vec(),
            None if family == NoteFamily::Footnote => {
                oxoffice_docx_oxml::footnotes::CT_Footnotes::new().to_xml_footnotes()?
            }
            None => oxoffice_docx_oxml::footnotes::CT_Footnotes::new().to_xml_endnotes()?,
        };
        let parsed = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&source)?;
        let matches = parsed
            .footnotes
            .iter()
            .enumerate()
            .filter(|(_, note)| note.note_type == record.note_type())
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(Error::Other("duplicate special note records".to_owned()));
        }
        let used = parsed
            .footnotes
            .iter()
            .map(|note| note.id)
            .collect::<HashSet<_>>();
        let id = if let Some((_, note)) = matches.first() {
            note.id
        } else {
            let preferred = match record {
                NoteSpecialRecord::Separator => -1,
                NoteSpecialRecord::ContinuationSeparator => 0,
                NoteSpecialRecord::ContinuationNotice => -2,
            };
            if !used.contains(&preferred) {
                preferred
            } else {
                let mut next = -2_i32;
                while used.contains(&next) {
                    next = next.checked_sub(1).ok_or_else(|| {
                        Error::Other("special note ID range is exhausted".to_owned())
                    })?;
                }
                next
            }
        };
        let note = oxoffice_docx_oxml::footnotes::CT_Footnote {
            id,
            note_type: record.note_type(),
            paragraphs,
        };
        let fragment = serialized_note_fragment(&note, kind)?;
        let updated = if let Some((index, _)) = matches.first() {
            let spans = scan_story_owners_with_special(&source, kind, true)?
                .into_iter()
                .filter(|owner| owner.kind == kind)
                .collect::<Vec<_>>();
            let span = spans
                .get(*index)
                .ok_or_else(|| Error::Other("special note owner cannot be located".to_owned()))?;
            let mut xml = source;
            xml.splice(span.full.clone(), fragment);
            xml
        } else {
            append_story_fragments_to_root(&source, root, item, &[fragment])?
        };
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let mut properties = candidate
            .note_properties(family)
            .cloned()
            .unwrap_or_default();
        if !properties.special_references.contains(&id) {
            properties.special_references.push(id);
        }
        candidate.stage_settings_mutation(|settings| match family {
            NoteFamily::Footnote => settings.set_footnote_properties(properties),
            NoteFamily::Endnote => settings.set_endnote_properties(properties),
        })?;
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// Find the current story identity for a normal footnote ID.
    pub fn footnote_story(&self, id: i32) -> Result<Option<StoryId>> {
        self.note_story(StoryKind::Footnote, id)
    }

    /// Find the current story identity for a normal endnote ID.
    pub fn endnote_story(&self, id: i32) -> Result<Option<StoryId>> {
        self.note_story(StoryKind::Endnote, id)
    }

    /// The part that holds the footnotes, endnotes or comments, if the
    /// document has one, read from the relationships alone.
    #[doc(hidden)]
    pub fn annotation_part_name(&self, kind: StoryKind) -> Result<Option<String>> {
        match kind {
            StoryKind::Comment => Ok(self.comments_part_name.clone()),
            _ => self.note_part_name(kind),
        }
    }

    /// The story of the comment `id`, read from the comments part alone:
    /// the body is not read. `None` when no comment has that ID.
    #[doc(hidden)]
    pub fn comment_story(&self, id: i32) -> Result<Option<StoryId>> {
        let (Some(part_name), Some(comments)) = (&self.comments_part_name, &self.comments) else {
            return Ok(None);
        };
        let mut matches = comments
            .comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| comment.id == id);
        let Some((index, _)) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(Error::Other(format!("comment ID {id} is ambiguous")));
        }
        let xml = self.package.get_part(part_name).ok_or_else(|| {
            Error::Other(format!("comment stories target missing part {part_name}"))
        })?;
        let stories = scan_story_owners(xml, StoryKind::Comment)?
            .into_iter()
            .filter(|owner| owner.kind == StoryKind::Comment)
            .map(|owner| StoryId {
                kind: StoryKind::Comment,
                part_name: part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            })
            .collect::<Vec<_>>();
        if stories.len() != comments.comments.len() {
            return Err(Error::Other(
                "comment story count differs from the comment definitions".to_owned(),
            ));
        }
        Ok(stories.into_iter().nth(index))
    }

    /// Omit review state only within transferred body content and its selected notes.
    pub(crate) fn omit_glossary_selected_body_comments(
        &mut self,
        include_final_section_properties: bool,
    ) -> Result<Vec<u8>> {
        self.omit_glossary_selected_body_comments_in(include_final_section_properties, None)
            .map(|(bytes, _)| bytes)
    }

    pub(crate) fn omit_glossary_selected_body_comments_in(
        &mut self,
        include_final_section_properties: bool,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(
        Vec<u8>,
        Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    )> {
        let part = self.doc_part_name.clone();
        let storage = self
            .package
            .get_part_shared(&part)
            .ok_or_else(|| Error::Other("fragment main source disappeared".into()))?;
        let storage = Arc::clone(storage);
        let xml = storage.as_slice();
        let parent = if let Some(writer) = template.as_deref_mut() {
            let source = if let Some((stored, source)) = &writer.sources.published_main
                && Arc::ptr_eq(stored, &storage)
            {
                Some(Arc::clone(source))
            } else {
                writer
                    .lineage
                    .original_part_source(writer.operation, &part, &storage)?
                    .map(Arc::new)
            };
            Some(source.ok_or_else(|| {
                Error::Other("glossary omission lost its actual main source".into())
            })?)
        } else {
            None
        };
        let owner = scan_story_owners(xml, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document fragment has no main body".into()))?;
        let start = direct_story_content_items(xml, &owner)?
            .first()
            .map(|item| item.full.start)
            .ok_or_else(|| Error::Other("document fragment main body is empty".into()))?;
        let end = story_owner_content_end(xml, &owner)?;
        let selected_source =
            if let (Some(writer), Some(parent)) = (template.as_deref_mut(), parent.as_ref()) {
                Some(package_authoritative_body_source(
                    writer.operation,
                    parent.as_ref(),
                    false,
                    &BTreeMap::new(),
                )?)
            } else {
                None
            };
        let selected_bytes = if selected_source.is_none() {
            package_authoritative_body_fragment(xml, false, &BTreeMap::new())?
        } else {
            Vec::new()
        };
        let selected = selected_source
            .as_ref()
            .map_or(selected_bytes.as_slice(), |source| source.as_bytes());
        self.omit_glossary_fragment_note_comments_in(&part, selected, template.as_deref_mut())?;
        let mut omitted_source = None;
        let omitted = if let (Some(writer), Some(selected_source)) =
            (template.as_deref_mut(), selected_source)
        {
            let edits = Self::omit_comment_marker_edits(selected_source.as_bytes())?;
            omitted_source = Some(apply_template_source_edits(
                writer.operation,
                selected_source,
                edits,
            )?);
            Vec::new()
        } else {
            Self::omit_comment_markers(&selected_bytes)?
        };
        let mut section_replacement = None;
        if include_final_section_properties {
            for item in scan_story_items(xml, &owner, None)? {
                if item.direct_owner_child
                    && item.kind == StoryItemKind::PreservedNode
                    && content_fragment_root_is_section_properties(xml, &item)?
                {
                    let scope = story_namespace_scope_at(xml, item.full.start)?;
                    if let (Some(writer), Some(parent)) = (template.as_deref_mut(), parent.as_ref())
                    {
                        let section = writer.operation.compose_sources(&[(
                            Some(parent.as_ref()),
                            item.full.clone(),
                            &[],
                        )])?;
                        let section = writer.operation.close_source_namespaces(section, &scope)?;
                        self.omit_glossary_fragment_note_comments_in(
                            &part,
                            section.as_bytes(),
                            Some(writer),
                        )?;
                        let edits = Self::omit_comment_marker_edits(section.as_bytes())?;
                        let section =
                            apply_template_source_edits(writer.operation, section, edits)?;
                        section_replacement = Some((item.full, Vec::new(), Some(section)));
                    } else {
                        let section =
                            close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
                        self.omit_glossary_fragment_note_comments(&part, &section)?;
                        section_replacement =
                            Some((item.full, Self::omit_comment_markers(&section)?, None));
                    }
                    break;
                }
            }
        }
        if let (Some(writer), Some(parent), Some(omitted)) =
            (template, parent.as_ref(), omitted_source)
        {
            let mut pieces = vec![
                (Some(parent.as_ref()), 0..start, &[][..]),
                (Some(&omitted), 0..omitted.as_bytes().len(), &[][..]),
            ];
            if let Some((range, _, Some(section))) = &section_replacement {
                pieces.push((Some(parent.as_ref()), end..range.start, &[]));
                pieces.push((Some(section), 0..section.as_bytes().len(), &[]));
                pieces.push((
                    Some(parent.as_ref()),
                    range.end..parent.as_bytes().len(),
                    &[],
                ));
            } else {
                pieces.push((Some(parent.as_ref()), end..parent.as_bytes().len(), &[]));
            }
            let updated = writer.operation.compose_sources(&pieces)?;
            set_story_source_xml_in(self, &part, Vec::new(), Some((writer, updated)))?;
            let source = writer
                .sources
                .published_main
                .as_ref()
                .ok_or_else(|| Error::Other("glossary omission lost its published source".into()))?
                .1
                .clone();
            let retained = writer.operation.repeat_source(&source)?;
            return Ok((writer.operation.finish_source(retained)?, Some(source)));
        }
        let mut updated = xml.to_vec();
        if let Some((range, section, _)) = section_replacement {
            updated.splice(range, section);
        }
        updated.splice(start..end, omitted);
        set_story_source_xml(self, &part, updated.clone())?;
        Ok((updated, None))
    }

    /// Omit only selected note review markers before glossary dependency capture.
    pub(crate) fn omit_glossary_fragment_note_comments(
        &mut self,
        source_part: &str,
        selected: &[u8],
    ) -> Result<()> {
        self.omit_glossary_fragment_note_comments_in(source_part, selected, None)
    }

    fn omit_glossary_fragment_note_comments_in(
        &mut self,
        source_part: &str,
        selected: &[u8],
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut pending = crate::field::fragment_note_references(selected)?;
        let mut seen = HashSet::new();
        while let Some((kind, id)) = pending.pop() {
            if !seen.insert((kind, id)) {
                continue;
            }
            let relationship_type = match kind {
                StoryKind::Footnote => rel_types::FOOTNOTES,
                StoryKind::Endnote => rel_types::ENDNOTES,
                _ => return Err(Error::Other("invalid glossary note dependency kind".into())),
            };
            if part_name_identity(source_part) != part_name_identity(&self.doc_part_name)
                && self
                    .package
                    .get_part_rels(source_part)
                    .iter()
                    .flat_map(|rels| &rels.items)
                    .any(|relationship| relationship.rel_type == relationship_type)
            {
                return Err(Error::Other(format!(
                    "cannot project glossary-local {kind:?} {id} through main numeric note ownership"
                )));
            }
            let story = self.note_story(kind, id)?.ok_or_else(|| {
                Error::Other(format!(
                    "glossary fragment {kind:?} {id} has no proved main note owner"
                ))
            })?;
            let (source, owner) = self.story_source_and_owner(&story)?;
            let part = source.part_name;
            let scope = story_namespace_scope_at(source.xml.as_ref(), owner.full.start)?;
            if let Some(writer) = template.as_deref_mut() {
                let storage = self
                    .package
                    .get_part_shared(&part)
                    .ok_or_else(|| Error::Other("glossary note source disappeared".into()))?;
                let parent = writer
                    .take_related_source(&part, storage)?
                    .ok_or_else(|| Error::Other("glossary note omission lost its source".into()))?;
                let note = writer.operation.compose_sources(&[(
                    Some(parent.as_ref()),
                    owner.full.clone(),
                    &[],
                )])?;
                let note = writer.operation.close_source_namespaces(note, &scope)?;
                pending.extend(crate::field::fragment_note_references(note.as_bytes())?);
                let edits = Self::omit_comment_marker_edits(note.as_bytes())?;
                let note = apply_template_source_edits(writer.operation, note, edits)?;
                let updated = writer.operation.compose_sources(&[
                    (Some(parent.as_ref()), 0..owner.full.start, &[]),
                    (Some(&note), 0..note.as_bytes().len(), &[]),
                    (
                        Some(parent.as_ref()),
                        owner.full.end..parent.as_bytes().len(),
                        &[],
                    ),
                ])?;
                let retained = writer.operation.repeat_source(&updated)?;
                let bytes = writer.operation.finish_source(retained)?;
                set_story_source_xml_in(self, &part, bytes, Some((writer, updated)))?;
            } else {
                let mut xml = source.xml.into_owned();
                let note = close_content_fragment_namespaces(&xml[owner.full.clone()], &scope)?;
                pending.extend(crate::field::fragment_note_references(&note)?);
                let omitted = Self::omit_comment_markers(&note)?;
                xml.splice(owner.full, omitted);
                set_story_source_xml(self, &part, xml)?;
            }
        }
        Ok(())
    }

    pub(crate) fn fragment_note_dependency(
        &self,
        kind: StoryKind,
        id: i32,
    ) -> Result<(String, Vec<u8>)> {
        self.fragment_note_dependency_in(kind, id, None)
            .map(|(part, xml, _)| (part, xml))
    }

    pub(crate) fn fragment_note_dependency_in(
        &self,
        kind: StoryKind,
        id: i32,
        scoped: Option<(
            &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
            &TemplateMainSources,
        )>,
    ) -> Result<(
        String,
        Vec<u8>,
        Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    )> {
        let story = self.note_story(kind, id)?.ok_or_else(|| {
            Error::Other(format!(
                "document fragment {kind:?} reference {id} has no note owner"
            ))
        })?;
        let (source, owner) = self.story_source_and_owner(&story)?;
        let xml = source.xml.as_ref();
        let scope = story_namespace_scope_at(xml, owner.full.start)?;
        let produced = if let Some((operation, sources)) = scoped
            && let Some(bytes) = self.package.get_part_shared(&source.part_name)
            && let Some(parent) = sources.related_source(&source.part_name, bytes)
        {
            let selected = operation.compose_sources(&[(Some(parent), owner.full.clone(), &[])])?;
            let selected = Arc::new(operation.close_source_namespaces(selected, &scope)?);
            let retained = operation.repeat_source(&selected)?;
            let bytes = operation.finish_source(retained)?;
            Some((bytes, selected))
        } else {
            None
        };
        if let Some((bytes, selected)) = produced {
            Ok((source.part_name, bytes, Some(selected)))
        } else {
            Ok((
                source.part_name,
                close_content_fragment_namespaces(&xml[owner.full], &scope)?,
                None,
            ))
        }
    }

    pub(crate) fn fragment_comment_dependency(&self, id: i32) -> Result<(String, Vec<u8>)> {
        let part = self.comments_part_name.as_ref().ok_or_else(|| {
            Error::Other("REF f annotation has no relationship-owned comments part".into())
        })?;
        let comments = self
            .comments
            .as_ref()
            .ok_or_else(|| Error::Other("REF f annotation has no source definitions".into()))?;
        let matching = comments
            .comments
            .iter()
            .enumerate()
            .filter(|(_, comment)| comment.id == id)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(Error::Other(
                "REF f annotation source ID is missing or ambiguous".into(),
            ));
        }
        let owners = self
            .stories()?
            .into_iter()
            .filter(|story| story.kind == StoryKind::Comment && story.part_name == *part)
            .collect::<Vec<_>>();
        if owners.len() != comments.comments.len() {
            return Err(Error::Other(
                "REF f annotation owner inventory differs from its definitions".into(),
            ));
        }
        let (source, owner) = self.story_source_and_owner(&owners[matching[0].0])?;
        let scope = story_namespace_scope_at(source.xml.as_ref(), owner.full.start)?;
        Ok((
            source.part_name,
            close_content_fragment_namespaces(&source.xml.as_ref()[owner.full], &scope)?,
        ))
    }

    pub(crate) fn publish_fragment_comment_staged(
        &mut self,
        part: &str,
        xml: Vec<u8>,
        replace_id: Option<i32>,
    ) -> Result<()> {
        if self.comments_part_name.as_deref() != Some(part) {
            return Err(Error::Other(
                "REF f annotation copy has a different relationship owner".into(),
            ));
        }
        let source = self
            .package
            .get_part(part)
            .ok_or_else(|| Error::Other("REF f destination comments part is missing".into()))?;
        let updated = if let Some(id) = replace_id {
            let matching = self
                .comments
                .as_ref()
                .ok_or_else(|| {
                    Error::Other("REF f cached annotation definitions are absent".into())
                })?
                .comments
                .iter()
                .enumerate()
                .filter(|(_, comment)| comment.id == id)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                return Err(Error::Other(
                    "REF f cached annotation owner is missing or ambiguous".into(),
                ));
            }
            let owners = self
                .stories()?
                .into_iter()
                .filter(|story| story.kind == StoryKind::Comment && story.part_name == part)
                .collect::<Vec<_>>();
            if owners.len() != self.comments.as_ref().unwrap().comments.len() {
                return Err(Error::Other(
                    "REF f cached annotation physical owners differ from their definitions".into(),
                ));
            }
            let (owner_source, owner) = self.story_source_and_owner(&owners[matching[0]])?;
            if owner_source.part_name != part {
                return Err(Error::Other(
                    "REF f cached annotation has a different relationship owner".into(),
                ));
            }
            let mut updated = source.to_vec();
            updated.splice(owner.full, xml);
            updated
        } else {
            append_story_fragments_to_root(source, b"comments", b"comment", &[xml])?
        };
        self.comments = Some(oxoffice_docx_oxml::comments::CT_Comments::from_xml(
            &updated,
        )?);
        self.package.set_part(part, updated);
        self.comments_dirty = false;
        Ok(())
    }

    pub(crate) fn ensure_fragment_note_part_staged(&mut self, kind: StoryKind) -> Result<String> {
        self.ensure_fragment_note_part_staged_in(kind, None)
    }

    pub(crate) fn ensure_fragment_note_part_staged_in(
        &mut self,
        kind: StoryKind,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<String> {
        let (preferred, relationship_type, content_type) = match kind {
            StoryKind::Footnote => (
                "/word/footnotes.xml",
                rel_types::FOOTNOTES,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
            ),
            StoryKind::Endnote => (
                "/word/endnotes.xml",
                rel_types::ENDNOTES,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
            ),
            _ => return Err(Error::Other("invalid fragment note story kind".to_owned())),
        };
        let existing = self.note_part_name(kind)?;
        let part = self.reserve_document_part_bundle(
            existing.as_deref(),
            preferred,
            relationship_type,
            content_type,
        )?;
        if kind == StoryKind::Footnote {
            self.footnotes_part_name = Some(part.clone());
        }
        if self.package.get_part(&part).is_none() {
            let notes = oxoffice_docx_oxml::footnotes::CT_Footnotes::new();
            let (xml, produced) = if let Some(template) = template {
                let source = template.operation.source_model_notes(
                    &notes,
                    kind == StoryKind::Endnote,
                    &[],
                    &[],
                    &[],
                    false,
                )?;
                let retained = template.operation.repeat_source(&source)?;
                (
                    template.operation.finish_source(retained)?,
                    Some((template, source)),
                )
            } else {
                (
                    if kind == StoryKind::Footnote {
                        notes.to_xml_footnotes()?
                    } else {
                        notes.to_xml_endnotes()?
                    },
                    None,
                )
            };
            self.package.set_part(&part, xml);
            if let Some((template, source)) = produced {
                template.sources.put_related_source(
                    &part,
                    self.package
                        .get_part_shared(&part)
                        .expect("actual new note part"),
                    source,
                );
            }
        }
        Ok(part)
    }

    pub(crate) fn publish_fragment_note_staged(
        &mut self,
        kind: StoryKind,
        part_name: &str,
        note_xml: Vec<u8>,
        replace_id: Option<i32>,
    ) -> Result<()> {
        self.publish_fragment_note_staged_in(kind, part_name, note_xml, replace_id, None)
    }

    pub(crate) fn publish_fragment_note_staged_in(
        &mut self,
        kind: StoryKind,
        part_name: &str,
        note_xml: Vec<u8>,
        replace_id: Option<i32>,
        template: Option<(
            &mut TemplateMainWriter<'_, '_>,
            Arc<oxoffice_docx_oxml::document::ScopedWordSource>,
        )>,
    ) -> Result<()> {
        let source = self.package.get_part(part_name).ok_or_else(|| {
            Error::Other(format!(
                "document fragment destination note part {part_name} is missing"
            ))
        })?;
        let (root, item) = match kind {
            StoryKind::Footnote => (b"footnotes".as_slice(), b"footnote".as_slice()),
            StoryKind::Endnote => (b"endnotes".as_slice(), b"endnote".as_slice()),
            _ => return Err(Error::Other("invalid fragment note story kind".to_owned())),
        };
        if let Some((template, note)) = template {
            let storage = self
                .package
                .get_part_shared(part_name)
                .expect("actual destination note part");
            let parent = template
                .take_related_source(part_name, storage)?
                .ok_or_else(|| Error::Other("destination note part lost its source".into()))?;
            let updated = if let Some(id) = replace_id {
                let story = self.note_story(kind, id)?.ok_or_else(|| {
                    Error::Other("REF f cached note replacement owner is absent".into())
                })?;
                let (owner_source, owner) = self.story_source_and_owner(&story)?;
                if owner_source.part_name != part_name {
                    return Err(Error::Other(
                        "REF f cached note replacement has a different relationship owner".into(),
                    ));
                }
                let updated = Arc::new(template.operation.compose_sources(&[
                    (Some(parent.as_ref()), 0..owner.full.start, &[]),
                    (Some(note.as_ref()), 0..note.as_bytes().len(), &[]),
                    (
                        Some(parent.as_ref()),
                        owner.full.end..parent.as_bytes().len(),
                        &[],
                    ),
                ])?);
                template.operation.source_notes(&updated)?;
                updated
            } else {
                append_story_sources_to_root(template.operation, parent, &[note], root, item)?
            };
            let retained = template.operation.repeat_source(&updated)?;
            let bytes = template.operation.finish_source(retained)?;
            self.package.set_part(part_name, bytes);
            template.sources.put_related_source(
                part_name,
                self.package
                    .get_part_shared(part_name)
                    .expect("actual appended note publication"),
                updated,
            );
            return Ok(());
        }
        let updated = if let Some(id) = replace_id {
            let story = self.note_story(kind, id)?.ok_or_else(|| {
                Error::Other("REF f cached note replacement owner is absent".into())
            })?;
            let (owner_source, owner) = self.story_source_and_owner(&story)?;
            if owner_source.part_name != part_name {
                return Err(Error::Other(
                    "REF f cached note replacement has a different relationship owner".into(),
                ));
            }
            let mut updated = source.to_vec();
            updated.splice(owner.full, note_xml);
            oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&updated)?;
            updated
        } else {
            append_story_fragments_to_root(source, root, item, &[note_xml])?
        };
        self.package.set_part(part_name, updated);
        Ok(())
    }

    pub(super) fn note_part_name(&self, kind: StoryKind) -> Result<Option<String>> {
        if kind == StoryKind::Footnote {
            return Ok(self.footnotes_part_name.clone());
        }
        if kind != StoryKind::Endnote {
            return Err(Error::Other("invalid note story kind".to_owned()));
        }
        let mut targets = self
            .package
            .get_part_rels(&self.doc_part_name)
            .map(|relationships| relationships.items.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|relationship| {
                relationship.rel_type == rel_types::ENDNOTES
                    && relationship_is_internal(relationship)
            })
            .map(|relationship| {
                OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
            });
        let first = targets.next();
        if targets.next().is_some() {
            return Err(Error::Other(
                "endnotes part has multiple internal relationships".to_owned(),
            ));
        }
        Ok(first)
    }

    /// The ids of the normal endnotes the endnotes part holds, read once;
    /// empty without that part. An id held twice is ambiguous, as for
    /// `endnote_story`.
    pub(crate) fn normal_endnote_ids(&self) -> Result<HashSet<i32>> {
        let Some(part_name) = self.note_part_name(StoryKind::Endnote)? else {
            return Ok(HashSet::new());
        };
        let xml = self
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("endnotes part {part_name} is missing")))?;
        let mut ids = HashSet::new();
        for note in oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?
            .footnotes
            .iter()
            .filter(|note| note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
        {
            if !ids.insert(note.id) {
                return Err(Error::Other(format!(
                    "{:?} ID {} is ambiguous",
                    StoryKind::Endnote,
                    note.id
                )));
            }
        }
        Ok(ids)
    }

    fn note_story(&self, kind: StoryKind, id: i32) -> Result<Option<StoryId>> {
        let Some(part_name) = self.note_part_name(kind)? else {
            if kind == StoryKind::Footnote
                && self.footnotes.footnotes.iter().any(|note| {
                    note.id == id
                        && note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal
                })
            {
                return Err(Error::Other(
                    "normal footnote has no relationship-resolved part".to_owned(),
                ));
            }
            return Ok(None);
        };
        let endnotes;
        let notes = if kind == StoryKind::Footnote {
            &self.footnotes
        } else {
            let xml = self
                .package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("endnotes part {part_name} is missing")))?;
            endnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?;
            &endnotes
        };
        let normal = notes
            .footnotes
            .iter()
            .filter(|note| note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
            .collect::<Vec<_>>();
        let mut matches = normal.iter().enumerate().filter(|(_, note)| note.id == id);
        let Some((index, _)) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(Error::Other(format!("{kind:?} ID {id} is ambiguous")));
        }
        // The note part alone holds the note stories: the body is not read.
        let xml = self.package.get_part(&part_name).ok_or_else(|| {
            Error::Other(format!("{kind:?} story targets missing part {part_name}"))
        })?;
        let stories = scan_story_owners(xml, kind)?
            .into_iter()
            .filter(|owner| owner.kind == kind)
            .map(|owner| StoryId {
                kind,
                part_name: part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            })
            .collect::<Vec<_>>();
        if stories.len() != normal.len() {
            return Err(Error::Other(
                "note story count differs from the normal note stream".to_owned(),
            ));
        }
        Ok(stories.into_iter().nth(index))
    }

    /// Move a normal footnote before another normal footnote without changing IDs.
    pub fn move_footnote_before(&mut self, id: i32, before_id: i32) -> Result<()> {
        self.move_note_before(StoryKind::Footnote, id, before_id)
    }

    /// Move a normal endnote before another normal endnote without changing IDs.
    pub fn move_endnote_before(&mut self, id: i32, before_id: i32) -> Result<()> {
        self.move_note_before(StoryKind::Endnote, id, before_id)
    }

    fn move_note_before(&mut self, kind: StoryKind, id: i32, before_id: i32) -> Result<()> {
        let label = if kind == StoryKind::Footnote {
            "footnote"
        } else {
            "endnote"
        };
        let source_story = self
            .note_story(kind, id)?
            .ok_or_else(|| Error::Other(format!("{label} ID {id} does not exist")))?;
        let target_story = self
            .note_story(kind, before_id)?
            .ok_or_else(|| Error::Other(format!("{label} ID {before_id} does not exist")))?;
        if id == before_id {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let part_name = source_story.part_name.clone();
        let mut xml = candidate
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("{label}s part {part_name} is missing")))?
            .to_vec();
        let owners = scan_story_owners(&xml, kind)?;
        let source = owners
            .iter()
            .find(|owner| owner.kind == kind && owner.owner_index == source_story.owner_index)
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: source_story.clone(),
            })?;
        let target = owners
            .iter()
            .find(|owner| owner.kind == kind && owner.owner_index == target_story.owner_index)
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: target_story.clone(),
            })?;
        let fragment = xml[source.full.clone()].to_vec();
        let source_range = source.full.clone();
        let target_start = target.full.start;
        xml.drain(source_range.clone());
        let insertion = if source_range.start < target_start {
            target_start - source_range.len()
        } else {
            target_start
        };
        xml.splice(insertion..insertion, fragment);
        set_story_source_xml(&mut candidate, &part_name, xml)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove a normal footnote and every matching reference in the main body.
    pub fn remove_footnote(&mut self, id: i32) -> Result<()> {
        self.remove_note(StoryKind::Footnote, id)
    }

    /// Remove a normal endnote and every matching reference in the main body.
    pub fn remove_endnote(&mut self, id: i32) -> Result<()> {
        self.remove_note(StoryKind::Endnote, id)
    }

    fn remove_note(&mut self, kind: StoryKind, id: i32) -> Result<()> {
        match self.remove_note_in_place(kind, id) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => self.remove_note_staged(kind, id),
            Err(error) => Err(error),
        }
    }

    /// Remove the note `id` of `kind` (`StoryKind::Footnote` or
    /// `StoryKind::Endnote`) in place when the in-place route takes it, and
    /// answer its story; `None`, with nothing changed, when it leaves the
    /// note to the staged route. The binding publishes the body blocks of
    /// the edit and that story.
    #[doc(hidden)]
    pub fn remove_note_in_place(&mut self, kind: StoryKind, id: i32) -> Result<Option<StoryId>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.remove_note_in_place_unchecked(kind, id);
        #[cfg(debug_assertions)]
        if !matches!(result, Ok(None)) {
            let answer = match &result {
                Ok(_) => Ok(()),
                Err(error) => Err(Error::Other(error.to_string())),
            };
            self.assert_in_place_matches_staged_declarations_aside(before, &answer, |staged| {
                staged.remove_note_staged(kind, id)
            });
        }
        result
    }

    /// [`Self::remove_note`] on the live document, at the cost of the note's
    /// part and the body blocks that hold its references (the written
    /// fragment walk finds them): each such block is written alone, loses
    /// its references as the staged route's byte remover removes them, and
    /// is read back. `None`, with nothing changed, for what the staged route
    /// keeps: a stale notes part, a note that carries comment marks, a
    /// block that does not read back. The relationships only the note used
    /// are released in the edit's journal.
    fn remove_note_in_place_unchecked(
        &mut self,
        kind: StoryKind,
        id: i32,
    ) -> Result<Option<StoryId>> {
        let label = if kind == StoryKind::Footnote {
            "footnote"
        } else {
            "endnote"
        };
        let story = self
            .note_story(kind, id)?
            .ok_or_else(|| Error::Other(format!("{label} ID {id} does not exist")))?;
        if kind == StoryKind::Footnote && self.footnotes_dirty {
            return Ok(None);
        }
        let part_name = story.part_name.clone();
        let Some(part) = self.package.get_part(&part_name) else {
            return Ok(None);
        };
        let mut notes_xml = part.to_vec();
        let Some(owner) = scan_story_owners(&notes_xml, kind)?
            .into_iter()
            .find(|owner| owner.kind == kind && owner.owner_index == story.owner_index)
        else {
            return Ok(None);
        };
        // A comment marked in the note goes with it: the staged route
        // reconciles the comments.
        if crate::comments::comment_source_markers(&notes_xml)?
            .iter()
            .any(|marker| {
                owner.full.start <= marker.span.start && marker.span.end <= owner.full.end
            })
        {
            return Ok(None);
        }
        let referenced_before = xml_attribute_values(&notes_xml)?;
        notes_xml.drain(owner.full);
        let referenced_after = xml_attribute_values(&notes_xml)?;
        let hits = self.body_slots_referencing_note(kind, id);
        let mut blocks = Vec::with_capacity(hits.len());
        for hit in &hits {
            let Some(block) = self.body_block_after_byte_edit(hit.slot, |xml| {
                remove_note_references(xml, id, kind)
            })?
            else {
                return Ok(None);
            };
            blocks.push((hit.slot, block));
        }
        let mut units = vec![if kind == StoryKind::Footnote {
            EditUnit::Footnotes
        } else {
            EditUnit::Endnotes
        }];
        // One unit per hit block, and the package for the relationships the
        // note releases.
        units.extend(hits.iter().map(|hit| EditUnit::BodyBlocks {
            slot: hit.slot,
            count: 1,
        }));
        units.push(EditUnit::Parts);
        self.edit_in_place_with_body_blocks(&units, blocks, |document| {
            // The pictures and links only the removed note showed go with
            // it, journaled.
            document.release_unreferenced_relationships(
                &part_name,
                &referenced_before,
                &referenced_after,
            );
            set_story_source_xml(document, &part_name, notes_xml)
        })?;
        Ok(Some(story))
    }

    /// The staged route of [`Self::remove_note`]: the whole main part is
    /// serialized and scanned, the debug oracle of the in-place route.
    fn remove_note_staged(&mut self, kind: StoryKind, id: i32) -> Result<()> {
        let label = if kind == StoryKind::Footnote {
            "footnote"
        } else {
            "endnote"
        };
        let story = self
            .note_story(kind, id)?
            .ok_or_else(|| Error::Other(format!("{label} ID {id} does not exist")))?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let part_name = story.part_name.clone();
        let mut notes_xml = candidate
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("{label}s part {part_name} is missing")))?
            .to_vec();
        let owner = scan_story_owners(&notes_xml, kind)?
            .into_iter()
            .find(|owner| owner.kind == kind && owner.owner_index == story.owner_index)
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: story.clone(),
            })?;
        let referenced_before = xml_attribute_values(&notes_xml)?;
        notes_xml.drain(owner.full);
        // The pictures and links only the removed note showed go with it.
        let referenced_after = xml_attribute_values(&notes_xml)?;
        candidate.release_unreferenced_relationships(
            &part_name,
            &referenced_before,
            &referenced_after,
        );
        let document_xml = candidate
            .package
            .get_part(&candidate.doc_part_name)
            .ok_or_else(|| Error::Other("main document part is missing".to_owned()))?;
        let without_references = remove_note_references(document_xml, id, kind)?;
        let document_part = candidate.doc_part_name.clone();
        set_story_source_xml(&mut candidate, &document_part, without_references)?;
        set_story_source_xml(&mut candidate, &part_name, notes_xml)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}

impl Document {
    pub(super) fn reserve_footnotes_bundle(&mut self) -> Result<()> {
        let existing = self.footnotes_part_name.clone();
        let part_name = self
            .reserve_document_part_bundle(
                existing.as_deref(),
                "/word/footnotes.xml",
                rel_types::FOOTNOTES,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
            )
            .map_err(|error| {
                Error::Other(format!("footnotes relationship allocation failed: {error}"))
            })?;
        self.footnotes_part_name = Some(part_name);
        Ok(())
    }
}

impl Document {
    /// The id of the mark style of a note reference: the document's own,
    /// found by id or by its built-in name (a localized Word file names it
    /// otherwise), else the style created here, as Word defines it.
    fn note_reference_style_id(&mut self, kind: StoryKind) -> Result<String> {
        let (id, name) = if kind == StoryKind::Endnote {
            ("EndnoteReference", "endnote reference")
        } else {
            ("FootnoteReference", "footnote reference")
        };
        if let Some(found) = built_in_character_style_id(&self.styles, id, name) {
            return Ok(found);
        }
        // The id taken by a style of another type is named as before rather
        // than defined twice.
        if self.styles.get_by_id(id).is_some() {
            return Ok(id.to_owned());
        }
        self.add_style(note_reference_style(kind, &self.styles))?;
        Ok(id.to_owned())
    }
}

/// The smallest note id from 2 that `occupied` does not hold.
fn smallest_free_note_id(occupied: &HashSet<i32>, what: &str) -> Result<i32> {
    let mut id = 2_i32;
    while occupied.contains(&id) {
        id = id
            .checked_add(1)
            .ok_or_else(|| Error::Other(format!("{what} ID range is exhausted")))?;
    }
    Ok(id)
}

/// The body run that shows note `id`, in the note reference style.
fn note_reference_run(kind: StoryKind, id: i32, mark_style: &str) -> CT_R {
    let mut run = CT_R::new("");
    run.properties = Some(CT_RPr {
        style_id: Some(mark_style.to_owned()),
        ..Default::default()
    });
    run.content = vec![if kind == StoryKind::Endnote {
        RunContent::EndnoteRef {
            id,
            custom_mark: None,
        }
    } else {
        RunContent::FootnoteRef {
            id,
            custom_mark: None,
        }
    }];
    run
}
