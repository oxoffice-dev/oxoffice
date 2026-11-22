//! Lists and numbering: list items, definitions, instances, levels and their links to styles.

use super::*;

/// The list a list item joins, as [`Document::begin_list_item`] finds or
/// starts it.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListItemKind {
    /// The first list whose first level is a bullet.
    Bullet,
    /// The decimal list the last restarted item at that level goes on, or
    /// the first decimal list.
    Numbered,
}

/// What an in-place numbering edit replaced: the numbering model and its
/// part name, and the package graph and identifier registry, where the edit
/// reserves the numbering part and its identifiers. The package graph shares
/// its part bytes, so keeping it costs the parts and relationships, not the
/// content. [`Document::undo_numbering_edit`] puts it back, so that a call
/// whose later step fails leaves the document as it was; dropping it keeps
/// the edit.
#[doc(hidden)]
pub struct NumberingJournal {
    package: OpcPackage,
    identifiers: DocumentIdentifiers,
    numbering: Option<CT_Numbering>,
    numbering_part_name: Option<String>,
    package_signatures_invalidated: bool,
}

impl Document {
    /// Whether the first level of the given numbering instance renders as
    /// bullets (true) or numbers (false), with the replacement level the
    /// instance sets in place of its definition's and a missing format read
    /// as decimal, as layout draws it. A `none` level, which draws no marker,
    /// reads as false. None if the id or its first level is unknown, or the
    /// format is not a standard one.
    pub fn numbering_is_bullet(&self, num_id: u32) -> Option<bool> {
        match self.effective_numbering_level(num_id, 0)?.num_fmt.as_ref() {
            Some(ST_NumberFormat::Bullet) => Some(true),
            Some(ST_NumberFormat::Other(_)) => None,
            _ => Some(false),
        }
    }

    // Concrete instance replacement levels are complete effective levels, just
    // as in layout. Keep the existing public abstract-level projection unchanged.
    fn effective_numbering_format(&self, num_id: u32, level: u32) -> Option<&str> {
        Some(
            self.effective_numbering_level(num_id, level)?
                .num_fmt
                .as_ref()
                .map(ST_NumberFormat::to_str)
                .unwrap_or("decimal"),
        )
    }

    fn effective_numbering_level(&self, num_id: u32, level: u32) -> Option<&CT_Lvl> {
        if num_id == 0 {
            return None;
        }
        let numbering = self.numbering.as_ref()?;
        let instance = numbering.nums.iter().find(|value| value.num_id == num_id)?;
        let definition = numbering.get_abstract_num_for(num_id)?;
        let base = definition.levels.iter().find(|value| value.ilvl == level)?;
        Some(
            instance
                .level_overrides
                .iter()
                .find(|value| value.ilvl == level)
                .and_then(|value| value.level.as_ref())
                .unwrap_or(base),
        )
    }

    /// Resolve the public reader projection for one numbering level.
    ///
    /// It reads the definition's level: a replacement level the instance
    /// sets is not applied here, while [`Self::numbering_is_bullet`] and
    /// layout apply it.
    ///
    /// `has_unmodeled_properties` reports extra XML or attributes attached to
    /// this numbering instance, definition, or level, excluding retained XML
    /// namespace declarations. Modeled producer metadata and raw preservation
    /// overlays for level properties are not reported.
    pub fn numbering_level(&self, num_id: u32, level: u32) -> Option<NumberingLevel<'_>> {
        if num_id == 0 {
            return None;
        }
        let numbering = self.numbering.as_ref()?;
        let instance = numbering.nums.iter().find(|item| item.num_id == num_id)?;
        let definition = numbering
            .abstract_nums
            .iter()
            .find(|item| item.abstract_num_id == instance.abstract_num_id)?;
        let level = definition.levels.iter().find(|item| item.ilvl == level)?;
        let format = level.num_fmt.as_ref();

        Some(NumberingLevel {
            level: level.ilvl,
            format: format
                .map(NumberingFormat::from_st)
                .unwrap_or(NumberingFormat::Decimal),
            format_name: format.map(ST_NumberFormat::to_str).unwrap_or("decimal"),
            start: level.start.unwrap_or(1),
            suffix: level
                .suffix
                .map(ListLevelSuffix::from_st)
                .unwrap_or(ListLevelSuffix::Tab),
            level_text: level.lvl_text.as_deref(),
            alignment: level.lvl_jc.map(list_level_alignment),
            paragraph_style: level.p_style.as_deref(),
            restart: level.restart.map(ListLevelRestart::from_st),
            legal_numbering: level.legal,
            indent_left: level
                .ppr
                .as_ref()
                .and_then(|properties| properties.ind_left),
            indent_hanging: level
                .ppr
                .as_ref()
                .and_then(|properties| properties.ind_hanging),
            indent_first_line: level
                .ppr
                .as_ref()
                .and_then(|properties| properties.ind_first_line),
            marker_properties: level.rpr.as_ref(),
            template_code: level.template_code.as_deref(),
            tentative: level.tentative,
            has_unmodeled_properties: !instance.extra_xml.is_empty()
                || !definition.extra_xml.is_empty()
                || !level.extra_xml.is_empty()
                || instance
                    .extra_attributes
                    .iter()
                    .chain(&definition.extra_attributes)
                    .chain(&level.extra_attributes)
                    .any(|(name, _)| name != "xmlns" && !name.starts_with("xmlns:")),
            has_paragraph_presentation: level.ppr.as_ref().is_some_and(|properties| {
                Self::has_list_paragraph_presentation(level.ilvl, properties)
            }),
            has_marker_presentation: level
                .rpr
                .as_ref()
                .is_some_and(|properties| properties != &CT_RPr::default()),
        })
    }

    fn has_list_paragraph_presentation(level: u32, properties: &CT_PPr) -> bool {
        let Some(standard_indent) = (u64::from(level) + 1)
            .checked_mul(720)
            .and_then(|value| i32::try_from(value).ok())
        else {
            return true;
        };
        let standard = CT_PPr {
            ind_left: Some(oxoffice_docx_oxml::units::Twips(standard_indent)),
            ind_hanging: Some(oxoffice_docx_oxml::units::Twips(360)),
            ..CT_PPr::default()
        };
        properties != &standard
    }
}

impl Document {
    // ---- Numbering/Lists ----

    /// Ensure a numbering part exists.
    ///
    /// The relationship and content-type override are added by
    /// [`Self::flush_to_package`], which knows the resolved part name and will
    /// not create a second numbering relationship if one already exists.
    fn ensure_numbering(&mut self) -> &mut CT_Numbering {
        self.numbering.get_or_insert_with(CT_Numbering::new)
    }

    pub(super) fn reserve_numbering_bundle(&mut self) -> Result<()> {
        self.identifiers.observe_package_graph(&self.package)?;
        if let Some(numbering) = &self.numbering {
            self.identifiers.abstract_numbering_ids.extend(
                numbering
                    .abstract_nums
                    .iter()
                    .map(|definition| definition.abstract_num_id),
            );
            self.identifiers
                .numbering_instance_ids
                .extend(numbering.nums.iter().map(|instance| instance.num_id));
        }
        let part_name = match self.numbering_part_name.as_deref() {
            Some(part_name) => part_name.to_owned(),
            None => self
                .identifiers
                .reserve_preferred_part_name(DEFAULT_NUMBERING_PART)?,
        };
        let already_linked =
            self.package
                .get_part_rels(&self.doc_part_name)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        relationship.rel_type == rel_types::NUMBERING
                            && relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(
                                &self.doc_part_name,
                                &relationship.target,
                            ) == part_name
                    })
                });
        let pending_id = if already_linked {
            None
        } else {
            Some(
                self.identifiers
                    .reserve_relationship_id_checked(&self.doc_part_name)
                    .map_err(|error| {
                        Error::Other(format!("numbering relationship allocation failed: {error}"))
                    })?,
            )
        };
        self.package
            .content_types
            .add_override(&part_name, NUMBERING_CONTENT_TYPE);
        self.identifiers.register_content_type_override(&part_name);
        if let Some(id) = pending_id {
            let target = relative_target(&self.doc_part_name, &part_name);
            self.package
                .get_or_create_part_rels(&self.doc_part_name)
                .add_with_id(&id, rel_types::NUMBERING, &target);
        }
        self.numbering_part_name = Some(part_name);
        Ok(())
    }
}

