//! Slide import: importing and transferring slides from another presentation, the preflight of what they carry, and the copy of their part graphs.

use super::*;

const MAX_SMARTART_TRANSFER_DIAGRAM_PARTS: usize = 128;
const CHART_EX_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/office/2014/relationships/chartEx";
const CHART_STYLE_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/office/2011/relationships/chartStyle";
const CHART_COLOR_STYLE_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/office/2011/relationships/chartColorStyle";
const CHART_USER_SHAPES_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartUserShapes";
const THEME_OVERRIDE_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/themeOverride";
pub(super) const VML_DRAWING_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";
pub(super) const TAGS_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tags";

/// Options for transactional complete-slide import.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportSlideOptions {
    pub layout_index: Option<usize>,
    pub insert_index: Option<usize>,
    pub keep_source_formatting: bool,
    pub keep_sections: bool,
}

impl Presentation {
    /// Copies one placeholder-free SmartArt slide from another presentation.
    ///
    /// `destination_layout_index` explicitly selects the destination-owned layout.
    /// Internal relationships are limited to that layout, images, and the five
    /// SmartArt diagram types. Internal images must not own relationships, and the
    /// copied diagram graph is limited to 128 parts. Slides with placeholders,
    /// external layouts, notes, comments, charts, embedded media, OLE objects, or
    /// other internal dependencies anywhere in the copied diagram graph are
    /// rejected without mutation.
    pub fn transfer_smartart_slide_from(
        &mut self,
        source: &Presentation,
        index: usize,
        destination_layout_index: usize,
    ) -> Result<SlideRef<'_>> {
        if index >= source.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index,
                slide_count: source.slides.len(),
            });
        }
        let destination_layout = self
            .layouts
            .get(destination_layout_index)
            .ok_or(Error::UnknownLayoutIndex {
                index: destination_layout_index,
                layout_count: self.layouts.len(),
            })?
            .part_name
            .clone();
        let source_package = source.staged_package(true)?;
        preflight_cross_presentation_transfer(&source_package, &source.slides[index])?;
        let mut staged = self.clone();
        let inserted = staged.slides.len();
        staged.copy_slide_from_in_place(
            &source_package,
            SlideRecord::clone(&source.slides[index]),
            inserted,
            "transfer SmartArt slide",
            Some(&destination_layout),
            None,
        )?;
        staged.refresh_document_metadata(MetadataEdit::Addition)?;
        self.commit_candidate(staged)?;
        self.slides
            .get(inserted)
            .map(|record| slide_ref(record))
            .ok_or(Error::UnknownSlideIndex {
                index: inserted,
                slide_count: self.slides.len(),
            })
    }

    /// Imports one slide of another presentation, using this presentation's theme.
    ///
    /// This is [`Presentation::import_slides`] for one slide, with the layout
    /// at `layout_index` and the insertion index `insert_index`. It returns
    /// the copy and drops the diagnostics, so a caller that needs to know
    /// which slide jumps were dropped calls `import_slides`.
    pub fn import_slide(
        &mut self,
        source: &Presentation,
        index: usize,
        layout_index: Option<usize>,
        insert_index: Option<usize>,
    ) -> Result<SlideRef<'_>> {
        let import = self.import_slides(
            source,
            &[index],
            &ImportSlideOptions {
                layout_index,
                insert_index,
                ..ImportSlideOptions::default()
            },
            &[],
        )?;
        let inserted = import.0[0];
        self.slides
            .get(inserted)
            .map(|record| slide_ref(record))
            .ok_or(Error::UnknownSlideIndex {
                index: inserted,
                slide_count: self.slides.len(),
            })
    }

    /// Imports slides of another presentation.
    ///
    /// `docProps/app.xml` counts the new slides and drops its title lists,
    /// which PowerPoint writes again on its next save.
    ///
    /// Without `options.keep_sections`, each copy joins the section of this
    /// presentation it lands in, as an added slide does.
    ///
    /// The slides at `indexes` are copied in that order to consecutive
    /// positions starting at `options.insert_index`, or appended when that is
    /// `None`. Every copy takes the layout at `options.layout_index` when one
    /// is given, and otherwise the first layout of this presentation named
    /// like its source slide's layout, so it takes this presentation's theme.
    /// The name match takes the first layout of that name, so a copy inside
    /// one multi-master deck should pass the source slide's own layout index.
    ///
    /// With `options.keep_source_formatting`, every copy relates to its
    /// source layout instead. A layout of this presentation equal to it is
    /// reused, and otherwise the layout is copied and joins an equal master
    /// of this presentation, or a copy of its source master and theme. Two
    /// layouts or masters are equal when their bytes and relationship ids are
    /// equal and their targets are equal by the same rule, a master's layout
    /// list aside, so the copies of one import, or of an earlier one, are
    /// reused rather than duplicated. Speaker notes stay on this
    /// presentation's one notes master, with their source appearance set on
    /// the notes page, except theme fonts the source theme leaves to
    /// automatic script selection (an empty East Asian or complex-script
    /// face): those keep their theme token, so notes text in those scripts
    /// takes this presentation's script fonts, as in PowerPoint.
    ///
    /// Shapes, placeholders, the slide background, speaker notes, SmartArt
    /// graphs, charts with their embedded workbook, style, colour, user-shape
    /// and theme-override parts, embedded objects with their preview image and
    /// legacy drawing, and modern comments are copied, and every relationship
    /// attribute is rebound to the new relationship scope. Pictures, audio and
    /// video reuse equal media parts of this presentation. External
    /// relationships such as hyperlinks keep their targets.
    ///
    /// Comment authors join this presentation's author list. A source author
    /// equal to one of this presentation's authors in name, initials, user id
    /// and provider id is replaced by it. Comment, reply and author ids that
    /// this presentation already uses are replaced by fresh sequential ids,
    /// and comment anchors follow the copied slide and its renumbered shapes.
    ///
    /// A jump to a slide, from the slide or its notes, is kept when its target
    /// is copied by this call or is listed in `existing_slide_map`. Any other jump is turned into
    /// a hyperlink that does nothing, and is reported as a diagnostic. Jumps
    /// that name no relationship, such as a `ppaction://customshow` action,
    /// are carried unchanged and may name a custom show this presentation
    /// lacks.
    ///
    /// Legacy comments are copied into a part of their own: their authors
    /// join this presentation's legacy author list, and each comment takes
    /// its author's next index. Controls and any other internal relationship
    /// type, on the slide, its notes or a copied part, are rejected without
    /// mutation, and the error names the relationship type.
    pub fn import_slides(
        &mut self,
        source: &Presentation,
        indexes: &[usize],
        options: &ImportSlideOptions,
        existing_slide_map: &[(usize, usize)],
    ) -> Result<(Vec<usize>, Vec<String>)> {
        const OPERATION: &str = "import slide";
        let inserted = options.insert_index.unwrap_or(self.slides.len());
        if inserted > self.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index: inserted,
                slide_count: self.slides.len(),
            });
        }
        if let Some(layout_index) = options.layout_index {
            if options.keep_source_formatting {
                return Err(invalid_presentation_mutation(
                    OPERATION,
                    "a destination layout and keep_source_formatting exclude each other".to_owned(),
                ));
            }
            if layout_index >= self.layouts.len() {
                return Err(Error::UnknownLayoutIndex {
                    index: layout_index,
                    layout_count: self.layouts.len(),
                });
            }
        }
        if indexes.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let selected: HashSet<_> = indexes.iter().copied().collect();
        if selected.len() != indexes.len() {
            return Err(invalid_presentation_mutation(
                OPERATION,
                "duplicate selected source slide".into(),
            ));
        }
        let mut source_package = source.staged_package(true)?;
        let mut jump_targets = HashMap::new();
        for &(source_index, destination_index) in existing_slide_map {
            let source_record =
                source
                    .slides
                    .get(source_index)
                    .ok_or(Error::UnknownSlideIndex {
                        index: source_index,
                        slide_count: source.slides.len(),
                    })?;
            let destination_record =
                self.slides
                    .get(destination_index)
                    .ok_or(Error::UnknownSlideIndex {
                        index: destination_index,
                        slide_count: self.slides.len(),
                    })?;
            if selected.contains(&source_index)
                || jump_targets.contains_key(&source_record.part_name)
            {
                return Err(invalid_presentation_mutation(
                    OPERATION,
                    "duplicate or selected existing slide mapping".into(),
                ));
            }
            jump_targets.insert(
                source_record.part_name.clone(),
                destination_record.part_name.clone(),
            );
        }
        let mut slide_namer = MediaNamer::scan(
            "/ppt/slides",
            "slide",
            self.package.parts.keys().map(String::as_str),
        );
        let mut copies = Vec::with_capacity(indexes.len());
        let mut visited_diagrams = HashSet::new();
        let mut visited_parts = HashSet::new();
        let mut visited_formatting = HashSet::new();
        let mut inherited_masters = HashSet::new();
        let mut copied_targets = HashMap::new();
        for &index in indexes {
            let record = source.slides.get(index).ok_or(Error::UnknownSlideIndex {
                index,
                slide_count: source.slides.len(),
            })?;
            let layout = if options.keep_source_formatting {
                let layout = source.slide_layout_index(index).ok_or_else(|| {
                    invalid_presentation_mutation(
                        OPERATION,
                        "the source slide's layout is not reached by its masters".to_owned(),
                    )
                })?;
                let part = source.layouts[layout].part_name.clone();
                let master_relationship = source_package
                    .get_part_rels(&part)
                    .and_then(|relationships| relationships.get_by_type(rel_types::SLIDE_MASTER))
                    .ok_or_else(|| footer_malformed(&part, "source layout has no master"))?;
                let master = OpcPackage::resolve_rel_target(&part, &master_relationship.target);
                if inherited_masters.insert(master.clone()) {
                    stage_source_master_defaults(source, &mut source_package, &master)?;
                }
                preflight_source_formatting(&source_package, &part, true, &mut visited_formatting)?;
                ImportLayout::Source(part)
            } else {
                ImportLayout::Destination(match options.layout_index {
                    Some(layout_index) => layout_index,
                    None => self.import_layout_index(source, index)?,
                })
            };
            preflight_slide_import(
                &source_package,
                record,
                &mut visited_diagrams,
                &mut visited_parts,
            )?;
            let slide_part = slide_namer.next_part_name("xml");
            copied_targets
                .entry(record.part_name.clone())
                .or_insert_with(|| slide_part.clone());
            copies.push((index, layout, slide_part));
        }
        if copies.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        jump_targets.extend(copied_targets);

        let mut staged = self.clone();
        staged.package = staged.staged_package(true)?;
        let original_main_ids = staged
            .package
            .get_part_rels(&staged.presentation_part)
            .map(|relationships| {
                relationships
                    .items
                    .iter()
                    .map(|item| item.id.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        if copies
            .iter()
            .any(|(index, _, _)| source.slides[*index].notes.is_some())
        {
            if staged.notes_master.is_none() {
                staged.add_default_notes_master_in_place()?;
            }
            staged.ensure_notes_master_registration_in_place()?;
        }
        if options.keep_source_formatting {
            for &index in indexes {
                if let Some(notes) = &source.slides[index].notes {
                    stage_source_notes_appearance(
                        source,
                        &mut source_package,
                        &notes.part_name,
                        &staged,
                    )?;
                }
            }
        }
        let mut diagnostics = Vec::new();
        let mut adopted = HashMap::new();
        let mut part_copies = HashMap::new();
        let mut diagram_copies = HashMap::new();
        let mut font_parts = visited_formatting.clone();
        font_parts.extend(visited_parts.iter().cloned());
        font_parts.extend(visited_diagrams.iter().cloned());
        for &index in indexes {
            font_parts.insert(source.slides[index].part_name.clone());
            if let Some(notes) = &source.slides[index].notes {
                font_parts.insert(notes.part_name.clone());
            }
            if let Some(comments) = &source.slides[index].comments {
                font_parts.insert(comments.part_name.clone());
            }
        }
        let table_styles = staged.import_table_style_dependencies(
            source,
            &mut source_package,
            &font_parts,
            &mut part_copies,
        )?;
        let font_names = imported_typefaces(&source_package, &font_parts)?;
        staged.import_embedded_font_dependencies(
            source,
            &source_package,
            &font_names,
            &mut part_copies,
        )?;
        staged.import_classic_comment_graphs(
            &source.slides,
            &source.presentation_part,
            &source_package,
            indexes,
            &mut part_copies,
        )?;
        for (offset, (index, layout, slide_part)) in copies.iter().enumerate() {
            let layout_part = match layout {
                ImportLayout::Destination(layout_index) => {
                    staged.layouts[*layout_index].part_name.clone()
                }
                ImportLayout::Source(layout) => {
                    staged.adopt_source_layout(&source_package, layout, &mut adopted)?
                }
            };
            let mut record = SlideRecord::clone(&source.slides[*index]);
            if !table_styles.is_empty() {
                record.slide =
                    CT_Slide::from_xml(required_part(&source_package, &record.part_name)?)
                        .map_err(|error| footer_malformed(&record.part_name, error.to_string()))?;
                if let Some(notes) = &mut record.notes {
                    notes.notes =
                        CT_NotesSlide::from_xml(required_part(&source_package, &notes.part_name)?)
                            .map_err(|error| {
                                footer_malformed(&notes.part_name, error.to_string())
                            })?;
                }
            }
            staged.copy_slide_from_in_place(
                &source_package,
                record,
                inserted + offset,
                OPERATION,
                Some(&layout_part),
                Some(SlideImportContext {
                    source,
                    source_index: *index,
                    slide_part,
                    jump_targets: &jump_targets,
                    diagnostics: &mut diagnostics,
                    part_copies: &mut part_copies,
                    diagram_copies: &mut diagram_copies,
                }),
            )?;
        }
        if options.keep_sections && !source.sections().is_empty() {
            let source_indexes = copies
                .iter()
                .map(|(index, _, _)| *index)
                .collect::<Vec<_>>();
            staged.keep_imported_sections(source, &source_indexes, inserted)?;
        }
        let mut main_copies = part_copies;
        main_copies.extend(
            adopted
                .iter()
                .map(|(source, destination)| (source.clone(), destination.clone())),
        );
        for (index, layout, slide_part) in &copies {
            main_copies.insert(source.slides[*index].part_name.clone(), slide_part.clone());
            if let ImportLayout::Source(layout) = layout {
                let source_master =
                    related_internal_part(&source_package, layout, rel_types::SLIDE_MASTER)?;
                let destination_layout = &adopted[layout];
                let destination_master = related_internal_part(
                    &staged.package,
                    destination_layout,
                    rel_types::SLIDE_MASTER,
                )?;
                if let (Some(source), Some(destination)) = (source_master, destination_master) {
                    main_copies.insert(source, destination);
                }
            }
        }
        if !table_styles.is_empty()
            && let (Some(source), Some(destination)) = (
                import_table_style_part(&source_package, &source.presentation_part)?,
                import_table_style_part(&staged.package, &staged.presentation_part)?,
            )
        {
            main_copies.insert(source, destination);
        }
        if indexes
            .iter()
            .any(|index| source.slides[*index].comments.is_some())
            && let (Some(source), Some(destination)) =
                (&source.comment_authors_part, &staged.comment_authors_part)
        {
            main_copies.insert(source.clone(), destination.clone());
        }
        staged.reconcile_imported_main_relationships(
            source,
            &source_package,
            &main_copies,
            &original_main_ids,
        )?;
        let inserted_indexes = (inserted..inserted + copies.len()).collect();
        staged.refresh_document_metadata(MetadataEdit::Addition)?;
        self.commit_candidate(staged)?;
        Ok((inserted_indexes, diagnostics))
    }

    /// Gives the copies of `source_indexes`, inserted from `inserted`, the
    /// sections of their source slides, as `ImportSlideOptions::keep_sections`
    /// describes.
    fn keep_imported_sections(
        &mut self,
        source: &Presentation,
        source_indexes: &[usize],
        inserted: usize,
    ) -> Result<()> {
        const OPERATION: &str = "import slide";
        let invalid =
            |error: OxmlError| invalid_presentation_mutation(OPERATION, error.to_string());
        let mut used = self
            .sections()
            .iter()
            .filter_map(|section| section.id.as_ref())
            .map(|id| id.to_ascii_uppercase())
            .collect::<HashSet<_>>();
        let mut section_id = |preferred: Option<&str>| {
            if let Some(preferred) = preferred
                && used.insert(preferred.to_ascii_uppercase())
            {
                return preferred.to_owned();
            }
            let id = (1_u64..)
                .map(|index| format!("{{00000000-0000-4000-8000-{index:012X}}}"))
                .find(|id| !used.contains(id))
                .expect("a finite set of used ids leaves a sequential id free");
            used.insert(id.clone());
            id
        };

        let mut runs: Vec<(Option<usize>, Vec<u32>)> = Vec::new();
        for (offset, &index) in source_indexes.iter().enumerate() {
            let source_id = source.slides[index].id;
            let section = source
                .sections()
                .iter()
                .position(|section| section.slide_ids.contains(&source_id));
            let copy_id = self.slides[inserted + offset].id;
            match runs.last_mut() {
                Some((last, ids)) if *last == section => ids.push(copy_id),
                _ => runs.push((section, vec![copy_id])),
            }
        }
        let id_map = source_indexes
            .iter()
            .enumerate()
            .map(|(offset, &index)| (source.slides[index].id, self.slides[inserted + offset].id))
            .collect::<HashMap<_, _>>();
        let mut imported = Vec::with_capacity(runs.len());
        for (section, ids) in runs {
            let section = section.map(|section| &source.sections()[section]);
            let id = section_id(section.and_then(|section| section.id.as_deref()));
            let name = section
                .and_then(|section| section.name.as_deref())
                .unwrap_or("Untitled Section");
            let mut copied = match section {
                Some(section) => {
                    let mut copied = section.clone();
                    copied.remap_slide_ids(&id_map).map_err(invalid)?;
                    copied.id = Some(id);
                    copied
                }
                None => Section::new(id, name, Vec::new()).map_err(invalid)?,
            };
            copied.slide_ids = ids;
            imported.push(copied);
        }

        let count = source_indexes.len();
        let existing = self.sections().to_vec();
        let mut sections = Vec::new();
        let mut seen = HashSet::new();
        for range in [0..inserted, inserted + count..self.slides.len()] {
            let mut runs: Vec<(Option<usize>, Vec<u32>)> = Vec::new();
            for record in &self.slides[range] {
                let owner = existing
                    .iter()
                    .position(|section| section.slide_ids.contains(&record.id));
                match runs.last_mut() {
                    Some((last, ids)) if *last == owner => ids.push(record.id),
                    _ => runs.push((owner, vec![record.id])),
                }
            }
            for (owner, ids) in runs {
                let mut section = match owner {
                    Some(owner) => existing[owner].clone(),
                    None => Section::new(section_id(None), "Default Section", Vec::new())
                        .map_err(invalid)?,
                };
                if let Some(owner) = owner
                    && !seen.insert(owner)
                {
                    section.id = Some(section_id(None));
                }
                section.slide_ids = ids;
                sections.push(section);
            }
            if !imported.is_empty() {
                sections.append(&mut imported);
            }
        }
        self.presentation.set_sections(sections).map_err(invalid)
    }

    /// Returns the first layout named like the layout of `source` slide `index`.
    fn import_layout_index(&self, source: &Presentation, index: usize) -> Result<usize> {
        const OPERATION: &str = "import slide";
        let name = source
            .slide_layout_index(index)
            .and_then(|layout| source.layout_name(layout))
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    OPERATION,
                    "the source slide's layout has no name, pass the destination layout".to_owned(),
                )
            })?;
        (0..self.layouts.len())
            .find(|&layout| self.layout_name(layout) == Some(name))
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    OPERATION,
                    format!(
                        "this presentation has no layout named {name:?}, pass the destination layout"
                    ),
                )
            })
    }
}

impl Presentation {
    /// Copies one slide record, with its notes and modern comments, into this
    /// presentation.
    ///
    /// `import` is `None` for a copy whose relationship targets already live
    /// in this package or are bounded by the caller's preflight. Its modern
    /// comments are then taken as this presentation's own, with its authors:
    /// a duplicate is the only such copy that may carry any. An import
    /// also copies charts and embedded objects, and maps or drops slide
    /// jumps. Copied comments and replies take fresh ids wherever this
    /// presentation already uses theirs.
    pub(super) fn copy_slide_from_in_place(
        &mut self,
        source_package: &OpcPackage,
        source: SlideRecord,
        inserted: usize,
        operation: &'static str,
        layout_target: Option<&str>,
        mut import: Option<SlideImportContext<'_>>,
    ) -> Result<usize> {
        let slide_part = match &import {
            Some(import) => import.slide_part.to_owned(),
            None => MediaNamer::scan(
                "/ppt/slides",
                "slide",
                self.package.parts.keys().map(String::as_str),
            )
            .next_part_name("xml"),
        };
        let id = self
            .presentation
            .slide_ids
            .iter()
            .map(|slide| slide.id)
            .max()
            .unwrap_or(255)
            .max(255)
            .checked_add(1)
            .ok_or(Error::SlideIdExhausted)?;
        let copy_part_graphs = import.is_some();
        let slide_map = import.as_ref().map(|import| import.jump_targets);

        let notes_part = source.notes.as_ref().map(|_| {
            MediaNamer::scan(
                "/ppt/notesSlides",
                "notesSlide",
                self.package.parts.keys().map(String::as_str),
            )
            .next_part_name("xml")
        });
        let notes = if let (Some(source_notes), Some(notes_part)) = (&source.notes, &notes_part) {
            let xml = if import.is_some() {
                required_part(source_package, &source_notes.part_name)?.to_vec()
            } else {
                source_notes
                    .notes
                    .to_xml()
                    .map_err(|error| Error::MalformedPart {
                        part_name: source_notes.part_name.clone(),
                        message: error.to_string(),
                    })?
            };
            let source_relationships = source_package
                .get_part_rels(&source_notes.part_name)
                .cloned()
                .unwrap_or_default();
            let (xml, source_relationships) = match import.as_mut() {
                Some(import) => import.release_unmapped_jumps(
                    &source_notes.part_name,
                    Some(&source.part_name),
                    xml,
                    source_relationships,
                )?,
                None => (xml, source_relationships),
            };
            let (mut relationships, mut relationship_map) = duplicate_relationship_scope(
                source_package,
                &mut self.package,
                &mut self.media_store,
                &source_notes.part_name,
                notes_part,
                &source_relationships,
                RelationshipScopeTargets {
                    slide: Some(&slide_part),
                    slide_map,
                    notes: None,
                    comments: None,
                    layout: None,
                    notes_master: self.notes_master_part.as_deref(),
                    copy_part_graphs,
                    graph_copies: import.as_mut().map(|context| ImportGraphCopies {
                        diagrams: &mut *context.diagram_copies,
                        parts: &mut *context.part_copies,
                    }),
                },
            )?;
            // The relationships back to the copied slide, and any external
            // one, collapse into one internal back relationship.
            let back_target = relative_part_target(notes_part, &slide_part);
            let back_ids = relationships
                .items
                .iter()
                .filter(|relationship| {
                    relationship.rel_type == rel_types::SLIDE
                        && (relationship.target == back_target
                            || relationship_is_external(relationship))
                })
                .map(|relationship| relationship.id.clone())
                .collect::<HashSet<_>>();
            let kept = relationships
                .items
                .iter()
                .filter(|relationship| !back_ids.contains(&relationship.id))
                .cloned()
                .collect();
            relationships = relationships.rebind_for_import(kept, &HashMap::new())?;
            let back_relationship_id = relationships.add(rel_types::SLIDE, &back_target);
            relationships =
                relationships.rebind_for_import(relationships.items.clone(), &HashMap::new())?;
            for mapped in relationship_map.values_mut() {
                if back_ids.contains(mapped) {
                    mapped.clone_from(&back_relationship_id);
                }
            }
            // One exact pass over the complete map: a second pass would
            // re-map ids the first one had just produced.
            let xml = rewrite_exact_rel_ids(&xml, &relationship_map).map_err(|error| {
                Error::MalformedPart {
                    part_name: source_notes.part_name.clone(),
                    message: error.to_string(),
                }
            })?;
            let xml = rewrite_shape_ids(&xml).map_err(|error| Error::MalformedPart {
                part_name: source_notes.part_name.clone(),
                message: error.to_string(),
            })?;
            let notes = CT_NotesSlide::from_xml(&xml).map_err(|error| Error::MalformedPart {
                part_name: notes_part.clone(),
                message: error.to_string(),
            })?;
            self.package.set_part(notes_part, xml);
            self.package
                .part_rels
                .insert(notes_part.clone(), relationships);
            self.package
                .content_types
                .add_override(notes_part, content_types::NOTES_SLIDE);
            Some(NotesRecord {
                part_name: notes_part.clone(),
                notes,
            })
        } else {
            None
        };

        let comments_part = source.comments.as_ref().map(|_| {
            MediaNamer::scan(
                "/ppt/comments",
                "comment",
                self.package.parts.keys().map(String::as_str),
            )
            .next_part_name("xml")
        });
        let slide_xml = if import.is_some() {
            required_part(source_package, &source.part_name)?.to_vec()
        } else {
            source
                .slide
                .to_xml()
                .map_err(|error| Error::MalformedPart {
                    part_name: source.part_name.clone(),
                    message: error.to_string(),
                })?
        };
        let source_relationships = source_package
            .get_part_rels(&source.part_name)
            .cloned()
            .unwrap_or_default();
        let (slide_xml, source_relationships) = match import.as_mut() {
            Some(import) => import.release_unmapped_jumps(
                &source.part_name,
                None,
                slide_xml,
                source_relationships,
            )?,
            None => (slide_xml, source_relationships),
        };
        let (slide_relationships, relationship_map) = duplicate_relationship_scope(
            source_package,
            &mut self.package,
            &mut self.media_store,
            &source.part_name,
            &slide_part,
            &source_relationships,
            RelationshipScopeTargets {
                slide: None,
                slide_map,
                notes: notes_part.as_deref(),
                comments: comments_part.as_deref(),
                layout: layout_target,
                notes_master: None,
                copy_part_graphs,
                graph_copies: import.as_mut().map(|context| ImportGraphCopies {
                    diagrams: &mut *context.diagram_copies,
                    parts: &mut *context.part_copies,
                }),
            },
        )?;
        let slide_xml = rewrite_rel_ids(&slide_xml, &relationship_map).map_err(|error| {
            Error::MalformedPart {
                part_name: source.part_name.clone(),
                message: error.to_string(),
            }
        })?;
        let (slide_xml, shape_ids) =
            rewrite_shape_ids_mapped(&slide_xml).map_err(|error| Error::MalformedPart {
                part_name: source.part_name.clone(),
                message: error.to_string(),
            })?;
        let slide = CT_Slide::from_xml(&slide_xml).map_err(|error| Error::MalformedPart {
            part_name: slide_part.clone(),
            message: error.to_string(),
        })?;
        let comments = match (&source.comments, comments_part) {
            (Some(source_comments), Some(part_name)) => {
                let mut comments = source_comments.comments.clone();
                // Without an import the copy is a duplicate inside this
                // presentation, so its authors are already this one's.
                let source_authors = match &import {
                    Some(import) => import.source.comment_authors.authors.clone(),
                    None => self.comment_authors.authors.clone(),
                };
                let (ids, author_ids) =
                    self.merge_imported_comment_identities(operation, &source_authors, &comments)?;
                let malformed = |error: OxmlError| Error::MalformedPart {
                    part_name: source_comments.part_name.clone(),
                    message: error.to_string(),
                };
                comments
                    .remap_for_copied_slide(&ids, &author_ids, id, &shape_ids)
                    .map_err(malformed)?;
                // The part is written now so that the next copy of this call
                // names its comment part after it.
                self.package
                    .set_part(&part_name, comments.to_xml().map_err(malformed)?);
                self.package
                    .content_types
                    .add_override(&part_name, content_types::POWERPOINT_COMMENTS);
                if let Some(relationships) =
                    source_package.get_part_rels(&source_comments.part_name)
                {
                    self.package
                        .set_part_rels(&part_name, relationships.clone());
                }
                Some(CommentRecord {
                    part_name,
                    comments,
                    dirty: true,
                })
            }
            _ => None,
        };

        let mut presentation_relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let presentation_relationship_id = presentation_relationships.add(
            rel_types::SLIDE,
            &relative_part_target(&self.presentation_part, &slide_part),
        );
        let slide_id = CT_SlideId::new(id, presentation_relationship_id).map_err(|error| {
            Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            }
        })?;
        self.package.set_part(&slide_part, slide_xml);
        self.package.set_part_rels(&slide_part, slide_relationships);
        self.package
            .set_part_rels(&self.presentation_part, presentation_relationships);
        self.package
            .content_types
            .add_override(&slide_part, content_types::SLIDE);