impl Document {
    /// The numbering instance identifiers that content names, whether an
    /// instance defines them or not: the typed paragraphs of the body, with
    /// the raw XML they keep (where drawings and text boxes are) and the raw
    /// blocks of the body, the typed notes and comments, and the header,
    /// footer, endnote and glossary parts, which the package holds as they
    /// are. It reads the live models, not a serialization, so a copy that a
    /// save left in the package never counts.
    pub(super) fn content_numbering_instance_ids(&self) -> HashSet<u32> {
        fn named(ids: &mut HashSet<u32>, paragraph: &CT_P) {
            if let Some(id) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.num_id)
            {
                ids.insert(id);
            }
            for (_, raw) in &paragraph.extra_xml {
                raw_numbering_instance_ids(raw, ids);
            }
            for raw in paragraph.runs.iter().flat_map(|run| &run.extra_xml) {
                raw_numbering_instance_ids(raw, ids);
            }
        }
        let mut ids = HashSet::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            named(&mut ids, paragraph);
        });
        for item in &self.document.body.content {
            if let BodyContent::RawXml(raw) = item {
                raw_numbering_instance_ids(raw, &mut ids);
            }
        }
        for paragraph in self
            .footnotes
            .footnotes
            .iter()
            .flat_map(|note| &note.paragraphs)
        {
            named(&mut ids, paragraph);
        }
        for paragraph in self
            .comments
            .iter()
            .flat_map(|comments| &comments.comments)
            .flat_map(|comment| &comment.paragraphs)
        {
            named(&mut ids, paragraph);
        }
        let typed = [
            Some(&self.doc_part_name),
            self.footnotes_part_name.as_ref(),
            self.comments_part_name
                .as_ref()
                .filter(|_| self.comments.is_some()),
        ];
        for part_name in self.word_story_part_names() {
            if typed.contains(&Some(&part_name)) {
                continue;
            }
            if let Some(properties) = self
                .package
                .get_part(&part_name)
                .and_then(|bytes| xml_paragraph_numbering_properties(bytes).ok())
            {
                ids.extend(properties.iter().filter_map(|properties| properties.num_id));
            }
        }
        ids
    }

    /// Reserve a numbering instance identifier that no instance holds and no
    /// content names, so that a new instance never numbers content that names
    /// a list the document does not define. On a document whose content
    /// names only defined instances, this is the identifier the registry
    /// alone gives.
    fn reserve_unnamed_numbering_instance_id(&mut self) -> Result<u32> {
        let mut occupied = self.identifiers.numbering_instance_ids.clone();
        occupied.extend(self.content_numbering_instance_ids());
        let id = reserve_u32_or_lowest_free(&mut occupied, 1, "numbering instance")?;
        self.identifiers.numbering_instance_ids.insert(id);
        Ok(id)
    }

    /// Keep what a numbering edit is about to replace.
    fn numbering_journal(&self) -> NumberingJournal {
        NumberingJournal {
            package: self.package.clone(),
            identifiers: self.identifiers.clone(),
            numbering: self.numbering.clone(),
            numbering_part_name: self.numbering_part_name.clone(),
            package_signatures_invalidated: self.package_signatures_invalidated,
        }
    }

    /// Put back what an in-place numbering edit replaced, and drop the layout
    /// derived from the edited numbering.
    #[doc(hidden)]
    pub fn undo_numbering_edit(&mut self, journal: NumberingJournal) {
        self.package = journal.package;
        self.identifiers = journal.identifiers;
        self.numbering = journal.numbering;
        self.numbering_part_name = journal.numbering_part_name;
        self.package_signatures_invalidated = journal.package_signatures_invalidated;
        self.drop_numbering_layout();
    }

    /// Drop the layout derived from the numbering. A numbering edit changes
    /// neither the typed body nor the main part, so the kept answer of
    /// [`Self::unsavable_body_edit`] stays.
    fn drop_numbering_layout(&mut self) {
        self.layout_cache
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.deterministic_layout_cache
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    /// Run `edit`, an addition to the numbering part, on this document, and
    /// return its value with the journal that undoes it. A refusal or a
    /// panic is undone at once. A success leaves what a staged commit
    /// leaves: a signed package records a package mutation and the layout
    /// derived from the numbering is dropped.
    ///
    /// Debug builds run `edit` on a staged clone too and compare, and check
    /// that the edit left no numbering reference of the content unresolved
    /// that was not already, the check an addition no longer runs. `edit` is
    /// `Fn` for that second run.
    fn edit_numbering_journaled<T: Clone + PartialEq + std::fmt::Debug>(
        &mut self,
        edit: impl Fn(&mut Self) -> Result<T>,
    ) -> Result<(T, NumberingJournal)> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let journal = self.numbering_journal();
        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| edit(self))) {
            Ok(result) => result,
            Err(panic) => {
                self.undo_numbering_edit(journal);
                std::panic::resume_unwind(panic)
            }
        };
        #[cfg(debug_assertions)]
        let outcome = result.as_ref().cloned().map_err(ToString::to_string);
        let result = match result {
            Ok(value) => {
                self.package_signatures_invalidated |=
                    self.package.package_rels.items.iter().any(|relationship| {
                        relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN
                    }) && (journal.package_signatures_invalidated
                        || crate::embedded::synchronized_package_mutation_invalidates_signature(
                            &journal.package,
                            &self.package,
                        ));
                self.drop_numbering_layout();
                Ok((value, journal))
            }
            Err(error) => {
                self.undo_numbering_edit(journal);
                Err(error)
            }
        };
        #[cfg(debug_assertions)]
        self.assert_numbering_edit_matches_staged(before, &edit, outcome);
        result
    }

    /// [`Self::edit_numbering_journaled`], keeping the edit.
    fn edit_numbering<T: Clone + PartialEq + std::fmt::Debug>(
        &mut self,
        edit: impl Fn(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.edit_numbering_journaled(edit)
            .map(|(value, _kept)| value)
    }

    /// Debug proof that an in-place numbering edit left what the staged edit
    /// leaves, or refused with its error and left the document as it was,
    /// and that it left the numbering references of the content that do not
    /// resolve as they were: it neither resolves one (a new instance never
    /// takes an identifier content names) nor leaves another unresolved.
    /// When neither document's content can be serialized, its references
    /// cannot be read, and the save reports that content.
    #[cfg(debug_assertions)]
    fn assert_numbering_edit_matches_staged<T: PartialEq + std::fmt::Debug>(
        &self,
        before: Self,
        edit: &impl Fn(&mut Self) -> Result<T>,
        outcome: std::result::Result<T, String>,
    ) {
        let mut staged = before.clone_for_staging();
        let staged_outcome = edit(&mut staged);
        let expected = match (outcome, staged_outcome) {
            (Ok(direct), Ok(value)) => {
                assert_eq!(
                    direct, value,
                    "in-place numbering edit returned another value"
                );
                let mut committed = before.clone_for_staging();
                staged.invalidate_layout();
                committed.commit_staged_mutation(staged);
                committed
            }
            (Err(direct), Err(staged)) => {
                assert_eq!(
                    direct,
                    staged.to_string(),
                    "in-place numbering edit refused otherwise"
                );
                before.clone_for_staging()
            }
            (Ok(_), Err(staged)) => panic!("in-place numbering edit hid the staged error {staged}"),
            (Err(direct), Ok(_)) => panic!("in-place numbering edit refused with {direct}"),
        };
        assert!(
            expected.package.same_package_as(&self.package),
            "in-place numbering edit left another package graph"
        );
        assert_eq!(
            expected.identifiers, self.identifiers,
            "in-place numbering edit left another identifier registry"
        );
        assert_eq!(expected.numbering, self.numbering);
        assert_eq!(expected.numbering_part_name, self.numbering_part_name);
        assert_eq!(
            expected.package_signatures_invalidated,
            self.package_signatures_invalidated
        );
        match (
            before.dangling_numbering_references(),
            self.dangling_numbering_references(),
        ) {
            (Ok(was), Ok(is)) => assert_eq!(
                was, is,
                "a numbering addition changed which references of the content resolve"
            ),
            (Err(_), Err(_)) => {}
            (was, is) => panic!(
                "numbering references readable before {} and after {}",
                was.is_ok(),
                is.is_ok()
            ),
        }
    }

    /// Add a bullet list item at the given indentation level (0-based).
    ///
    /// If no bullet list definition exists yet, one is created automatically.
    /// Returns a mutable `Paragraph` for further configuration. A character
    /// XML 1.0 cannot carry is dropped; [`Self::try_add_bullet_list_item`]
    /// refuses it instead.
    pub fn add_bullet_list_item(&mut self, text: &str, level: u32) -> Paragraph<'_> {
        self.try_add_bullet_list_item(&oxoffice_core::xml::strip_non_xml_characters(text), level)
            .expect("the historical infallible list helper requires a valid document and level")
    }

    /// Checked counterpart. Invalid text and levels leave the document
    /// unchanged, and so does a numbering graph that is not valid or a list
    /// that has no such level. Content that already names a missing list is
    /// left as it is: the item checks the reference it makes.
    pub fn try_add_bullet_list_item(&mut self, text: &str, level: u32) -> Result<Paragraph<'_>> {
        self.add_list_item(ListItemKind::Bullet, text, level)
    }

    /// Add a numbered list item at the given indentation level (0-based).
    ///
    /// If no numbered list definition exists yet, one is created automatically.
    /// Returns a mutable `Paragraph` for further configuration. A character
    /// XML 1.0 cannot carry is dropped; [`Self::try_add_numbered_list_item`]
    /// refuses it instead.
    pub fn add_numbered_list_item(&mut self, text: &str, level: u32) -> Paragraph<'_> {
        self.try_add_numbered_list_item(&oxoffice_core::xml::strip_non_xml_characters(text), level)
            .expect("the historical infallible list helper requires a valid document and level")
    }

    /// Checked counterpart, refusing as [`Self::try_add_bullet_list_item`] does.
    pub fn try_add_numbered_list_item(&mut self, text: &str, level: u32) -> Result<Paragraph<'_>> {
        self.try_add_numbered_list_item_with_restart(text, level, false)
    }

    /// Checked numbered authoring with persisted explicit restart continuation.
    ///
    /// A restart gives the item an instance of its own through
    /// [`Self::restart_numbering`], which checks every numbering reference of
    /// the content.
    pub fn try_add_numbered_list_item_with_restart(
        &mut self,
        text: &str,
        level: u32,
        restart: bool,
    ) -> Result<Paragraph<'_>> {
        if restart {
            let mut candidate = self.clone_for_staging();
            let (num_id, _kept) = candidate.begin_list_item(ListItemKind::Numbered, text, level)?;
            candidate.append_body_paragraph(Self::list_item_paragraph(text, num_id, level));
            let index = candidate.paragraph_count() - 1;
            candidate.restart_numbering(index, 1)?;
            candidate.validate_numbering_graph()?;
            self.commit_staged_mutation(candidate);
            return match self.document.body.content.last_mut() {
                Some(BodyContent::Paragraph(p)) => Ok(Paragraph { inner: p }),
                _ => unreachable!("the list item is the last body child"),
            };
        }
        self.add_list_item(ListItemKind::Numbered, text, level)
    }

    /// A list item without a restart: its numbering half, then its paragraph
    /// appended in place. The body could take a typed edit before, so it keeps
    /// that answer, as the typed inserts do.
    fn add_list_item(
        &mut self,
        kind: ListItemKind,
        text: &str,
        level: u32,
    ) -> Result<Paragraph<'_>> {
        let (num_id, _kept) = self.begin_list_item(kind, text, level)?;
        let slot = self.content_count();
        self.append_body_paragraph(Self::list_item_paragraph(text, num_id, level));
        self.keep_body_savable_after_typed_insert(slot, true);
        match self.document.body.content.last_mut() {
            Some(BodyContent::Paragraph(p)) => Ok(Paragraph { inner: p }),
            _ => unreachable!("the list item is the last body child"),
        }
    }

    /// The numbering half of a list item, in place: join the document's
    /// bullet or numbered list, or start one, and return the instance the
    /// item's paragraph names with the journal that undoes this half.
    ///
    /// It refuses, leaving the document unchanged, a level over 8, text XML
    /// cannot carry, a body a typed edit cannot leave savable (as the typed
    /// inserts refuse it), a numbering graph that is not valid, and a list
    /// that has no such level. It does not read the content's other
    /// numbering references: an addition cannot leave one unresolved, and a
    /// new list takes an identifier no content names. The paragraph is
    /// [`Self::list_item_paragraph`], appended by the caller.
    #[doc(hidden)]
    pub fn begin_list_item(
        &mut self,
        kind: ListItemKind,
        text: &str,
        level: u32,
    ) -> Result<(u32, NumberingJournal)> {
        if level > 8 {
            return Err(Error::Other(
                "list level must be between 0 and 8".to_owned(),
            ));
        }
        oxoffice_core::xml::reject_non_xml_characters("text", text)?;
        self.refuse_unsavable_body_edit()?;
        self.validate_numbering_model()?;
        self.edit_numbering_journaled(|document| {
            document.reserve_numbering_bundle()?;
            let num_id = match kind {
                ListItemKind::Bullet => document.bullet_list_instance()?,
                ListItemKind::Numbered => document.numbered_list_instance(level)?,
            };
            document.validate_numbering_model()?;
            match document.numbering_reference_error(num_id, level) {
                Some(error) => Err(error),
                None => Ok(num_id),
            }
        })
    }

    /// The first instance whose definition's first level is a bullet, or a
    /// new nine-level bullet list.
    fn bullet_list_instance(&mut self) -> Result<u32> {
        let existing = self.numbering.as_ref().and_then(|numbering| {
            numbering
                .nums
                .iter()
                .find(|n| {
                    numbering
                        .get_abstract_num_for(n.num_id)
                        .map(|a| {
                            a.levels.first().and_then(|l| l.num_fmt.as_ref())
                                == Some(&oxoffice_docx_oxml::numbering::ST_NumberFormat::Bullet)
                        })
                        .unwrap_or(false)
                })
                .map(|instance| instance.num_id)
        });
        if let Some(num_id) = existing {
            return Ok(num_id);
        }
        let abstract_id = self.identifiers.reserve_abstract_numbering_id()?;
        let num_id = self.reserve_unnamed_numbering_instance_id()?;
        Ok(self.ensure_numbering().add_list_with_ids(
            &[(
                oxoffice_docx_oxml::numbering::ST_NumberFormat::Bullet,
                Some(1),
            )],
            abstract_id,
            num_id,
        ))
    }

    /// The instance of the last body paragraph at `level` whose decimal
    /// level restarts, so that the list it restarted goes on; else the first
    /// decimal instance whose `level` is not a bullet; else a new nine-level
    /// numbered list.
    fn numbered_list_instance(&mut self, level: u32) -> Result<u32> {
        let existing = self.numbering.as_ref().and_then(|numbering| {
            numbering
                .nums
                .iter()
                .find(|instance| {
                    numbering
                        .get_abstract_num_for(instance.num_id)
                        .is_some_and(|definition| {
                            definition
                                .levels
                                .first()
                                .and_then(|value| value.num_fmt.as_ref())
                                == Some(&ST_NumberFormat::Decimal)
                        })
                        && !matches!(
                            self.effective_numbering_format(instance.num_id, level),
                            Some("bullet" | "none")
                        )
                })
                .map(|instance| instance.num_id)
        });
        let continuation = self
            .document
            .body
            .content
            .iter()
            .rev()
            .filter_map(|item| {
                let BodyContent::Paragraph(p) = item else {
                    return None;
                };
                let properties = p.properties.as_ref()?;
                let id = properties.num_id?;
                // A `w:numId` without `w:ilvl` numbers at the level its style
                // chain gives, else 0, as the layout reads it.
                let shown = properties.num_ilvl.unwrap_or_else(|| {
                    super::numbering_types::effective_numbering_reference(
                        self.numbering.as_ref(),
                        &self.styles,
                        properties,
                    )
                    .map_or(0, |(_, level)| level)
                });
                if shown != level {
                    return None;
                }
                let instance = self
                    .numbering
                    .as_ref()?
                    .nums
                    .iter()
                    .find(|n| n.num_id == id)?;
                (self
                    .effective_numbering_format(id, level)
                    .is_some_and(|name| name == "decimal")
                    && instance
                        .level_overrides
                        .iter()
                        .any(|v| v.ilvl == level && v.start_override.is_some()))
                .then_some(id)
            })
            .next();
        if let Some(id) = continuation {
            return Ok(id);
        }
        if let Some(num_id) = existing {
            return Ok(num_id);
        }
        let abstract_id = self.identifiers.reserve_abstract_numbering_id()?;
        let num_id = self.reserve_unnamed_numbering_instance_id()?;
        Ok(self.ensure_numbering().add_list_with_ids(
            &[(
                oxoffice_docx_oxml::numbering::ST_NumberFormat::Decimal,
                Some(1),
            )],
            abstract_id,
            num_id,
        ))
    }

    /// The paragraph of a list item: `text` in one run, numbered by `num_id`
    /// at `level`.
    #[doc(hidden)]
    pub fn list_item_paragraph(text: &str, num_id: u32, level: u32) -> CT_P {
        let mut p = Self::detached_text_paragraph(text);
        p.properties = Some(CT_PPr {
            num_id: Some(num_id),
            num_ilvl: Some(level),
            ..Default::default()
        });
        p
    }

    /// Append `paragraph` to the body, as [`Self::add_paragraph`] appends its
    /// own.
    #[doc(hidden)]
    pub fn append_body_paragraph(&mut self, paragraph: CT_P) -> Paragraph<'_> {
        self.invalidate_layout();
        self.document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
        match self.document.body.content.last_mut() {
            Some(BodyContent::Paragraph(p)) => Paragraph { inner: p },
            _ => unreachable!("the paragraph was just appended"),
        }
    }
    /// Restart one current direct-body numbered paragraph without changing its peers.
    pub fn restart_numbering(&mut self, paragraph: usize, start: u32) -> Result<u32> {
        if self.content_index_of_paragraph(paragraph).is_none() {
            return Err(Error::Other(
                "numbering restart requires a direct body paragraph".into(),
            ));
        }
        self.validate_numbering_graph()?;
        let properties = self
            .paragraph(paragraph)
            .and_then(|p| p.inner.properties.as_ref())
            .ok_or_else(|| Error::Other("paragraph has no explicit numbering".into()))?;
        // A `w:numId` without `w:ilvl` numbers at the level its style chain
        // gives, else 0, as the layout reads it.
        let (id, level) = properties
            .num_id
            .map(|id| {
                let level = properties.num_ilvl.unwrap_or_else(|| {
                    super::numbering_types::effective_numbering_reference(
                        self.numbering.as_ref(),
                        &self.styles,
                        properties,
                    )
                    .map_or(0, |(_, level)| level)
                });
                (id, level)
            })
            .ok_or_else(|| Error::Other("paragraph has no explicit numbering level".into()))?;
        if level > 8
            || self
                .effective_numbering_format(id, level)
                .is_none_or(|name| matches!(name, "bullet" | "none"))
        {
            return Err(Error::Other(
                "numbering restart requires a numbered non-bullet level".into(),
            ));
        }
        let source = self
            .numbering
            .as_ref()
            .and_then(|numbering| numbering.nums.iter().find(|item| item.num_id == id))
            .ok_or_else(|| Error::Other("numbering instance is absent".into()))?;
        if !restart_can_copy(source) {
            return Err(Error::Other(
                "numbering restart cannot project unsupported override source".into(),
            ));
        }
        let mut instance = self
            .numbering_instance(id)
            .expect("the instance was found above");
        if let Some(value) = instance
            .level_overrides
            .iter_mut()
            .find(|v| v.level == level)
        {
            value.start = Some(start);
        } else {
            instance
                .level_overrides
                .push(NumberingLevelOverride::new(level).start(start));
        }
        let mut candidate = self.clone_for_staging();
        let new_id =
            candidate.add_numbering_instance(instance.definition_id, &instance.level_overrides)?;
        candidate
            .paragraph_mut(paragraph)
            .ok_or_else(|| Error::Other("numbering paragraph disappeared".into()))?
            .set_numbering(new_id, level);
        candidate.validate_numbering_graph()?;
        self.commit_staged_mutation(candidate);
        Ok(new_id)
    }

    /// Create a list definition with explicit per-level formats and return
    /// its numId.
    ///
    /// Unlike [`Self::add_bullet_list_item`] / [`Self::add_numbered_list_item`],
    /// which share one bullet and one numbered definition per document, every
    /// call creates a fresh definition — so separate lists restart their
    /// numbering, and one definition can mix formats across levels (e.g. a
    /// bullet list whose nested level is decimal). Attach paragraphs with
    /// [`crate::Paragraph::set_numbering`].
    ///
    /// `levels[i]` configures level `i`; deeper unspecified levels fall back
    /// to the standard template rotation for the last specified format's
    /// family. An empty slice produces the standard numbered template. Word
    /// supports nine levels, so entries after index eight are ignored.
    ///
    /// ```no_run
    /// use oxoffice_docx::{Document, ListLevel};
    ///
    /// let mut doc = Document::new();
    /// let num_id = doc.add_list_definition(&[
    ///     ListLevel::bullet(),
    ///     ListLevel::decimal().start(3),
    /// ]);
    /// doc.add_paragraph("first bullet").set_numbering(num_id, 0);
    /// doc.add_paragraph("third decimal").set_numbering(num_id, 1);
    /// ```
    pub fn add_list_definition(&mut self, levels: &[ListLevel]) -> u32 {
        if levels
            .iter()
            .take(9)
            .enumerate()
            .any(|(level, value)| validate_list_level(level as u32, value).is_err())
        {
            return 0;
        }
        let formats: Vec<(ST_NumberFormat, Option<u32>)> = levels
            .iter()
            .take(9)
            .map(|level| (level.format.to_st(), level.start))
            .collect();
        self.edit_numbering(|document| {
            document
                .reserve_numbering_bundle()
                .expect("an in-memory document can allocate a numbering part");
            let abstract_id = document
                .identifiers
                .reserve_abstract_numbering_id()
                .expect("an in-memory document cannot exhaust numbering identifiers");
            let num_id = document
                .reserve_unnamed_numbering_instance_id()
                .expect("an in-memory document cannot exhaust numbering identifiers");
            let num_id =
                document
                    .ensure_numbering()
                    .add_list_with_ids(&formats, abstract_id, num_id);
            if let Some(definition) = document.numbering.as_mut().and_then(|numbering| {
                numbering
                    .abstract_nums
                    .iter_mut()
                    .find(|definition| definition.abstract_num_id == abstract_id)
            }) {
                for (level, value) in levels.iter().take(9).enumerate() {
                    let existing = &definition.levels[level];
                    definition.levels[level] = merge_list_level(existing, value)
                        .expect("list level was validated before staged allocation");
                }
            }
            document.validate_numbering_model()?;
            Ok(num_id)
        })
        .unwrap_or(0)
    }

    /// Inspect all abstract numbering definitions in package order.
    pub fn numbering_definitions(&self) -> Vec<NumberingDefinition> {
        self.numbering
            .as_ref()
            .map(|numbering| {
                numbering
                    .abstract_nums
                    .iter()
                    .map(|definition| NumberingDefinition {
                        id: definition.abstract_num_id,
                        levels: definition
                            .levels
                            .iter()
                            .map(|level| NumberingDefinitionLevel {
                                level: level.ilvl,
                                properties: list_level_from_ct(level),
                            })
                            .collect(),
                        paragraph_style_links: definition
                            .levels
                            .iter()
                            .map(|level| level.p_style.clone())
                            .collect(),
                        has_unmodeled_properties: !definition.extra_xml.is_empty()
                            || !definition.extra_attributes.is_empty()
                            || definition
                                .nsid_raw
                                .as_ref()
                                .is_some_and(|(_, raw, prefixes)| {
                                    typed_numbering_leaf_has_unmodeled(raw, prefixes)
                                })
                            || definition.multi_level_type_raw.as_ref().is_some_and(
                                |(_, raw, prefixes)| {
                                    typed_numbering_leaf_has_unmodeled(raw, prefixes)
                                },
                            )
                            || definition
                                .tmpl_raw
                                .as_ref()
                                .is_some_and(|(_, raw, prefixes)| {
                                    typed_numbering_leaf_has_unmodeled(raw, prefixes)
                                })
                            || definition.levels.iter().any(|level| {
                                numbering_level_has_unmodeled(level)
                                    || matches!(
                                        level.num_fmt.as_ref(),
                                        Some(ST_NumberFormat::Other(_))
                                    )
                            }),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Inspect one abstract numbering definition by identifier.
    pub fn numbering_definition(&self, id: u32) -> Option<NumberingDefinition> {
        self.numbering_definitions()
            .into_iter()
            .find(|definition| definition.id == id)
    }

    /// Inspect all numbering instances in package order.
    pub fn numbering_instances(&self) -> Vec<NumberingInstance> {
        self.numbering
            .as_ref()
            .map(|numbering| {
                numbering
                    .nums
                    .iter()
                    .map(|instance| NumberingInstance {
                        id: instance.num_id,
                        definition_id: instance.abstract_num_id,
                        level_overrides: instance
                            .level_overrides
                            .iter()
                            .map(|value| NumberingLevelOverride {
                                level: value.ilvl,
                                start: value.start_override,
                                replacement: value.level.as_ref().map(list_level_from_ct),
                                paragraph_style_link: value
                                    .level
                                    .as_ref()
                                    .and_then(|level| level.p_style.clone()),
                                has_unmodeled_properties: numbering_override_has_unmodeled(value),
                            })
                            .collect(),
                        has_unmodeled_properties: !instance.extra_xml.is_empty()
                            || !instance.extra_attributes.is_empty()
                            || instance.abstract_num_id_raw.as_ref().is_some_and(
                                |(_, raw, prefixes)| {
                                    typed_numbering_leaf_has_unmodeled(raw, prefixes)
                                },
                            )
                            || instance
                                .level_overrides
                                .iter()
                                .any(numbering_override_has_unmodeled),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Inspect one numbering instance by identifier.
    pub fn numbering_instance(&self, id: u32) -> Option<NumberingInstance> {
        self.numbering_instances()
            .into_iter()
            .find(|instance| instance.id == id)
    }

    /// Create an abstract numbering definition and return its identifier.
    ///
    /// It checks the numbering graph, not the numbering references of the
    /// content, which an addition cannot leave unresolved: content that
    /// already names a missing list does not refuse it.
    pub fn add_numbering_definition(&mut self, levels: &[ListLevel]) -> Result<u32> {
        validate_numbering_level_slice(levels)?;
        let authored = levels
            .iter()
            .enumerate()
            .map(|(level, value)| ct_level_from_list(level as u32, value))
            .collect::<Result<Vec<_>>>()?;
        self.edit_numbering(|document| {
            document.reserve_numbering_bundle()?;
            let id = document.identifiers.reserve_abstract_numbering_id()?;
            let mut definition = CT_AbstractNum::new(id);
            definition.multi_level_type = Some("hybridMultilevel".to_owned());
            definition.levels = authored.clone();
            document.ensure_numbering().push_abstract_num(definition);
            document.validate_numbering_model()?;
            Ok(id)
        })
    }

    /// Replace the modeled levels of an existing abstract definition.
    pub fn update_numbering_definition(
        &mut self,
        id: u32,
        levels: &[NumberingDefinitionLevel],
    ) -> Result<()> {
        validate_numbering_definition_levels(levels)?;
        let mut candidate = self.clone_for_staging();
        let numbering = candidate
            .numbering
            .as_mut()
            .ok_or_else(|| Error::Other(format!("numbering definition {id} does not exist")))?;
        let definition = numbering
            .abstract_nums
            .iter_mut()
            .find(|definition| definition.abstract_num_id == id)
            .ok_or_else(|| Error::Other(format!("numbering definition {id} does not exist")))?;
        if definition.levels.len() != levels.len() {
            return Err(Error::Other(format!(
                "numbering definition {id} updates must retain its {} levels",
                definition.levels.len()
            )));
        }
        definition.levels = definition
            .levels
            .iter()
            .map(|existing| {
                let value = levels
                    .iter()
                    .find(|value| value.level == existing.ilvl)
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "numbering definition {id} update is missing level {}",
                            existing.ilvl
                        ))
                    })?;
                merge_list_level(existing, &value.properties)
            })
            .collect::<Result<Vec<_>>>()?;
        candidate.reserve_numbering_bundle()?;
        candidate.validate_numbering_graph()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove an abstract definition that no instance references.
    pub fn remove_numbering_definition(&mut self, id: u32) -> Result<bool> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Ok(false);
        };
        if !numbering
            .abstract_nums
            .iter()
            .any(|definition| definition.abstract_num_id == id)
        {
            return Ok(false);
        }
        if let Some(instance) = numbering
            .nums
            .iter()
            .find(|instance| instance.abstract_num_id == id)
        {
            return Err(Error::Other(format!(
                "numbering definition {id} is referenced by instance {}",
                instance.num_id
            )));
        }
        let mut candidate = self.clone_for_staging();
        candidate
            .numbering
            .as_mut()
            .expect("checked numbering part")
            .remove_abstract_num(id);
        candidate.validate_numbering_graph()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(true)
    }

    /// Create a numbering instance for an existing definition.
    ///
    /// It checks the numbering graph, not the numbering references of the
    /// content, as [`Self::add_numbering_definition`] does.
    pub fn add_numbering_instance(
        &mut self,
        definition_id: u32,
        overrides: &[NumberingLevelOverride],
    ) -> Result<u32> {
        self.edit_numbering(|document| {
            document.reserve_numbering_bundle()?;
            let numbering = document.ensure_numbering();
            if !numbering
                .abstract_nums
                .iter()
                .any(|definition| definition.abstract_num_id == definition_id)
            {
                return Err(Error::Other(format!(
                    "numbering definition {definition_id} does not exist"
                )));
            }
            let level_overrides = overrides
                .iter()
                .map(numbering_override_from_public)
                .collect::<Result<Vec<_>>>()?;
            let id = document.reserve_unnamed_numbering_instance_id()?;
            document.ensure_numbering().push_num(CT_Num {
                num_id: id,
                abstract_num_id: definition_id,
                abstract_num_id_raw: None,
                level_overrides,
                extra_xml: Vec::new(),
                extra_attributes: Vec::new(),
            });
            document.validate_numbering_model()?;
            Ok(id)
        })
    }

    /// Replace the definition and overrides used by an existing instance.
    ///
    /// A style linked both ways to a level of the instance keeps its link:
    /// an update that would drop the level's name of the style is refused,
    /// as only [`Self::unlink_style_from_numbering`] removes a link.
    pub fn update_numbering_instance(
        &mut self,
        id: u32,
        definition_id: u32,
        overrides: &[NumberingLevelOverride],
    ) -> Result<()> {
        let links = self.two_way_style_links(id);
        let mut candidate = self.clone_for_staging();
        let numbering = candidate
            .numbering
            .as_mut()
            .ok_or_else(|| Error::Other(format!("numbering instance {id} does not exist")))?;
        if !numbering
            .abstract_nums
            .iter()
            .any(|definition| definition.abstract_num_id == definition_id)
        {
            return Err(Error::Other(format!(
                "numbering definition {definition_id} does not exist"
            )));
        }
        let instance = numbering
            .nums
            .iter_mut()
            .find(|instance| instance.num_id == id)
            .ok_or_else(|| Error::Other(format!("numbering instance {id} does not exist")))?;
        let mut updated = Vec::with_capacity(overrides.len());
        for value in overrides {
            let existing = instance
                .level_overrides
                .iter()
                .find(|existing| existing.ilvl == value.level);
            updated.push(numbering_override_for_update(existing, value)?);
        }
        instance.abstract_num_id = definition_id;
        instance.level_overrides = updated;
        let kept = candidate.two_way_style_links(id);
        if links.iter().any(|link| !kept.contains(link)) {
            return Err(Error::Other(
                "standalone numbering style-link mutation is not supported yet".to_owned(),
            ));
        }
        candidate.reserve_numbering_bundle()?;
        candidate.validate_numbering_graph()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// The styles linked both ways to a level of instance `id`, with that
    /// level: the style points at the level, which names the style back.
    fn two_way_style_links(&self, id: u32) -> Vec<(String, u32)> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Vec::new();
        };
        let Some(instance) = numbering.nums.iter().find(|item| item.num_id == id) else {
            return Vec::new();
        };
        let definition = numbering.get_abstract_num_for(id);
        self.styles
            .styles
            .iter()
            .filter_map(|style| {
                let properties = style.ppr.as_ref()?;
                if properties.num_id != Some(id) {
                    return None;
                }
                let level = properties.num_ilvl.unwrap_or(0);
                let named = instance
                    .level_overrides
                    .iter()
                    .find(|item| item.ilvl == level)
                    .and_then(|item| item.level.as_ref())
                    .or_else(|| definition?.levels.iter().find(|item| item.ilvl == level))?
                    .p_style
                    .as_deref();
                (named == Some(style.style_id.as_str())).then(|| (style.style_id.clone(), level))
            })
            .collect()
    }

    /// Remove an unreferenced numbering instance.
    pub fn remove_numbering_instance(&mut self, id: u32) -> Result<bool> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Ok(false);
        };
        if !numbering.nums.iter().any(|instance| instance.num_id == id) {
            return Ok(false);
        }
        if self.document_references_numbering_instance(id)? {
            return Err(Error::Other(format!(
                "numbering instance {id} is referenced by document content"
            )));
        }
        let mut candidate = self.clone_for_staging();
        candidate
            .numbering
            .as_mut()
            .expect("checked numbering part")
            .remove_num(id);
        candidate.validate_numbering_graph()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(true)
    }

    /// Validate the complete typed numbering graph: definitions, instances,
    /// the styles linked to them, and every numbering reference of the
    /// content, which this reads from every story part.
    pub fn validate_numbering_graph(&self) -> Result<()> {
        self.validate_numbering_model()?;
        self.validate_content_numbering_references()
    }

    /// The numbering graph without the content: definitions, instances and
    /// the styles linked to them. It reads the numbering and styles models,
    /// not the stories.
    fn validate_numbering_model(&self) -> Result<()> {
        let numbering = self.numbering.as_ref();
        let Some(numbering) = numbering else {
            for style in &self.styles.styles {
                if style.style_type == StyleType::Paragraph
                    && style
                        .ppr
                        .as_ref()
                        .and_then(|properties| properties.num_id)
                        .is_some_and(|num_id| num_id != 0)
                {
                    return Err(Error::Other(format!(
                        "style '{}' references a missing numbering part",
                        style.style_id
                    )));
                }
            }
            return Ok(());
        };
        let mut definition_ids = HashSet::new();
        for definition in &numbering.abstract_nums {
            if !definition_ids.insert(definition.abstract_num_id) {
                return Err(Error::Other(format!(
                    "duplicate numbering definition id {}",
                    definition.abstract_num_id
                )));
            }
            let mut levels = HashSet::new();
            for level in &definition.levels {
                if level.ilvl > 8 || !levels.insert(level.ilvl) {
                    return Err(Error::Other(format!(
                        "numbering definition {} has invalid or duplicate level {}",
                        definition.abstract_num_id, level.ilvl
                    )));
                }
                validate_ct_numbering_level(level)?;
                if let Some(style_id) = level.p_style.as_deref()
                    && self.styles.get_by_id(style_id).is_none()
                {
                    return Err(Error::Other(format!(
                        "numbering definition {} references missing style '{style_id}'",
                        definition.abstract_num_id
                    )));
                }
            }
        }
        let mut instance_ids = HashSet::new();
        for instance in &numbering.nums {
            if instance.num_id == 0 || !instance_ids.insert(instance.num_id) {
                return Err(Error::Other(format!(
                    "invalid or duplicate numbering instance id {}",
                    instance.num_id
                )));
            }
            let definition = numbering
                .abstract_nums
                .iter()
                .find(|definition| definition.abstract_num_id == instance.abstract_num_id)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "numbering instance {} references missing definition {}",
                        instance.num_id, instance.abstract_num_id
                    ))
                })?;
            let mut override_levels = HashSet::new();
            for level_override in &instance.level_overrides {
                if level_override.ilvl > 8 || !override_levels.insert(level_override.ilvl) {
                    return Err(Error::Other(format!(
                        "numbering instance {} has invalid or duplicate override level {}",
                        instance.num_id, level_override.ilvl
                    )));
                }
                if !definition
                    .levels
                    .iter()
                    .any(|level| level.ilvl == level_override.ilvl)
                {
                    return Err(Error::Other(format!(
                        "numbering instance {} override level {} is not owned by definition {}",
                        instance.num_id, level_override.ilvl, instance.abstract_num_id
                    )));
                }
                if let Some(level) = &level_override.level {
                    if level.ilvl != level_override.ilvl {
                        return Err(Error::Other(format!(
                            "numbering instance {} override level {} owns replacement level {}",
                            instance.num_id, level_override.ilvl, level.ilvl
                        )));
                    }
                    validate_ct_numbering_level(level)?;
                    if let Some(style_id) = level.p_style.as_deref()
                        && self.styles.get_by_id(style_id).is_none()
                    {
                        return Err(Error::Other(format!(
                            "numbering instance {} override references missing style '{style_id}'",
                            instance.num_id
                        )));
                    }
                }
            }
        }
        let mut style_links = HashMap::<&str, (u32, u32)>::new();
        for style in &self.styles.styles {
            if style.style_type != StyleType::Paragraph {
                continue;
            }
            let Some(properties) = style.ppr.as_ref() else {
                continue;
            };
            // A `w:ilvl` without a `w:numId` is schema-valid. The style takes
            // the instance of its `basedOn` chain, as layout and Word resolve
            // it, and is not numbered when the chain has none.
            let num_id = match properties.num_id {
                Some(num_id) => num_id,
                None if properties.num_ilvl.is_some() => {
                    match style::resolve_paragraph_properties(Some(&style.style_id), &self.styles)
                        .num_id
                    {
                        Some(num_id) => num_id,
                        None => continue,
                    }
                }
                None => continue,
            };
            if num_id == 0 {
                continue;
            }
            let level = properties.num_ilvl.unwrap_or(0);
            let instance = numbering
                .nums
                .iter()
                .find(|instance| instance.num_id == num_id)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "style '{}' references missing numbering instance {num_id}",
                        style.style_id
                    ))
                })?;
            let definition = numbering
                .abstract_nums
                .iter()
                .find(|definition| definition.abstract_num_id == instance.abstract_num_id)
                .expect("numbering instances were validated above");
            let base = definition
                .levels
                .iter()
                .find(|candidate| candidate.ilvl == level)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "style '{}' references numbering instance {num_id} at missing level {level}",
                        style.style_id
                    ))
                })?;
            let effective = instance
                .level_overrides
                .iter()
                .find(|value| value.ilvl == level)
                .and_then(|value| value.level.as_ref())
                .unwrap_or(base);
            // Only a level that names the style back links the two. Without
            // that, the style is numbered by the level one way, which Word
            // reads and LibreOffice writes: its List Bullet style keeps its
            // `w:numPr` and the level loses its `w:pStyle`.
            if effective.p_style.as_deref() == Some(style.style_id.as_str()) {
                style_links.insert(style.style_id.as_str(), (num_id, level));
            }
        }
        for definition in &numbering.abstract_nums {
            for level in &definition.levels {
                let Some(style_id) = level.p_style.as_deref() else {
                    continue;
                };
                let linked = style_links.get(style_id).is_some_and(|(num_id, ilvl)| {
                    *ilvl == level.ilvl
                        && numbering.nums.iter().any(|instance| {
                            instance.num_id == *num_id
                                && instance.abstract_num_id == definition.abstract_num_id
                        })
                });
                if !linked {
                    return Err(Error::Other(format!(
                        "numbering definition {} level {} and style '{style_id}' are not reciprocally linked",
                        definition.abstract_num_id, level.ilvl
                    )));
                }
            }
        }
        for instance in &numbering.nums {
            for level in instance
                .level_overrides
                .iter()
                .filter_map(|value| value.level.as_ref())
            {
                let Some(style_id) = level.p_style.as_deref() else {
                    continue;
                };
                if style_links.get(style_id) != Some(&(instance.num_id, level.ilvl)) {
                    return Err(Error::Other(format!(
                        "numbering instance {} replacement level {} and style '{style_id}' are not reciprocally linked",
                        instance.num_id, level.ilvl
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every numbering reference of the content resolves: its instance
    /// exists and the instance's definition has the level. This reads every
    /// story part, so only an edit that can leave a reference unresolved runs
    /// it: removing or redefining an instance, linking a style. An addition
    /// checks the reference it makes, with [`Self::numbering_reference_error`].
    fn validate_content_numbering_references(&self) -> Result<()> {
        for (_, _, error) in self.document_numbering_references()? {
            if let Some(error) = error {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Why a numbering reference to `num_id` at `level` does not resolve, or
    /// `None` when it does.
    fn numbering_reference_error(&self, num_id: u32, level: u32) -> Option<Error> {
        numbering_reference_error_in(self.numbering.as_ref(), num_id, level)
    }

    /// The numbering references of the content that do not resolve, for the
    /// debug oracle of the numbering additions.
    #[cfg(debug_assertions)]
    fn dangling_numbering_references(&self) -> Result<std::collections::BTreeSet<(u32, u32)>> {
        Ok(self
            .document_numbering_references()?
            .into_iter()
            .filter(|(_, _, error)| error.is_some())
            .map(|(num_id, level, _)| (num_id, level))
            .collect())
    }

    /// Each numbering reference of the content with why it does not resolve,
    /// if it does not. A glossary document resolves against its own
    /// numbering and styles, and the `mc:Fallback` copy of a text box is not
    /// read twice.
    fn document_numbering_references(&self) -> Result<Vec<(u32, u32, Option<Error>)>> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let glossary = candidate.glossary_numbering()?;
        let mut references = Vec::new();
        for part_name in candidate.word_story_part_names() {
            let Some(xml) = candidate
                .package
                .get_part(&part_name)
                .filter(|bytes| identifier_xml_is_well_formed(bytes))
                .filter(|_| !glossary.skipped.contains(&part_name))
            else {
                continue;
            };
            let (numbering, styles) = if glossary.part_names.contains(&part_name) {
                (glossary.numbering.as_ref(), &glossary.styles)
            } else {
                (candidate.numbering.as_ref(), &*candidate.styles)
            };
            for direct in xml_paragraph_numbering_properties(xml)? {
                if let Some((num_id, level)) = super::numbering_types::effective_numbering_reference(
                    numbering, styles, &direct,
                ) {
                    references.push((
                        num_id,
                        level,
                        numbering_reference_error_in(numbering, num_id, level),
                    ));
                }
            }
        }
        Ok(references)
    }

    pub(crate) fn word_story_part_names(&self) -> Vec<String> {
        let mut story_parts = vec![self.doc_part_name.clone()];
        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            story_parts.extend(
                relationships
                    .items
                    .iter()
                    .filter(|relationship| {
                        relationship_is_internal(relationship)
                            && matches!(
                                relationship.rel_type.as_str(),
                                rel_types::HEADER
                                    | rel_types::FOOTER
                                    | rel_types::FOOTNOTES
                                    | rel_types::ENDNOTES
                                    | rel_types::COMMENTS
                                    | rel_types::GLOSSARY_DOCUMENT
                            )
                    })
                    .map(|relationship| {
                        OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
                    }),
            );
        }
        story_parts
    }

    fn document_references_numbering_instance(&self, id: u32) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let mut owners = candidate.word_story_part_names();
        owners.extend(candidate.styles_part_name.clone());
        for xml in owners.into_iter().filter_map(|part_name| {
            candidate
                .package
                .get_part(&part_name)
                .filter(|bytes| identifier_xml_is_well_formed(bytes))
        }) {
            if xml_references_numbering_instance(xml, id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Redefine one level (0–8) of an existing list definition, for callers
    /// that only learn a deeper level's format when content first reaches it.
    ///
    /// Returns `false` when `num_id` is unknown or `level` is out of range.
    pub fn set_list_level(&mut self, num_id: u32, level: u32, spec: ListLevel) -> bool {
        if validate_list_level(level, &spec).is_err() {
            return false;
        }
        let mut candidate = self.clone_for_staging();
        let updated = candidate.numbering.as_mut().is_some_and(|numbering| {
            let Some(abstract_id) = numbering
                .nums
                .iter()
                .find(|instance| instance.num_id == num_id)
                .map(|instance| instance.abstract_num_id)
            else {
                return false;
            };
            let Some(definition) = numbering
                .abstract_nums
                .iter_mut()
                .find(|definition| definition.abstract_num_id == abstract_id)
            else {
                return false;
            };
            let Some(index) = definition
                .levels
                .iter()
                .position(|existing| existing.ilvl == level)
            else {
                return false;
            };
            let Ok(updated) = merge_list_level(&definition.levels[index], &spec) else {
                return false;
            };
            definition.levels[index] = updated;
            true
        });
        if updated {
            candidate
                .reserve_numbering_bundle()
                .expect("an in-memory document can allocate a numbering part");
            if candidate.validate_numbering_graph().is_err() {
                return false;
            }
            candidate.invalidate_layout();
            self.commit_staged_mutation(candidate);
        }
        updated
    }

    /// Atomically associate a paragraph style with one concrete numbering level.
    ///
    /// Both the style's paragraph properties and the numbering level's
    /// paragraph-style link are published together. Existing links must either
    /// match this exact tuple or the operation fails without changing the
    /// document. A style `w:numPr` holding a `w:ilvl` and no `w:numId` is not
    /// a link, and the new link replaces its level. A repeated style ID
    /// resolves to its first definition, and the style graph is validated as
    /// in [`Document::add_style`].
    pub fn link_style_to_numbering(
        &mut self,
        style_id: &str,
        num_id: u32,
        level: u32,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.validate_numbering_graph()?;
        candidate.reserve_styles_bundle()?;
        candidate.reserve_numbering_bundle()?;

        let style_index = candidate
            .styles
            .styles
            .iter()
            .position(|style| style.style_id == style_id)
            .ok_or_else(|| Error::Other(format!("style '{style_id}' does not exist")))?;
        if candidate.styles.styles[style_index].style_type != StyleType::Paragraph {
            return Err(Error::Other(format!(
                "style '{style_id}' is not a paragraph style"
            )));
        }

        let numbering = candidate
            .numbering
            .as_ref()
            .ok_or_else(|| Error::Other(format!("numbering instance {num_id} does not exist")))?;
        let instance_index = numbering
            .nums
            .iter()
            .position(|instance| instance.num_id == num_id)
            .ok_or_else(|| Error::Other(format!("numbering instance {num_id} does not exist")))?;
        let definition_id = numbering.nums[instance_index].abstract_num_id;
        let definition_index = numbering
            .abstract_nums
            .iter()
            .position(|definition| definition.abstract_num_id == definition_id)
            .expect("the numbering graph was validated");
        let override_index = numbering.nums[instance_index]
            .level_overrides
            .iter()
            .position(|value| value.ilvl == level && value.level.is_some());
        let definition_level_index = numbering.abstract_nums[definition_index]
            .levels
            .iter()
            .position(|value| value.ilvl == level)
            .ok_or_else(|| {
                Error::Other(format!("numbering instance {num_id} has no level {level}"))
            })?;

        let style_numbering = style_numbering_level(&candidate.styles.styles[style_index]);
        // A `w:ilvl` without a `w:numId` owns no link, so the link replaces it.
        if style_numbering.0.is_some() && style_numbering != (Some(num_id), Some(level)) {
            return Err(Error::Other(format!(
                "style '{style_id}' already references a different numbering level"
            )));
        }

        for (candidate_definition_index, definition) in numbering.abstract_nums.iter().enumerate() {
            for (candidate_level_index, candidate_level) in definition.levels.iter().enumerate() {
                if candidate_level.p_style.as_deref() == Some(style_id)
                    && (candidate_definition_index, candidate_level_index)
                        != (definition_index, definition_level_index)
                {
                    return Err(Error::Other(format!(
                        "style '{style_id}' is already linked to another numbering level"
                    )));
                }
            }
        }
        for (candidate_instance_index, instance) in numbering.nums.iter().enumerate() {
            for (candidate_override_index, value) in instance.level_overrides.iter().enumerate() {
                if value
                    .level
                    .as_ref()
                    .and_then(|level| level.p_style.as_deref())
                    == Some(style_id)
                    && Some(candidate_override_index)
                        != override_index.filter(|_| candidate_instance_index == instance_index)
                {
                    return Err(Error::Other(format!(
                        "style '{style_id}' is already linked to another numbering level"
                    )));
                }
            }
        }

        let target_style = override_index
            .and_then(|index| {
                numbering.nums[instance_index].level_overrides[index]
                    .level
                    .as_ref()
            })
            .unwrap_or(&numbering.abstract_nums[definition_index].levels[definition_level_index])
            .p_style
            .as_deref();
        if target_style.is_some() && target_style != Some(style_id) {
            return Err(Error::Other(format!(
                "numbering instance {num_id} level {level} is already linked to another style"
            )));
        }

        let properties = Arc::make_mut(&mut candidate.styles).styles[style_index]
            .ppr
            .get_or_insert_with(CT_PPr::default);
        // A style already numbered at this level without `w:ilvl` keeps its
        // form; otherwise the link writes both.
        if !(properties.num_id == Some(num_id) && properties.num_ilvl.unwrap_or(0) == level) {
            properties.num_id = Some(num_id);
            properties.num_ilvl = Some(level);
        }
        let numbering = candidate
            .numbering
            .as_mut()
            .expect("the numbering part was resolved above");
        if let Some(index) = override_index {
            numbering.nums[instance_index].level_overrides[index]
                .level
                .as_mut()
                .expect("the override was selected only when it owns a level")
                .p_style = Some(style_id.to_owned());
        } else {
            numbering.abstract_nums[definition_index].levels[definition_level_index].p_style =
                Some(style_id.to_owned());
        }

        style::validate_style_graph_change(&self.styles, &candidate.styles)?;
        candidate.validate_numbering_graph()?;
        candidate.flush_to_package()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Atomically remove one exact paragraph-style numbering association.
    ///
    /// Unlinking removes the style's `w:numId` and `w:ilvl` together, so a
    /// style that held only a `w:ilvl` before it was linked does not get it
    /// back. A style that holds only a `w:ilvl`, takes its instance from its
    /// `basedOn` chain and is named back by the level loses only that name
    /// and keeps its level. A style holding only a `w:ilvl` that no level
    /// names is not linked, and unlinking it changes nothing. A style that
    /// points at the level without being named back, the one-way link
    /// LibreOffice writes, loses its numbering and the level stays as it is.
    /// A repeated style ID resolves to its first definition, and the style
    /// graph is validated as in [`Document::add_style`].
    pub fn unlink_style_from_numbering(
        &mut self,
        style_id: &str,
        num_id: u32,
        level: u32,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.validate_numbering_graph()?;

        let style_index = candidate
            .styles
            .styles
            .iter()
            .position(|style| style.style_id == style_id)
            .ok_or_else(|| Error::Other(format!("style '{style_id}' does not exist")))?;
        if candidate.styles.styles[style_index].style_type != StyleType::Paragraph {
            return Err(Error::Other(format!(
                "style '{style_id}' is not a paragraph style"
            )));
        }
        let numbering = candidate
            .numbering
            .as_ref()
            .ok_or_else(|| Error::Other(format!("numbering instance {num_id} does not exist")))?;
        let instance_index = numbering
            .nums
            .iter()
            .position(|instance| instance.num_id == num_id)
            .ok_or_else(|| Error::Other(format!("numbering instance {num_id} does not exist")))?;
        let definition_id = numbering.nums[instance_index].abstract_num_id;
        let definition_index = numbering
            .abstract_nums
            .iter()
            .position(|definition| definition.abstract_num_id == definition_id)
            .expect("the numbering graph was validated");
        let override_index = numbering.nums[instance_index]
            .level_overrides
            .iter()
            .position(|value| value.ilvl == level && value.level.is_some());
        let definition_level_index = numbering.abstract_nums[definition_index]
            .levels
            .iter()
            .position(|value| value.ilvl == level)
            .ok_or_else(|| {
                Error::Other(format!("numbering instance {num_id} has no level {level}"))
            })?;

        let style_numbering = style_numbering_level(&candidate.styles.styles[style_index]);
        let target_style = override_index
            .and_then(|index| {
                numbering.nums[instance_index].level_overrides[index]
                    .level
                    .as_ref()
            })
            .unwrap_or(&numbering.abstract_nums[definition_index].levels[definition_level_index])
            .p_style
            .as_deref();
        if style_numbering.0.is_none() && target_style.is_none() {
            return Ok(());
        }
        // A level may name back a style that takes its instance from its
        // `basedOn` chain, which is the link recorded for it.
        let inherited = style_numbering == (None, Some(level))
            && style::resolve_paragraph_properties(Some(style_id), &candidate.styles).num_id
                == Some(num_id);
        // A style may point at the level without being named back, the
        // one-way link LibreOffice writes: only the style's side goes.
        let named_back = target_style == Some(style_id);
        if !(style_numbering == (Some(num_id), Some(level)) || inherited && named_back) {
            return Err(Error::Other(format!(
                "style '{style_id}' and numbering instance {num_id} level {level} are not linked"
            )));
        }

        candidate.reserve_styles_bundle()?;
        candidate.reserve_numbering_bundle()?;
        let properties = Arc::make_mut(&mut candidate.styles).styles[style_index]
            .ppr
            .as_mut()
            .expect("the exact style link was validated above");
        if !inherited {
            properties.num_id = None;
            properties.num_ilvl = None;
        }
        let numbering = candidate
            .numbering
            .as_mut()
            .expect("the numbering part was resolved above");
        if named_back {
            if let Some(index) = override_index {
                numbering.nums[instance_index].level_overrides[index]
                    .level
                    .as_mut()
                    .expect("the override was selected only when it owns a level")
                    .p_style = None;
            } else {
                numbering.abstract_nums[definition_index].levels[definition_level_index].p_style =
                    None;
            }
        }

        style::validate_style_graph_change(&self.styles, &candidate.styles)?;
        candidate.validate_numbering_graph()?;
        candidate.flush_to_package()?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }
}

/// The values of the `numId` elements in raw XML read without its namespace
/// context, so an element of that name in any namespace counts: a superset,
/// which is what keeping identifiers out of an allocation needs.
fn raw_numbering_instance_ids(raw: &[u8], ids: &mut HashSet<u32>) {
    // `str::find` runs the optimized search of `core`, so that the raw XML a
    // body keeps on every paragraph and run costs little to read.
    let Ok(mut rest) = std::str::from_utf8(raw) else {
        return;
    };
    while let Some(at) = rest.find("numId") {
        let tag = &rest[at + 5..];
        let end = tag.find('>').unwrap_or(tag.len());
        if let Some(value) = tag[..end]
            .find("val=")
            .and_then(|position| tag[..end].get(position + 5..))
        {
            let digits = value.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(id) = value[..digits].parse::<u32>() {
                ids.insert(id);
            }
        }
        rest = &tag[end..];
    }
}

/// Why a numbering reference to `num_id` at `level` does not resolve against
/// `numbering`, or `None` when it does.
fn numbering_reference_error_in(
    numbering: Option<&CT_Numbering>,
    num_id: u32,
    level: u32,
) -> Option<Error> {
    let missing = || {
        Error::Other(format!(
            "document content references missing numbering instance {num_id} at level {level}"
        ))
    };
    let Some(numbering) = numbering else {
        return Some(missing());
    };
    let Some(instance) = numbering
        .nums
        .iter()
        .find(|instance| instance.num_id == num_id)
    else {
        return Some(missing());
    };
    let Some(definition) = numbering
        .abstract_nums
        .iter()
        .find(|definition| definition.abstract_num_id == instance.abstract_num_id)
    else {
        return Some(Error::Other(format!(
            "numbering instance {num_id} references missing definition {}",
            instance.abstract_num_id
        )));
    };
    (!definition
        .levels
        .iter()
        .any(|candidate| candidate.ilvl == level))
    .then(|| {
        Error::Other(format!(
            "document content references numbering instance {num_id} at missing level {level}"
        ))
    })
}

/// A style's direct numbering as `(numId, level)`. A `w:numId` without a
/// `w:ilvl`, as Word and python-docx write List Bullet and List Number, is
/// read at level 0, as the graph check reads it (a `w:ilvl` the style
/// inherits through `w:basedOn` is not looked at).
pub(super) fn style_numbering_level(
    style: &oxoffice_docx_oxml::styles::CT_Style,
) -> (Option<u32>, Option<u32>) {
    style
        .ppr
        .as_ref()
        .map(|properties| {
            (
                properties.num_id,
                properties.num_ilvl.or(properties.num_id.map(|_| 0)),
            )
        })
        .unwrap_or((None, None))
}