        self.presentation.slide_ids.insert(inserted, slide_id);
        self.presentation.place_slide_in_sections(id);
        self.slides.insert(
            inserted,
            Arc::new(SlideRecord {
                id,
                part_name: slide_part,
                slide,
                notes,
                comments,
            }),
        );
        Ok(inserted)
    }
}

struct ImportGraphCopies<'a> {
    diagrams: &'a mut HashMap<String, String>,
    parts: &'a mut HashMap<String, String>,
}

struct RelationshipScopeTargets<'a> {
    slide: Option<&'a str>,
    /// Source slide parts and the destination slides their jumps land on,
    /// consulted before `slide`.
    slide_map: Option<&'a HashMap<String, String>>,
    notes: Option<&'a str>,
    comments: Option<&'a str>,
    layout: Option<&'a str>,
    notes_master: Option<&'a str>,
    /// Copies chart, embedded-object and legacy-drawing graphs instead of
    /// sharing their parts.
    copy_part_graphs: bool,
    graph_copies: Option<ImportGraphCopies<'a>>,
}

/// The layout one imported slide relates to.
enum ImportLayout {
    /// A layout of the destination, by index.
    Destination(usize),
    /// The source slide's own layout part, adopted with its master and theme.
    Source(String),
}

/// Neutralizes an imported jump target without releasing its click payload.
fn release_imported_jump_actions(
    xml: &[u8],
    ids: &HashSet<String>,
) -> std::result::Result<Vec<u8>, OxmlError> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    let bom = usize::from(xml.starts_with(&[0xef, 0xbb, 0xbf])) * 3;
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        let event_end = reader.buffer_position() as usize + bom;
        match &event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let content: &[u8] = start.as_ref();
                let (namespace, local) = reader.resolver().resolve_element(start.name());
                if import_namespace_uri(namespace, reader.decoder())?
                    == oxoffice_drawing::namespace::A_NS
                    && matches!(
                        local.as_ref(),
                        b"hlinkClick" | b"hlinkHover" | b"hlinkMouseOver"
                    )
                {
                    let attributes = start
                        .attributes()
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    let mut target = None;
                    for attribute in &attributes {
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if local.as_ref() == b"id"
                            && import_namespace_uri(namespace, reader.decoder())?
                                == oxoffice_pptx_oxml::namespace::R_NS
                        {
                            if target.is_some() {
                                return Err(OxmlError::InvalidValue(
                                    "ambiguous imported hyperlink target".into(),
                                ));
                            }
                            target = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )?
                                    .into_owned(),
                            );
                        }
                    }
                    if target.as_ref().is_some_and(|id| ids.contains(id)) {
                        let closing = if matches!(event, XmlEvent::Empty(_)) {
                            2
                        } else {
                            1
                        };
                        let content_start = event_end
                            .checked_sub(closing + content.len())
                            .ok_or_else(|| {
                                OxmlError::InvalidValue("invalid hyperlink source span".into())
                            })?;
                        let mut has_action = false;
                        for attribute in &attributes {
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            let owned_id = local.as_ref() == b"id"
                                && import_namespace_uri(namespace, reader.decoder())?
                                    == oxoffice_pptx_oxml::namespace::R_NS;
                            let action = attribute.key.as_ref() == b"action";
                            has_action |= action;
                            if !owned_id && !action {
                                continue;
                            }
                            let offset = (attribute.value.as_ref().as_ptr() as usize)
                                .checked_sub(content.as_ptr() as usize)
                                .ok_or_else(|| {
                                    OxmlError::InvalidValue(
                                        "invalid hyperlink attribute span".into(),
                                    )
                                })?;
                            let from = content_start + offset;
                            let to = from + attribute.value.as_ref().len();
                            if xml.get(from..to) != Some(attribute.value.as_ref()) {
                                return Err(OxmlError::InvalidValue(
                                    "hyperlink attribute source mismatch".into(),
                                ));
                            }
                            edits.push((
                                from..to,
                                if action {
                                    b"ppaction://noaction".to_vec()
                                } else {
                                    Vec::new()
                                },
                            ));
                        }
                        if !has_action {
                            let at = content_start + content.len();
                            edits.push((at..at, b" action=\"ppaction://noaction\"".to_vec()));
                        }
                    }
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = xml.to_vec();
    for (range, bytes) in edits.into_iter().rev() {
        result.splice(range, bytes);
    }
    Ok(result)
}

/// What a slide import adds to one slide copy.
pub(super) struct SlideImportContext<'a> {
    source: &'a Presentation,
    source_index: usize,
    slide_part: &'a str,
    jump_targets: &'a HashMap<String, String>,
    diagnostics: &'a mut Vec<String>,
    part_copies: &'a mut HashMap<String, String>,
    diagram_copies: &'a mut HashMap<String, String>,
}

impl SlideImportContext<'_> {
    /// Turns every jump of `part` to a slide outside `jump_targets` into a
    /// hyperlink that does nothing, and drops the relationships only those
    /// hyperlinks named. A jump to `back_target`, the slide of a notes part,
    /// is kept.
    fn release_unmapped_jumps(
        &mut self,
        part: &str,
        back_target: Option<&str>,
        xml: Vec<u8>,
        relationships: Relationships,
    ) -> Result<(Vec<u8>, Relationships)> {
        let mut released = HashSet::new();
        for relationship in &relationships.items {
            if relationship.rel_type != rel_types::SLIDE || relationship_is_external(relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(part, &relationship.target);
            if Some(target.as_str()) == back_target || self.jump_targets.contains_key(&target) {
                continue;
            }
            released.insert(relationship.id.clone());
            self.diagnostics.push(format!("source slide {} owner {part} relationship {} target {target}: dropped unmapped slide jump", self.source_index, relationship.id));
        }
        if released.is_empty() {
            return Ok((xml, relationships));
        }
        let malformed = |error: OxmlError| Error::MalformedPart {
            part_name: part.to_owned(),
            message: error.to_string(),
        };
        let released_xml = release_imported_jump_actions(&xml, &released).map_err(malformed)?;
        let before: HashSet<String> = relationship_ids(&xml)
            .map_err(malformed)?
            .into_iter()
            .collect();
        let after: HashSet<String> = relationship_ids(&released_xml)
            .map_err(malformed)?
            .into_iter()
            .collect();
        if let Some(id) = released.iter().find(|id| after.contains(*id)) {
            return Err(invalid_presentation_mutation(
                "import slide",
                format!(
                    "{part} names the slide relationship {id} outside a hyperlink, which is not carried"
                ),
            ));
        }
        let kept = relationships
            .items
            .iter()
            .filter(|relationship| {
                !((released.contains(&relationship.id) || before.contains(&relationship.id))
                    && !after.contains(&relationship.id))
            })
            .cloned()
            .collect();
        let relationships = relationships.rebind_for_import(kept, &HashMap::new())?;
        Ok((released_xml, relationships))
    }
}

fn preflight_cross_presentation_transfer(
    source_package: &OpcPackage,
    source: &SlideRecord,
) -> Result<()> {
    if source
        .slide
        .contains_placeholder()
        .map_err(|error| Error::MalformedPart {
            part_name: source.part_name.clone(),
            message: error.to_string(),
        })?
    {
        return Err(invalid_presentation_mutation(
            "transfer SmartArt slide",
            "placeholders require cross-presentation index reconciliation and are outside the bounded SmartArt transfer".to_owned(),
        ));
    }
    if source.comments.is_some() {
        return Err(invalid_presentation_mutation(
            "transfer SmartArt slide",
            "modern comments are outside the bounded cross-presentation transfer graph".to_owned(),
        ));
    }
    if source.notes.is_some() {
        return Err(invalid_presentation_mutation(
            "transfer SmartArt slide",
            "notes are outside the bounded cross-presentation transfer graph".to_owned(),
        ));
    }
    let relationships = source_package
        .get_part_rels(&source.part_name)
        .cloned()
        .unwrap_or_default();
    let external_layout_count = relationships
        .items
        .iter()
        .filter(|relationship| {
            relationship.rel_type == rel_types::SLIDE_LAYOUT
                && relationship_is_external(relationship)
        })
        .count();
    if external_layout_count != 0 {
        return Err(invalid_presentation_mutation(
            "transfer SmartArt slide",
            "an external slide-layout relationship cannot be replaced by the selected destination layout".to_owned(),
        ));
    }
    let internal_layout_count = relationships
        .items
        .iter()
        .filter(|relationship| {
            relationship.rel_type == rel_types::SLIDE_LAYOUT
                && !relationship_is_external(relationship)
        })
        .count();
    if internal_layout_count != 1 {
        return Err(invalid_presentation_mutation(
            "transfer SmartArt slide",
            format!(
                "source slide must own exactly one internal slide-layout relationship, found {internal_layout_count}"
            ),
        ));
    }
    let mut smartart_frames = Vec::new();
    collect_smartart_frames(
        &source.slide.common_slide_data.shape_tree.children,
        &mut smartart_frames,
    );
    for (frame, _) in smartart_frames {
        if let GraphicDataPayload::SmartArt(relationship_ids) = frame.graphic_data.payload() {
            validate_smartart_relationship_roles(
                source_package,
                &source.part_name,
                relationship_ids,
            )?;
        }
    }
    let mut visited_diagram_parts = HashSet::new();
    for relationship in &relationships.items {
        if relationship_is_external(relationship) {
            continue;
        }
        if relationship.rel_type != rel_types::SLIDE_LAYOUT
            && relationship.rel_type != rel_types::IMAGE
            && !is_diagram_relationship(&relationship.rel_type)
        {
            return Err(invalid_presentation_mutation(
                "transfer SmartArt slide",
                format!(
                    "internal relationship type {} is outside the bounded cross-presentation transfer graph",
                    relationship.rel_type
                ),
            ));
        }
        let resolved = OpcPackage::resolve_rel_target(&source.part_name, &relationship.target);
        required_part(source_package, &resolved)?;
        if relationship.rel_type == rel_types::IMAGE {
            preflight_internal_transfer_image(
                source_package,
                &resolved,
                "transfer SmartArt slide",
            )?;
        } else if is_diagram_relationship(&relationship.rel_type) {
            preflight_diagram_part_graph(
                source_package,
                &resolved,
                &mut visited_diagram_parts,
                "transfer SmartArt slide",
                true,
            )?;
        }
    }
    Ok(())
}

/// Validates the finite formatting topology before semantic comparison.
/// Layouts reach a master, masters reach a theme, and copied theme/tag/VML
/// parts may reach only leaf images. No recursive comparison follows arbitrary edges.
fn preflight_source_formatting(
    package: &OpcPackage,
    part: &str,
    layout: bool,
    visited: &mut HashSet<String>,
) -> Result<()> {
    let mut pending = vec![(part.to_owned(), layout)];
    while let Some((part, layout)) = pending.pop() {
        let expected = if layout {
            content_types::SLIDE_LAYOUT
        } else {
            content_types::SLIDE_MASTER
        };
        if package.content_types.content_type_for(&part) != Some(expected) {
            return Err(Error::MalformedPart {
                part_name: part,
                message: format!("formatting owner must have content type {expected}"),
            });
        }
        required_part(package, &part)?;
        if !visited.insert(part.clone()) {
            continue;
        }
        let Some(relationships) = package.get_part_rels(&part) else {
            continue;
        };
        for relationship in &relationships.items {
            if relationship_is_external(relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(&part, &relationship.target);
            match relationship.rel_type.as_str() {
                rel_types::SLIDE_MASTER if layout => pending.push((target, false)),
                rel_types::SLIDE_LAYOUT if !layout => {
                    required_part(package, &target)?;
                }
                rel_types::IMAGE => {
                    preflight_internal_transfer_image(package, &target, "import slide")?
                }
                kind @ (TAGS_RELATIONSHIP | VML_DRAWING_RELATIONSHIP) => {
                    preflight_carried_part_graph(package, &target, kind, visited)?
                }
                rel_types::THEME if !layout => {
                    if package.content_types.content_type_for(&target) != Some(content_types::THEME)
                    {
                        return Err(Error::MalformedPart {
                            part_name: target,
                            message: "master theme has an incompatible content type".into(),
                        });
                    }
                    preflight_carried_part_graph(package, &target, rel_types::THEME, visited)?;
                }
                kind => {
                    return Err(invalid_presentation_mutation(
                        "import slide",
                        format!(
                            "{part} has an internal relationship of type {kind}, which is not carried"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn preflight_slide_import(
    source_package: &OpcPackage,
    source: &SlideRecord,
    visited_diagram_parts: &mut HashSet<String>,
    visited_parts: &mut HashSet<String>,
) -> Result<()> {
    let mut smartart_frames = Vec::new();
    collect_smartart_frames(
        &source.slide.common_slide_data.shape_tree.children,
        &mut smartart_frames,
    );
    for (frame, _) in smartart_frames {
        if let GraphicDataPayload::SmartArt(relationship_ids) = frame.graphic_data.payload() {
            validate_smartart_relationship_roles(
                source_package,
                &source.part_name,
                relationship_ids,
            )?;
        }
    }
    let comments_part = source
        .comments
        .as_ref()
        .map(|comments| comments.part_name.as_str());
    preflight_import_scope(
        source_package,
        &source.part_name,
        None,
        comments_part,
        visited_diagram_parts,
        visited_parts,
    )?;
    if let Some(notes) = &source.notes {
        preflight_import_scope(
            source_package,
            &notes.part_name,
            Some(&source.part_name),
            None,
            visited_diagram_parts,
            visited_parts,
        )?;
    }
    Ok(())
}

/// Accepts the internal relationships a slide import rebinds or copies.
///
/// A slide scope, whose `notes_of` is `None`, may relate to its layout, its
/// notes, its modern comments part `comments_part`, images, audio, video,
/// SmartArt graphs, charts, embedded objects and legacy drawings. A notes
/// scope may relate to its notes master, images, audio and video. Both may
/// relate to slides, the notes back to the slide named by `notes_of`, and
/// every other slide relationship is a jump that the copy maps or releases.
/// External relationships are always carried.
fn preflight_import_scope(
    source_package: &OpcPackage,
    part: &str,
    notes_of: Option<&str>,
    comments_part: Option<&str>,
    visited_diagram_parts: &mut HashSet<String>,
    visited_parts: &mut HashSet<String>,
) -> Result<()> {
    let slide = notes_of.is_none();
    const OPERATION: &str = "import slide";
    let relationships = source_package
        .get_part_rels(part)
        .cloned()
        .unwrap_or_default();
    for relationship in &relationships.items {
        if relationship_is_external(relationship) {
            continue;
        }
        let resolved = OpcPackage::resolve_rel_target(part, &relationship.target);
        let rel_type = relationship.rel_type.as_str();
        match rel_type {
            rel_types::SLIDE_LAYOUT | rel_types::NOTES_SLIDE if slide => {}
            rel_types::NOTES_MASTER if !slide => {}
            rel_types::SLIDE => {}
            rel_types::COMMENTS if slide => {
                required_part(source_package, &resolved)?;
                if let Some(owned) = source_package.get_part_rels(&resolved).and_then(|owned| {
                    owned
                        .items
                        .iter()
                        .find(|owned| !relationship_is_external(owned))
                }) {
                    return Err(invalid_presentation_mutation(
                        OPERATION,
                        format!(
                            "{resolved} has an internal relationship of type {}, which is not carried",
                            owned.rel_type
                        ),
                    ));
                }
            }
            rel_types::POWERPOINT_COMMENTS if slide && comments_part == Some(&resolved) => {
                // The copy is written from the comment model, so the part
                // may own external relationships only.
                if let Some(owned) = source_package.get_part_rels(&resolved).and_then(|owned| {
                    owned
                        .items
                        .iter()
                        .find(|owned| !relationship_is_external(owned))
                }) {
                    return Err(invalid_presentation_mutation(
                        OPERATION,
                        format!(
                            "{resolved} has an internal relationship of type {}, which is not carried",
                            owned.rel_type
                        ),
                    ));
                }
            }
            rel_types::IMAGE => {
                preflight_internal_transfer_image(source_package, &resolved, OPERATION)?;
            }
            _ if is_audio_video_relationship(rel_type) => {
                required_part(source_package, &resolved)?;
            }
            _ if slide && is_diagram_relationship(rel_type) => {
                preflight_diagram_part_graph(
                    source_package,
                    &resolved,
                    visited_diagram_parts,
                    OPERATION,
                    false,
                )?;
            }
            _ if slide && is_carried_part_graph_relationship(rel_type) => {
                preflight_carried_part_graph(source_package, &resolved, rel_type, visited_parts)?;
            }
            _ => {
                return Err(invalid_presentation_mutation(
                    OPERATION,
                    format!(
                        "{part} has an internal relationship of type {rel_type}, which is not carried"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Returns whether a slide import copies the part graph behind a slide
/// relationship of this type, rather than rebinding or sharing it.
fn is_carried_part_graph_relationship(relationship_type: &str) -> bool {
    matches!(
        relationship_type,
        rel_types::CHART
            | rel_types::COMMENTS
            | CHART_EX_RELATIONSHIP
            | rel_types::OLE_OBJECT
            | rel_types::STRICT_OLE_OBJECT
            | rel_types::PACKAGE
            | VML_DRAWING_RELATIONSHIP
    )
}

/// Accepts one part of a chart, embedded-object, legacy-drawing or comments
/// graph and, through it, the parts it relates to.
///
/// A chart may relate to its embedded workbook or object, its style, colour,
/// user-shape and theme-override parts. Every part may relate to images that
/// own no relationships, and to external targets. Anything else is refused.
pub(super) fn preflight_carried_part_graph(
    source_package: &OpcPackage,
    part: &str,
    relationship_type: &str,
    visited: &mut HashSet<String>,
) -> Result<()> {
    const OPERATION: &str = "import slide";
    if !visited.insert(part.to_owned()) {
        return Ok(());
    }
    required_part(source_package, part)?;
    let chart = matches!(relationship_type, rel_types::CHART | CHART_EX_RELATIONSHIP);
    let relationships = source_package
        .get_part_rels(part)
        .cloned()
        .unwrap_or_default();
    for relationship in &relationships.items {
        if relationship_is_external(relationship) {
            continue;
        }
        let resolved = OpcPackage::resolve_rel_target(part, &relationship.target);
        match relationship.rel_type.as_str() {
            rel_types::IMAGE => {
                preflight_internal_transfer_image(source_package, &resolved, OPERATION)?;
            }
            rel_type @ (rel_types::PACKAGE
            | rel_types::OLE_OBJECT
            | rel_types::STRICT_OLE_OBJECT
            | CHART_STYLE_RELATIONSHIP
            | CHART_COLOR_STYLE_RELATIONSHIP
            | CHART_USER_SHAPES_RELATIONSHIP
            | THEME_OVERRIDE_RELATIONSHIP)
                if chart =>
            {
                preflight_carried_part_graph(source_package, &resolved, rel_type, visited)?;
            }
            rel_type => {
                return Err(invalid_presentation_mutation(
                    OPERATION,
                    format!(
                        "{part} has an internal relationship of type {rel_type}, which is not carried"
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn preflight_internal_transfer_image(
    source_package: &OpcPackage,
    image_part: &str,
    operation: &'static str,
) -> Result<()> {
    required_part(source_package, image_part)?;
    if source_package
        .get_part_rels(image_part)
        .is_some_and(|relationships| !relationships.items.is_empty())
    {
        return Err(invalid_presentation_mutation(
            operation,
            format!(
                "image part {image_part} owns relationships outside the bounded cross-presentation transfer"
            ),
        ));
    }
    Ok(())
}

fn preflight_diagram_part_graph(
    source_package: &OpcPackage,
    source_part: &str,
    visited: &mut HashSet<String>,
    operation: &'static str,
    bounded: bool,
) -> Result<()> {
    let mut pending = vec![source_part.to_owned()];
    while let Some(part) = pending.pop() {
        if visited.contains(&part) {
            continue;
        }
        if bounded && visited.len() >= MAX_SMARTART_TRANSFER_DIAGRAM_PARTS {
            return Err(invalid_presentation_mutation(
                operation,
                format!(
                    "diagram graph exceeds the {MAX_SMARTART_TRANSFER_DIAGRAM_PARTS}-part transfer bound"
                ),
            ));
        }
        visited.insert(part.clone());
        required_part(source_package, &part)?;
        let Some(relationships) = source_package.get_part_rels(&part) else {
            continue;
        };
        for relationship in &relationships.items {
            if relationship_is_external(relationship) {
                continue;
            }
            let resolved = OpcPackage::resolve_rel_target(&part, &relationship.target);
            required_part(source_package, &resolved)?;
            if relationship.rel_type == rel_types::IMAGE {
                preflight_internal_transfer_image(source_package, &resolved, operation)?;
            } else if is_diagram_relationship(&relationship.rel_type) {
                pending.push(resolved);
            } else {
                return Err(invalid_presentation_mutation(
                    operation,
                    format!(
                        "diagram part {part} owns internal relationship type {} outside the bounded cross-presentation transfer",
                        relationship.rel_type
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn duplicate_relationship_scope(
    source_package: &OpcPackage,
    destination_package: &mut OpcPackage,
    media_store: &mut MediaStore,
    source_part: &str,
    destination_part: &str,
    source_relationships: &Relationships,
    targets: RelationshipScopeTargets<'_>,
) -> Result<(Relationships, HashMap<String, String>)> {
    let mut destination = Relationships::new();
    let mut relationship_map = HashMap::new();
    let mut owned_diagrams = HashMap::new();
    let mut owned_parts = HashMap::new();
    let bounded_diagrams = targets.graph_copies.is_none();
    let (diagram_copies, part_copies) = match targets.graph_copies {
        Some(copies) => (copies.diagrams, copies.parts),
        None => (&mut owned_diagrams, &mut owned_parts),
    };
    for relationship in &source_relationships.items {
        let target = if relationship_is_external(relationship) {
            relationship.target.clone()
        } else {
            let resolved = match relationship.rel_type.as_str() {
                rel_types::SLIDE => {
                    let resolved =
                        OpcPackage::resolve_rel_target(source_part, &relationship.target);
                    targets
                        .slide_map
                        .and_then(|slide_map| slide_map.get(&resolved).cloned())
                        .or_else(|| targets.slide.map(str::to_owned))
                        .unwrap_or(resolved)
                }
                rel_types::NOTES_SLIDE => targets.notes.map(str::to_owned).unwrap_or_else(|| {
                    OpcPackage::resolve_rel_target(source_part, &relationship.target)
                }),
                rel_types::POWERPOINT_COMMENTS => {
                    targets.comments.map(str::to_owned).unwrap_or_else(|| {
                        OpcPackage::resolve_rel_target(source_part, &relationship.target)
                    })
                }
                rel_types::SLIDE_LAYOUT => targets.layout.map(str::to_owned).unwrap_or_else(|| {
                    OpcPackage::resolve_rel_target(source_part, &relationship.target)
                }),
                rel_types::NOTES_MASTER => {
                    targets.notes_master.map(str::to_owned).unwrap_or_else(|| {
                        OpcPackage::resolve_rel_target(source_part, &relationship.target)
                    })
                }
                _ => OpcPackage::resolve_rel_target(source_part, &relationship.target),
            };
            let resolved = if relationship.rel_type == rel_types::IMAGE {
                let bytes = required_part(source_package, &resolved)?.to_vec();
                media_store.insert(destination_package, &bytes, &resolved)
            } else if is_audio_video_relationship(&relationship.rel_type) {
                let bytes = required_part(source_package, &resolved)?;
                if destination_package.get_part(&resolved) == Some(bytes) {
                    // A duplicate inside one deck keeps sharing its media
                    // part, wherever that part is stored.
                    resolved
                } else {
                    let format = resolve(bytes, &resolved);
                    let extension = resolved
                        .rsplit_once('.')
                        .map_or_else(|| format.extension(), |(_, extension)| extension);
                    let content_type = source_package
                        .content_types
                        .content_type_for(&resolved)
                        .unwrap_or_else(|| format.content_type());
                    media_store.insert_explicit(destination_package, bytes, extension, content_type)
                }
            } else if is_diagram_relationship(&relationship.rel_type) {
                duplicate_diagram_part_graph(
                    source_package,
                    destination_package,
                    media_store,
                    &resolved,
                    diagram_copies,
                    bounded_diagrams,
                )?
            } else if targets.copy_part_graphs
                && is_carried_part_graph_relationship(&relationship.rel_type)
            {
                duplicate_part_graph(
                    source_package,
                    destination_package,
                    media_store,
                    &resolved,
                    part_copies,
                )?
            } else {
                resolved
            };
            relative_part_target(destination_part, &resolved)
        };
        let new_id = if bounded_diagrams
            && relationship
                .id
                .strip_prefix("rId")
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        {
            destination.add(&relationship.rel_type, &target)
        } else {
            destination.add_with_id(&relationship.id, &relationship.rel_type, &target);
            relationship.id.clone()
        };
        if let Some(inserted) = destination.items.last_mut() {
            inserted.target_mode = relationship.target_mode.clone();
        }
        relationship_map.insert(relationship.id.clone(), new_id);
    }
    for relationship in source_relationships.items.iter().filter(|relationship| {
        relationship.rel_type == rel_types::DIAGRAM_DATA && !relationship_is_external(relationship)
    }) {
        let source_data = OpcPackage::resolve_rel_target(source_part, &relationship.target);
        let Some(copied_data) = diagram_copies.get(&source_data) else {
            continue;
        };
        let mut data = CT_DiagramData::from_xml(required_part(destination_package, copied_data)?)
            .map_err(|error| Error::MalformedPart {
            part_name: copied_data.clone(),
            message: error.to_string(),
        })?;
        data.remap_drawing_relationship(&relationship_map)
            .map_err(|error| Error::MalformedPart {
                part_name: copied_data.clone(),
                message: error.to_string(),
            })?;
        destination_package.set_part(
            copied_data,
            data.to_xml().map_err(|error| Error::MalformedPart {
                part_name: copied_data.clone(),
                message: error.to_string(),
            })?,
        );
    }
    let destination =
        source_relationships.rebind_for_import(destination.items, &relationship_map)?;
    Ok((destination, relationship_map))
}

fn duplicate_diagram_part_graph(
    source_package: &OpcPackage,
    destination_package: &mut OpcPackage,
    media_store: &mut MediaStore,
    source_part: &str,
    copies: &mut HashMap<String, String>,
    bounded: bool,
) -> Result<String> {
    if let Some(existing) = copies.get(source_part) {
        return Ok(existing.clone());
    }
    if bounded && copies.len() >= MAX_SMARTART_TRANSFER_DIAGRAM_PARTS {
        return Err(invalid_presentation_mutation(
            "copy SmartArt diagram graph",
            format!(
                "diagram graph exceeds the {MAX_SMARTART_TRANSFER_DIAGRAM_PARTS}-part copy bound"
            ),
        ));
    }
    let root = fresh_related_part_name(destination_package, source_part);
    copies.insert(source_part.to_owned(), root.clone());
    destination_package.set_part(&root, required_part(source_package, source_part)?.to_vec());
    let mut pending = vec![source_part.to_owned()];
    while let Some(part) = pending.pop() {
        let destination_part = copies[&part].clone();
        let source_xml = required_part(source_package, &part)?;
        if let Some(content_type) = source_package.content_types.content_type_for(&part) {
            destination_package
                .content_types
                .add_override(&destination_part, content_type);
        }
        let source_relationships = source_package
            .get_part_rels(&part)
            .cloned()
            .unwrap_or_default();
        let mut destination_relationships = Relationships::new();
        let mut relationship_map = HashMap::new();
        for relationship in &source_relationships.items {
            let target = if relationship_is_external(relationship) {
                relationship.target.clone()
            } else {
                let resolved = OpcPackage::resolve_rel_target(&part, &relationship.target);
                let copied = if relationship.rel_type == rel_types::IMAGE {
                    let bytes = required_part(source_package, &resolved)?.to_vec();
                    media_store.insert(destination_package, &bytes, &resolved)
                } else if is_diagram_relationship(&relationship.rel_type) {
                    if let Some(existing) = copies.get(&resolved) {
                        existing.clone()
                    } else {
                        if bounded && copies.len() >= MAX_SMARTART_TRANSFER_DIAGRAM_PARTS {
                            return Err(invalid_presentation_mutation(
                                "copy SmartArt diagram graph",
                                format!(
                                    "diagram graph exceeds the {MAX_SMARTART_TRANSFER_DIAGRAM_PARTS}-part copy bound"
                                ),
                            ));
                        }
                        let copied = fresh_related_part_name(destination_package, &resolved);
                        copies.insert(resolved.clone(), copied.clone());
                        destination_package
                            .set_part(&copied, required_part(source_package, &resolved)?.to_vec());
                        pending.push(resolved);
                        copied
                    }
                } else {
                    resolved
                };
                relative_part_target(&destination_part, &copied)
            };
            destination_relationships.add_with_id(
                &relationship.id,
                &relationship.rel_type,
                &target,
            );
            let id = relationship.id.clone();
            if let Some(inserted) = destination_relationships.items.last_mut() {
                inserted.target_mode = relationship.target_mode.clone();
            }
            relationship_map.insert(relationship.id.clone(), id);
        }
        let rewritten = rewrite_exact_rel_ids(source_xml, &relationship_map).map_err(|error| {
            Error::MalformedPart {
                part_name: part.clone(),
                message: error.to_string(),
            }
        })?;
        destination_package.set_part(&destination_part, rewritten);
        let destination_relationships = source_relationships
            .rebind_for_import(destination_relationships.items, &HashMap::new())?;
        if source_package.get_part_rels(&part).is_some() {
            destination_package.set_part_rels(&destination_part, destination_relationships);
        }
    }
    Ok(root)
}

/// Copies one part and the parts it relates to, under fresh names.
///
/// Relationship ids are kept, so the copied bytes need no rewrite. Images go
/// through the media store and reuse equal media. A part reached twice is
/// copied once. The graph was bounded by `preflight_carried_part_graph`.
pub(super) fn duplicate_part_graph(
    source_package: &OpcPackage,
    destination_package: &mut OpcPackage,
    media_store: &mut MediaStore,
    source_part: &str,
    copies: &mut HashMap<String, String>,
) -> Result<String> {
    if let Some(existing) = copies.get(source_part) {
        return Ok(existing.clone());
    }
    let bytes = required_part(source_package, source_part)?.to_vec();
    let destination_part = fresh_related_part_name(destination_package, source_part);
    copies.insert(source_part.to_owned(), destination_part.clone());
    destination_package.set_part(&destination_part, bytes);
    if let Some(content_type) = source_package.content_types.content_type_for(source_part)
        && destination_package
            .content_types
            .content_type_for(&destination_part)
            != Some(content_type)
    {
        destination_package
            .content_types
            .add_override(&destination_part, content_type);
    }
    let source_relationships = source_package
        .get_part_rels(source_part)
        .cloned()
        .unwrap_or_default();
    let mut destination_relationships = Relationships::new();
    for relationship in &source_relationships.items {
        let target = if relationship_is_external(relationship) {
            relationship.target.clone()
        } else {
            let resolved = OpcPackage::resolve_rel_target(source_part, &relationship.target);
            let copied = if relationship.rel_type == rel_types::IMAGE {
                let bytes = required_part(source_package, &resolved)?.to_vec();
                media_store.insert(destination_package, &bytes, &resolved)
            } else {
                duplicate_part_graph(
                    source_package,
                    destination_package,
                    media_store,
                    &resolved,
                    copies,
                )?
            };
            relative_part_target(&destination_part, &copied)
        };
        destination_relationships.add_with_id(&relationship.id, &relationship.rel_type, &target);
        if let Some(inserted) = destination_relationships.items.last_mut() {
            inserted.target_mode = relationship.target_mode.clone();
        }
    }
    let destination_relationships =
        source_relationships.rebind_for_import(destination_relationships.items, &HashMap::new())?;
    if source_package.get_part_rels(source_part).is_some() {
        destination_package
            .part_rels
            .insert(destination_part.clone(), destination_relationships);
    }
    Ok(destination_part)
}

fn is_audio_video_relationship(relationship_type: &str) -> bool {
    matches!(
        relationship_type,
        rel_types::AUDIO | rel_types::VIDEO | rel_types::POWERPOINT_MEDIA
    )
}

fn is_diagram_relationship(relationship_type: &str) -> bool {
    matches!(
        relationship_type,
        rel_types::DIAGRAM_DATA
            | rel_types::DIAGRAM_LAYOUT
            | rel_types::DIAGRAM_QUICK_STYLE
            | rel_types::DIAGRAM_COLORS
            | rel_types::DIAGRAM_DRAWING
    )
}
