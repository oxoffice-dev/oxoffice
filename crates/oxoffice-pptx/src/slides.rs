//! Slides: the borrowed slide handles, slide size and numbering, sections, adding, removing, moving and duplicating slides, and notes text with the notes master it needs.

use super::*;
use crate::shared_parts::{give_slide_its_own_part, point_back_at};

impl Presentation {
    /// Returns the number of slides in presentation order.
    pub fn len(&self) -> usize {
        self.slides.len()
    }

    /// Returns whether the presentation has no slides.
    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }

    /// Returns one slide by zero-based presentation index.
    pub fn slide(&self, index: usize) -> Option<SlideRef<'_>> {
        self.slides.get(index).map(|record| slide_ref(record))
    }

    /// Returns one slide for in-place mutation by zero-based index.
    pub fn slide_mut(&mut self, index: usize) -> Option<SlideMut<'_>> {
        self.slides.get(index)?;
        Some(slide_mut(self, index))
    }
}

impl Presentation {
    /// Iterates slides in `p:sldIdLst` order.
    pub fn slides(&self) -> impl ExactSizeIterator<Item = SlideRef<'_>> {
        self.slides.iter().map(|record| slide_ref(record))
    }
}

impl Presentation {
    /// Returns the optional slide dimensions in EMUs.
    pub fn slide_size(&self) -> Option<(Emu, Emu)> {
        self.presentation
            .slide_size
            .as_ref()
            .map(|size| (size.cx, size.cy))
    }

    /// Replaces the slide dimensions after validating both values.
    ///
    /// A size type such as `screen16x9` names the old dimensions, so new ones
    /// drop it and the deck reads as a custom size.
    pub fn set_slide_size(&mut self, width: Emu, height: Emu) -> Result<()> {
        self.presentation
            .set_slide_size(width, height)
            .map_err(|error| Error::InvalidPresentationMutation {
                operation: "set slide size",
                message: error.to_string(),
            })
    }

    /// Reads the optional displayed start without flattening absent and zero.
    pub fn slide_number_start(&self) -> Result<Option<u32>> {
        self.presentation
            .first_slide_number()
            .map_err(|error| Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            })
    }

    /// Stages checked displayed numbering without changing physical slide IDs.
    pub fn set_slide_number_start(&mut self, value: Option<u32>) -> Result<()> {
        let start = value.unwrap_or(1);
        if let Some(last) = self.slides.len().checked_sub(1) {
            let last = u32::try_from(last)
                .map_err(|_| invalid_slide_mutation("set slide number start", "too many slides"))?;
            start.checked_add(last).ok_or_else(|| {
                invalid_slide_mutation("set slide number start", "displayed slide number overflow")
            })?;
        }
        if self.slide_number_start()? == value {
            return Ok(());
        }
        let mut staged = self.clone();
        staged.presentation.set_first_slide_number(value);
        self.commit_candidate(staged)
    }
}

impl Presentation {
    /// Returns presentation sections in producer order.
    pub fn sections(&self) -> &[Section] {
        self.presentation.sections()
    }

    /// Replaces all presentation sections after validating stable slide ids.
    pub fn set_sections(&mut self, sections: Vec<Section>) -> Result<()> {
        let mut staged = self.clone();
        staged
            .presentation
            .set_sections(sections)
            .map_err(|error| invalid_presentation_mutation("set sections", error.to_string()))?;
        self.commit_candidate(staged)
    }
}

impl Presentation {
    /// Removes one slide and its owned package graph by zero-based index.
    ///
    /// The package metadata stops naming it: `docProps/app.xml` counts the
    /// slides left and drops its title lists and text counts, unless the source-preserving
    /// patch cannot edit it (one outside its namespace, say), and the package
    /// thumbnail, which may picture it, goes.
    pub fn remove_slide(&mut self, index: usize) -> Result<()> {
        if index >= self.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index,
                slide_count: self.slides.len(),
            });
        }
        let mut staged = self.clone();
        staged.remove_slide_in_place(index)?;
        self.commit_candidate(staged)
    }

    /// Moves one slide to an existing final zero-based index.
    ///
    /// In a deck with sections, a slide that lands next to a slide of its own
    /// section stays in it, so it can lead or end that section; otherwise it
    /// joins the section of the slide before it, else of the slide after it.
    pub fn move_slide(&mut self, from_index: usize, to_index: usize) -> Result<()> {
        let slide_count = self.slides.len();
        if from_index >= slide_count {
            return Err(Error::UnknownSlideIndex {
                index: from_index,
                slide_count,
            });
        }
        if to_index >= slide_count {
            return Err(Error::UnknownSlideIndex {
                index: to_index,
                slide_count,
            });
        }
        if from_index == to_index {
            return Ok(());
        }
        let slide = self.slides.remove(from_index);
        self.slides.insert(to_index, slide);
        let slide_id = self.presentation.slide_ids.remove(from_index);
        let id = slide_id.id;
        self.presentation.slide_ids.insert(to_index, slide_id);
        self.presentation.place_slide_in_sections(id);
        Ok(())
    }

    /// Replaces one slide's speaker-note text, creating its notes slide when absent.
    ///
    /// A new notes slide follows python-pptx: it relates to the notes master
    /// and the slide, and clones the master's slide image, body and slide
    /// number placeholders. A presentation without a notes master first
    /// receives a copy of the bundled template's notes master and its theme,
    /// which needs the `default-template` or `render` feature.
    pub fn set_notes_text(&mut self, slide_index: usize, text: &str) -> Result<()> {
        self.require_slide_index(slide_index)?;
        if self.slides[slide_index].notes.is_some() {
            return slide_mut(self, slide_index).set_notes_text(text);
        }
        let mut staged = self.clone();
        staged.add_notes_slide_in_place(slide_index)?;
        slide_mut(&mut staged, slide_index).set_notes_text(text)?;
        self.commit_candidate(staged)
    }

    pub(super) fn ensure_notes_master_registration_in_place(&mut self) -> Result<()> {
        let master_part = self.notes_master_part.as_ref().ok_or_else(|| {
            footer_malformed(
                &self.presentation_part,
                "presentation has no notes master owner",
            )
        })?;
        let relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .ok_or_else(|| {
                footer_malformed(
                    &self.presentation_part,
                    "notes master has no global relationship",
                )
            })?;
        let global = relationships
            .items
            .iter()
            .filter(|relationship| relationship.rel_type == rel_types::NOTES_MASTER)
            .collect::<Vec<_>>();
        if global.len() != 1 {
            return Err(footer_malformed(
                &self.presentation_part,
                "notes master requires exactly one global relationship",
            ));
        }
        let relationship = global[0];
        reject_external(&self.presentation_part, relationship)?;
        if OpcPackage::resolve_rel_target(&self.presentation_part, &relationship.target)
            != *master_part
        {
            return Err(footer_malformed(
                &self.presentation_part,
                "notes master global relationship disagrees with its owner",
            ));
        }
        let before = self
            .presentation
            .to_xml()
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        let mut presentation = self.presentation.clone();
        presentation
            .ensure_notes_master_relationship(&relationship.id)
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        if presentation
            .to_xml()
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?
            != before
        {
            for notes in self.slides.iter().filter_map(|slide| slide.notes.as_ref()) {
                let explicit_same_master = self
                    .package
                    .get_part_rels(&notes.part_name)
                    .is_some_and(|rels| {
                        let mut masters = rels
                            .items
                            .iter()
                            .filter(|rel| rel.rel_type == rel_types::NOTES_MASTER);
                        masters.next().is_some_and(|rel| {
                            matches!(rel.target_mode.as_deref(), None | Some("Internal"))
                                && OpcPackage::resolve_rel_target(&notes.part_name, &rel.target)
                                    == *master_part
                                && masters.next().is_none()
                        })
                    });
                if !explicit_same_master {
                    return Err(footer_malformed(
                        &self.presentation_part,
                        "registering a notes master would change existing notes inheritance",
                    ));
                }
            }
        }
        self.presentation = presentation;
        Ok(())
    }

    fn add_notes_slide_in_place(&mut self, slide_index: usize) -> Result<()> {
        const OPERATION: &str = "add notes slide";
        if self.notes_master.is_none() {
            self.add_default_notes_master_in_place()?;
        }
        self.ensure_notes_master_registration_in_place()?;
        let (Some(master_part), Some(master)) = (&self.notes_master_part, &self.notes_master)
        else {
            return Err(Error::InvalidSlideMutation {
                operation: OPERATION,
                message: "presentation has no notes master".to_owned(),
            });
        };
        let mut notes = CT_NotesSlide::from_xml(
            format!(
                r#"<p:notes xmlns:a="{A_NS}" xmlns:p="{P_NS}" xmlns:r="{R_NS}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
            )
            .as_bytes(),
        )
        .expect("canonical notes slide shell");
        let tree = &mut notes.common_slide_data.shape_tree;
        let mut shape_ids = ShapeIdAllocator::scan(tree);
        for child in &master.common_slide_data.shape_tree.children {
            let ShapeTreeChild::Shape(shape) = child else {
                continue;
            };
            let Some(placeholder) = &shape.placeholder else {
                continue;
            };
            let label = match placeholder.ph_type {
                Some(PhType::SlideImage) => "Slide Image Placeholder",
                Some(PhType::Body) => "Notes Placeholder",
                Some(PhType::SlideNumber) => "Slide Number Placeholder",
                _ => continue,
            };
            let id = shape_ids.allocate();
            let mut clone = CT_Shape::new_placeholder(id, placeholder.clone())
                .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
            clone
                .set_name(&format!("{label} {}", id - 1))
                .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
            if placeholder.ph_type != Some(PhType::Body) {
                clone.text_body = None;
            }
            tree.append_child(ShapeTreeChild::Shape(clone));
        }

        let slide_part = self.slides[slide_index].part_name.clone();
        let notes_part = MediaNamer::scan(
            "/ppt/notesSlides",
            "notesSlide",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let mut notes_relationships = Relationships::new();
        notes_relationships.add(
            rel_types::NOTES_MASTER,
            &relative_part_target(&notes_part, master_part),
        );
        notes_relationships.add(
            rel_types::SLIDE,
            &relative_part_target(&notes_part, &slide_part),
        );
        let mut slide_relationships = self
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        slide_relationships.add(
            rel_types::NOTES_SLIDE,
            &relative_part_target(&slide_part, &notes_part),
        );
        let notes_xml = notes
            .to_xml()
            .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
        self.package.set_part(&notes_part, notes_xml);
        self.package.set_part_rels(&notes_part, notes_relationships);
        self.package.set_part_rels(&slide_part, slide_relationships);
        self.package
            .content_types
            .add_override(&notes_part, content_types::NOTES_SLIDE);
        Arc::make_mut(&mut self.slides[slide_index]).notes = Some(NotesRecord {
            part_name: notes_part,
            notes,
        });
        Ok(())
    }

    #[cfg(any(feature = "default-template", feature = "render"))]
    pub(super) fn add_default_notes_master_in_place(&mut self) -> Result<()> {
        let template = Self::from_bytes(DEFAULT_TEMPLATE)?;
        let (Some(template_master_part), Some(master)) =
            (template.notes_master_part, template.notes_master)
        else {
            return Err(invalid_slide_mutation(
                "add notes master",
                "the bundled template has no notes master",
            ));
        };
        let template_theme_part =
            related_internal_part(&template.package, &template_master_part, rel_types::THEME)?
                .ok_or_else(|| {
                    invalid_slide_mutation(
                        "add notes master",
                        "the bundled notes master has no theme",
                    )
                })?;
        let master_part = MediaNamer::scan(
            "/ppt/notesMasters",
            "notesMaster",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let theme_part = MediaNamer::scan(
            "/ppt/theme",
            "theme",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        self.package.set_part(
            &master_part,
            required_part(&template.package, &template_master_part)?.to_vec(),
        );
        self.package.set_part(
            &theme_part,
            required_part(&template.package, &template_theme_part)?.to_vec(),
        );
        let mut master_relationships = Relationships::new();
        master_relationships.add(
            rel_types::THEME,
            &relative_part_target(&master_part, &theme_part),
        );
        self.package
            .set_part_rels(&master_part, master_relationships);
        self.package
            .content_types
            .add_override(&master_part, content_types::NOTES_MASTER);
        self.package
            .content_types
            .add_override(&theme_part, content_types::THEME);
        let mut presentation_relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let relationship_id = presentation_relationships.add(
            rel_types::NOTES_MASTER,
            &relative_part_target(&self.presentation_part, &master_part),
        );
        self.presentation
            .ensure_notes_master_relationship(&relationship_id)
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        self.package
            .set_part_rels(&self.presentation_part, presentation_relationships);
        self.notes_master_part = Some(master_part);
        self.notes_master = Some(master);
        Ok(())
    }

    #[cfg(not(any(feature = "default-template", feature = "render")))]
    pub(super) fn add_default_notes_master_in_place(&mut self) -> Result<()> {
        Err(invalid_slide_mutation(
            "add notes master",
            "presentation has no notes master and the bundled template is not compiled in",
        ))
    }

    /// Duplicates one slide immediately after its source.
    ///
    /// The copy has its own notes and its own modern comments, whose comment
    /// and reply ids are fresh and whose anchors follow the copy. Its legacy
    /// comments are copied into a part of its own, each taking its author's
    /// next index; a legacy reply whose parent comment is on another slide
    /// brings a copy of that parent along, as an import does.
    ///
    /// `docProps/app.xml` counts the new slide and drops its title lists and text counts,
    /// which PowerPoint writes again on its next save.
    ///
    /// In a deck with sections, the slide joins the section of the slide
    /// before it, else of the slide after it.
    pub fn duplicate_slide(&mut self, index: usize) -> Result<SlideRef<'_>> {
        if index >= self.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index,
                slide_count: self.slides.len(),
            });
        }
        let mut staged = self.clone();
        let inserted = staged.duplicate_slide_in_place(index)?;
        staged.refresh_document_metadata(MetadataEdit::Addition)?;
        *self = staged;
        self.slides
            .get(inserted)
            .map(|record| slide_ref(record))
            .ok_or(Error::UnknownSlideIndex {
                index: inserted,
                slide_count: self.slides.len(),
            })
    }
}

impl Presentation {
    fn remove_slide_in_place(&mut self, index: usize) -> Result<()> {
        let record = self.slides[index].clone();
        self.presentation.remove_slide_from_sections(record.id);
        let presentation_relationship_id =
            self.presentation.slide_ids[index].relationship_id.clone();
        self.presentation
            .remove_custom_show_slide(&presentation_relationship_id)
            .map_err(|error| Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            })?;

        // Every part the slide or its notes reach is a candidate: charts with
        // their workbooks, legacy comments, embedded objects and media go
        // unless another part still references them.
        let mut candidates = HashSet::new();
        self.release_slide_jumps_to(&record.part_name, &mut candidates)?;
        collect_internal_targets(&self.package, &record.part_name, &mut candidates);
        // A notes slide another slide still relates to stays with that
        // slide, and names it.
        let sharer = |part: &str, of: fn(&SlideRecord) -> Option<&str>| {
            self.slides
                .iter()
                .enumerate()
                .find(|(other, slide)| {
                    *other != index && of(slide).is_some_and(|name| name.eq_ignore_ascii_case(part))
                })
                .map(|(_, slide)| slide.part_name.clone())
        };
        let notes_sharer = record.notes.as_ref().and_then(|notes| {
            sharer(&notes.part_name, |slide| {
                slide.notes.as_ref().map(|notes| notes.part_name.as_str())
            })
        });
        if let Some(notes) = &record.notes {
            if let Some(sharer) = &notes_sharer {
                point_back_at(
                    &mut self.package,
                    &notes.part_name,
                    &record.part_name,
                    sharer,
                );
            } else {
                collect_internal_targets(&self.package, &notes.part_name, &mut candidates);
                self.package.remove_part(&notes.part_name);
                self.package.remove_part_rels(&notes.part_name);
                self.package.content_types.remove_override(&notes.part_name);
            }
        }
        if let Some(comments) = &record.comments {
            self.package.remove_part(&comments.part_name);
            self.package.remove_part_rels(&comments.part_name);
            self.package
                .content_types
                .remove_override(&comments.part_name);
        }
        self.package.remove_part(&record.part_name);
        self.package.remove_part_rels(&record.part_name);
        self.package
            .content_types
            .remove_override(&record.part_name);
        if let Some(relationships) = self.package.get_part_rels_mut(&self.presentation_part) {
            relationships
                .items
                .retain(|relationship| relationship.id != presentation_relationship_id);
        }
        self.presentation.slide_ids.remove(index);
        self.slides.remove(index);
        prune_unreachable_parts(&mut self.package, &candidates);
        self.media_store = self.media_store.rescan(&self.package);
        self.refresh_document_metadata(MetadataEdit::Removal)
    }
}

impl Presentation {
    /// Gives slide `index` a notes slide of its own before an edit writes
    /// its notes, when another slide relates to the same part (decks edited
    /// by oxoffice 0.1.0 can): the copy holds what the part holds, and the
    /// part then names one of the slides that keep it. Nothing changes for
    /// notes no other slide relates to.
    pub(super) fn own_notes_in_place(&mut self, index: usize) {
        let record = &self.slides[index];
        let Some(notes) = &record.notes else {
            return;
        };
        let Some(keeper) =
            self.slides
                .iter()
                .enumerate()
                .find(|(other, slide)| {
                    *other != index
                        && slide.notes.as_ref().is_some_and(|other| {
                            other.part_name.eq_ignore_ascii_case(&notes.part_name)
                        })
                })
                .map(|(_, slide)| slide.part_name.clone())
        else {
            return;
        };
        let slide_part = record.part_name.clone();
        let original = notes.part_name.clone();
        let relationship_id = self
            .package
            .get_part_rels(&slide_part)
            .and_then(|relationships| relationships.get_by_type(rel_types::NOTES_SLIDE))
            .map(|relationship| relationship.id.clone());
        let bytes = self.package.get_part(&original).map(<[u8]>::to_vec);
        let (Some(relationship_id), Some(bytes)) = (relationship_id, bytes) else {
            return;
        };
        let Ok(copy) =
            give_slide_its_own_part(&mut self.package, &slide_part, &relationship_id, bytes)
        else {
            return;
        };
        point_back_at(&mut self.package, &original, &slide_part, &keeper);
        if let Some(notes) = &mut Arc::make_mut(&mut self.slides[index]).notes {
            notes.part_name = copy;
        }
    }
}

impl Presentation {
    /// Makes every hyperlink of another slide that jumps to `slide_part` do
    /// nothing and removes its relationship, as PowerPoint does when the
    /// target slide is deleted.
    ///
    /// Every `a:hlinkClick`, `a:hlinkHover`, or `a:hlinkMouseOver` that
    /// named it, on a shape or a text run, keeps an empty `r:id` and the
    /// action `ppaction://noaction`. Every relationship that the slide named
    /// only inside a released element goes, a click sound included, and the
    /// internal targets of those relationships join `candidates` for pruning.
    fn release_slide_jumps_to(
        &mut self,
        slide_part: &str,
        candidates: &mut HashSet<String>,
    ) -> Result<()> {
        for record in &mut self.slides {
            let Some(relationships) = self.package.get_part_rels_mut(&record.part_name) else {
                continue;
            };
            let released: HashSet<String> = relationships
                .items
                .iter()
                .filter(|relationship| {
                    !relationship_is_external(relationship)
                        && OpcPackage::resolve_rel_target(&record.part_name, &relationship.target)
                            == slide_part
                })
                .map(|relationship| relationship.id.clone())
                .collect();
            if released.is_empty() || record.part_name == slide_part {
                continue;
            }
            let malformed = |error: OxmlError| Error::MalformedPart {
                part_name: record.part_name.clone(),
                message: error.to_string(),
            };
            let original = record.slide.to_xml().map_err(malformed)?;
            let xml = release_hyperlinks(&original, &released).map_err(malformed)?;
            let before: HashSet<String> = relationship_ids(&original)
                .map_err(malformed)?
                .into_iter()
                .collect();
            let after: HashSet<String> = relationship_ids(&xml)
                .map_err(malformed)?
                .into_iter()
                .collect();
            relationships.items.retain(|relationship| {
                let dropped = (released.contains(&relationship.id)
                    || before.contains(&relationship.id))
                    && !after.contains(&relationship.id);
                if dropped && !relationship_is_external(relationship) {
                    let target =
                        OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                    if target != slide_part {
                        candidates.insert(target);
                    }
                }
                !dropped
            });
            let slide = CT_Slide::from_xml(&xml).map_err(malformed)?;
            Arc::make_mut(record).slide = slide;
        }
        Ok(())
    }

    fn duplicate_slide_in_place(&mut self, index: usize) -> Result<usize> {
        let source_package = self.package.clone();
        let source = SlideRecord::clone(&self.slides[index]);
        // Legacy comments are copied as an import copies them: a part of
        // the copy's own, whose comments take their authors' next indexes.
        let mut legacy_comments = HashMap::new();
        let source_slides = self.slides.clone();
        let presentation_part = self.presentation_part.clone();
        self.import_classic_comment_graphs(
            &source_slides,
            &presentation_part,
            &source_package,
            &[index],
            &mut legacy_comments,
        )?;
        let inserted = self.copy_slide_from_in_place(
            &source_package,
            source,
            index + 1,
            "duplicate slide",
            None,
            None,
        )?;
        let slide_part = self.slides[inserted].part_name.clone();
        if let Some(relationship) =
            self.package
                .get_part_rels_mut(&slide_part)
                .and_then(|relationships| {
                    relationships
                        .items
                        .iter_mut()
                        .find(|relationship| relationship.rel_type == rel_types::COMMENTS)
                })
            && let Some(copied) = legacy_comments.get(&OpcPackage::resolve_rel_target(
                &slide_part,
                &relationship.target,
            ))
        {
            relationship.target = relative_part_target(&slide_part, copied);
        }
        Ok(inserted)
    }
}

impl Presentation {
    /// Inserts a new slide at a final zero-based position, including append.
    ///
    /// Layout selection, creation and placement share one staged copy, which
    /// replaces the presentation only when all three succeed. It edits the
    /// package in place, as `add_slide` and `move_slide` do, so it publishes
    /// without re-reading the whole package.
    pub fn add_slide_at(&mut self, layout_index: usize, position: usize) -> Result<usize> {
        if layout_index >= self.layouts.len() {
            return Err(Error::UnknownLayoutIndex {
                index: layout_index,
                layout_count: self.layouts.len(),
            });
        }
        let old_count = self.slides.len();
        let new_count = old_count.checked_add(1).ok_or(Error::SlideIdExhausted)?;
        if position >= new_count {
            return Err(Error::UnknownSlideIndex {
                index: position,
                slide_count: new_count,
            });
        }
        let mut candidate = self.clone();
        let id = candidate.add_slide(layout_index)?.id();
        // The new slide takes the section of where it lands, not of the end
        // of the deck it was appended to first.
        candidate.presentation.remove_slide_from_sections(id);
        candidate.move_slide(old_count, position)?;
        candidate.presentation.place_slide_in_sections(id);
        *self = candidate;
        Ok(position)
    }

    /// Synthesizes a new slide from one layout selected by zero-based index.
    ///
    /// `docProps/app.xml` counts the new slide and drops its title lists and text counts,
    /// which PowerPoint writes again on its next save.
    ///
    /// In a deck with sections, the slide joins the section of the slide
    /// before it, else of the slide after it.
    pub fn add_slide(&mut self, layout_index: usize) -> Result<SlideRef<'_>> {
        let layout = self
            .layouts
            .get(layout_index)
            .ok_or(Error::UnknownLayoutIndex {
                index: layout_index,
                layout_count: self.layouts.len(),
            })?;
        let layout_part = layout.part_name.clone();
        let placeholders = layout
            .layout
            .common_slide_data
            .shape_tree
            .children
            .iter()
            .filter_map(|child| match child {
                ShapeTreeChild::Shape(shape) => shape.placeholder.as_ref(),
                _ => None,
            })
            .filter(|placeholder| !is_latent_placeholder(placeholder))
            .cloned()
            .collect::<Vec<_>>();

        let mut shape_tree = CT_ShapeTree::new();
        let mut shape_ids = ShapeIdAllocator::scan(&shape_tree);
        for placeholder in placeholders {
            let id = shape_ids.allocate();
            let name = generated_placeholder_name(&shape_tree, &placeholder, id)?;
            let mut shape = CT_Shape::new_placeholder(id, placeholder).map_err(|error| {
                Error::MalformedPart {
                    part_name: "new slide".to_owned(),
                    message: error.to_string(),
                }
            })?;
            shape
                .set_name(&name)
                .map_err(|error| invalid_slide_mutation("add slide", error.to_string()))?;
            shape_tree.children.push(ShapeTreeChild::Shape(shape));
        }

        for shape in self.new_slide_latent_shapes(layout_index, &mut shape_ids)? {
            shape_tree.children.push(shape);
        }

        let slide_part = MediaNamer::scan(
            "/ppt/slides",
            "slide",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let slide = CT_Slide::new(shape_tree);
        let slide_xml = slide.to_xml().map_err(|error| Error::MalformedPart {
            part_name: slide_part.clone(),
            message: error.to_string(),
        })?;

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

        let mut slide_relationships = Relationships::new();
        slide_relationships.add(
            rel_types::SLIDE_LAYOUT,
            &relative_part_target(&slide_part, &layout_part),
        );
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
        self.presentation.slide_ids.push(slide_id);
        self.presentation.place_slide_in_sections(id);
        self.slides.push(Arc::new(SlideRecord {
            id,
            part_name: slide_part,
            slide,
            notes: None,
            comments: None,
        }));
        self.refresh_document_metadata(MetadataEdit::Addition)?;
        self.slides
            .last()
            .map(|record| slide_ref(record))
            .ok_or(Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: "new slide record was not retained".to_owned(),
            })
    }
}

fn generated_placeholder_name(
    tree: &CT_ShapeTree,
    placeholder: &CT_Placeholder,
    id: u32,
) -> Result<String> {
    let base = match placeholder.ph_type.as_ref().unwrap_or(&PhType::Object) {
        PhType::Title | PhType::CenteredTitle => "Title",
        PhType::Subtitle => "Subtitle",
        PhType::Body => "Text Placeholder",
        PhType::Object => "Content Placeholder",
        PhType::Picture => "Picture Placeholder",
        PhType::Chart => "Chart Placeholder",
        PhType::Table => "Table Placeholder",
        PhType::Diagram => "SmartArt Placeholder",
        PhType::Media => "Media Placeholder",
        PhType::ClipArt => "ClipArt Placeholder",
        PhType::DateTime => "Date Placeholder",
        PhType::Footer => "Footer Placeholder",
        PhType::SlideNumber => "Slide Number Placeholder",
        PhType::Header => "Header Placeholder",
        PhType::SlideImage => "Slide Image Placeholder",
        PhType::VerticalTitle => "Vertical Title",
        PhType::VerticalBody => "Vertical Text Placeholder",
        PhType::VerticalObject => "Vertical Content Placeholder",
        PhType::Other(_) => "Placeholder",
    };
    let vertical = placeholder.orientation() == Some("vert") && !base.starts_with("Vertical ");
    // Unknown imported type naming keeps the existing generic ID convention.
    let mut ordinal = if matches!(placeholder.ph_type, Some(PhType::Other(_))) {
        id
    } else {
        id.saturating_sub(1)
    };
    loop {
        let name = format!(
            "{}{base} {ordinal}",
            if vertical { "Vertical " } else { "" }
        );
        if !tree
            .children
            .iter()
            .any(|child| child.non_visual_name().as_deref() == Some(name.as_str()))
        {
            return Ok(name);
        }
        ordinal = ordinal.checked_add(1).ok_or_else(|| {
            invalid_slide_mutation("add slide", "placeholder name space exhausted")
        })?;
    }
}

fn is_latent_placeholder(placeholder: &CT_Placeholder) -> bool {
    matches!(
        placeholder.ph_type.as_ref(),
        Some(PhType::DateTime | PhType::Footer | PhType::SlideNumber)
    )
}

pub(super) fn slide_ref(record: &SlideRecord) -> SlideRef<'_> {
    SlideRef { record }
}

pub(super) fn slide_mut(presentation: &mut Presentation, index: usize) -> SlideMut<'_> {
    SlideMut {
        presentation,
        index,
    }
}

impl<'a> SlideMut<'a> {
    /// Sets whether the slide is hidden from the slideshow.
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.presentation.slides[self.index].slide.hidden() == hidden {
            return;
        }
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .set_hidden(hidden);
        // Only the hidden-slide count changes. Metadata that cannot be
        // refreshed is left as it was: it is no reason to refuse a hide.
        let _ = self
            .presentation
            .refresh_document_metadata(MetadataEdit::Visibility);
    }

    /// Sets a direct DrawingML fill without replacing a theme reference.
    pub fn set_background(&mut self, fill: Fill) -> Result<()> {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .set_background(fill)
            .map_err(|error| Error::InvalidSlideMutation {
                operation: "set slide background",
                message: error.to_string(),
            })
    }

    /// Removes only a direct-fill background.
    pub fn clear_background(&mut self) {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .clear_background();
    }

    /// Removes any slide background, direct fill or theme reference, so the
    /// slide follows its layout and master background.
    pub fn remove_background(&mut self) {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .common_slide_data
            .background = None;
    }

    /// Replaces speaker-note text while preserving its body placeholder.
    pub fn set_notes_text(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.presentation.slides[self.index]
            .notes
            .as_ref()
            .ok_or_else(|| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: "slide has no notes part".to_owned(),
            })?
            .notes
            .clone();
        let body = first_notes_body_mut(&mut candidate.common_slide_data.shape_tree.children)
            .ok_or_else(|| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: "notes slide has no body placeholder".to_owned(),
            })?;
        let formatting_run = body.paragraphs().iter().find_map(|paragraph| {
            paragraph.runs.iter().find_map(|run| match run {
                TextRun::Run(run) => Some(run.clone()),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
        });
        // An empty first line would drop the formatting run before later
        // paragraphs copy it. A space keeps that copy, and the line loop
        // writes the caller's text afterwards.
        let seeded = (formatting_run.is_some() && (text.is_empty() || text.starts_with('\n')))
            .then(|| {
                text.split('\n')
                    .map(|line| if line.is_empty() { " " } else { line })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        body.set_text(seeded.as_deref().unwrap_or(text));
        // The general DrawingML setter treats CRLF as a paragraph separator
        // and drops every run when a line is empty. Notes authoring retains
        // the caller's exact line endings and the first regular run.
        for (index, line) in text.split('\n').enumerate() {
            let paragraph =
                body.paragraph_mut(index)
                    .ok_or_else(|| Error::InvalidSlideMutation {
                        operation: "set notes text",
                        message: "notes paragraph count changed during staging".to_owned(),
                    })?;
            assign_notes_line(paragraph, line, formatting_run.as_ref());
        }
        candidate
            .to_xml()
            .map_err(|error| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: error.to_string(),
            })?;
        // Only an edit that succeeds unshares the notes.
        self.presentation.own_notes_in_place(self.index);
        if let Some(notes) = &mut Arc::make_mut(&mut self.presentation.slides[self.index]).notes {
            notes.notes = candidate;
        }
        Ok(())
    }

    /// Appends a rectangular table at the top of the slide's z-order.
    pub fn add_table(
        &mut self,
        rows: usize,
        columns: usize,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| table_member(id, rows, columns, left, top, width, height))
    }

    /// Appends a no-fill textbox at the top of the slide's z-order.
    pub fn add_textbox(
        &mut self,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| textbox_member(id, left, top, width, height))
    }

    /// Appends an ordinary preset shape at the top of the slide's z-order.
    ///
    /// The shape carries the theme style python-pptx writes, so it shows the
    /// theme's accent fill, line, and effect with light text until direct
    /// properties replace them.
    pub fn add_shape(
        &mut self,
        preset: &str,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| preset_member(id, preset, left, top, width, height))
    }

    /// Appends a preset shape without a theme style for HTML and ODP imports.
    #[cfg(feature = "default-template")]
    pub(crate) fn add_unstyled_shape(
        &mut self,
        preset: &str,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| unstyled_preset_member(id, preset, left, top, width, height))
    }

    /// Appends a free-standing connector at the top of the slide's z-order.
    ///
    /// The connector carries the theme style python-pptx writes, so it shows
    /// the theme's line until a direct line replaces it.
    pub fn add_connector(
        &mut self,
        connector: ConnectorType,
        begin_x: Emu,
        begin_y: Emu,
        end_x: Emu,
        end_y: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| connector_member(id, connector, begin_x, begin_y, end_x, end_y))
    }

    /// Appends an empty group at the top of the slide's z-order.
    pub fn add_group_shape(&mut self) -> Result<ShapeMut<'_>> {
        self.append_member(|id| Ok(group_member(id, false)))
    }

    fn shape_tree(&mut self) -> &mut CT_ShapeTree {
        &mut Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .common_slide_data
            .shape_tree
    }

    /// Returns one immediate z-order child as a read-only handle.
    pub fn shape(&self, index: usize) -> Option<ShapeRef<'_>> {
        self.presentation.slides[self.index]
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)
            .map(shape_ref)
    }

    /// Returns one immediate z-order child for in-place mutation.
    pub fn shape_mut(&mut self, index: usize) -> Option<ShapeMut<'_>> {
        self.shape_tree().children.get(index)?;
        Some(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }

    /// Consumes this handle and returns one immediate z-order child for mutation.
    pub fn into_shape_mut(self, index: usize) -> Option<ShapeMut<'a>> {
        self.presentation.slides[self.index]
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)?;
        Some(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }
    fn append_member(
        &mut self,
        build: impl FnOnce(u32) -> Result<ShapeTreeChild>,
    ) -> Result<ShapeMut<'_>> {
        let owner = ShapeOwner::Slide(self.index);
        if self.presentation.local_append_eligible(owner, &[])? {
            let index = self.presentation.append_locally(owner, &[], build)?;
            return Ok(owned_shape_mut(self.presentation, owner, vec![index]));
        }
        let mut candidate = self.presentation.clone();
        let tree = &mut Arc::make_mut(&mut candidate.slides[self.index])
            .slide
            .common_slide_data
            .shape_tree;
        let index = append_new_member(tree, &[], build)?;
        candidate.refresh_connector_attachments(
            ShapeOwner::Slide(self.index),
            &self.presentation.owner_data(owner)?.shape_tree.children,
            &HashSet::new(),
        )?;
        candidate.slides[self.index]
            .slide
            .to_xml()
            .map_err(|error| Error::MalformedPart {
                part_name: candidate.slides[self.index].part_name.clone(),
                message: error.to_string(),
            })?;
        *self.presentation = candidate;
        Ok(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }
}

impl<'a> SlideRef<'a> {
    /// Returns the producer-assigned slide id.
    pub fn id(&self) -> u32 {
        self.record.id
    }

    /// Returns the optional `p:cSld` name.
    pub fn name(&self) -> Option<&str> {
        self.record.slide.common_slide_data.name.as_deref()
    }

    /// Returns whether the slide is hidden from the slideshow.
    pub fn hidden(&self) -> bool {
        self.record.slide.hidden()
    }

    /// Returns whether the slide carries an explicit `p:bg` element.
    pub fn has_explicit_background(&self) -> bool {
        self.record.slide.has_explicit_background()
    }

    /// Returns the direct `p:bgPr` fill when the slide background has one.
    pub fn background_fill(&self) -> Option<&'a Fill> {
        match self
            .record
            .slide
            .common_slide_data
            .background
            .as_ref()?
            .rendering()
        {
            BackgroundRendering::Properties(fill) => fill.as_deref(),
            BackgroundRendering::Reference { .. } | BackgroundRendering::Unsupported(_) => None,
        }
    }

    /// Returns one immediate z-order child by zero-based index.
    pub fn shape(&self, index: usize) -> Option<ShapeRef<'a>> {
        self.record
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)
            .map(shape_ref)
    }

    /// Returns the first title or centered-title placeholder on the slide.
    pub fn title(&self) -> Option<ShapeRef<'_>> {
        self.shapes().find(|shape| {
            matches!(
                shape_placeholder(shape.child).and_then(|placeholder| placeholder.ph_type.as_ref()),
                Some(PhType::Title | PhType::CenteredTitle)
            )
        })
    }

    /// Returns the placeholder whose OOXML placeholder index equals `index`.
    pub fn placeholder(&self, index: u32) -> Option<ShapeRef<'_>> {
        self.shapes().find(|shape| {
            shape_placeholder(shape.child).map(|placeholder| placeholder.idx.unwrap_or(0))
                == Some(index)
        })
    }

    /// Iterates the immediate shape tree in z-order. [`Shapes::walk`] also
    /// visits the shapes inside groups.
    pub fn shapes(&self) -> Shapes<'a> {
        shapes(&self.record.slide.common_slide_data.shape_tree.children)
    }

    /// Returns visible shape and table text recursively in z-order.
    pub fn text(&self) -> String {
        let mut text = Vec::new();
        collect_text(
            &self.record.slide.common_slide_data.shape_tree.children,
            &mut text,
        );
        text.join("\n")
    }

    /// Compares the accepted notes model, including run distribution and retained XML.
    /// Missing notes compare equal only to missing notes.
    pub fn notes_text_matches(&self, other: &SlideRef<'_>) -> bool {
        self.record.notes.as_ref().map(|record| &record.notes)
            == other.record.notes.as_ref().map(|record| &record.notes)
    }

    /// Returns the one direct writable notes-body text, or no body.
    ///
    /// Grouped, alternate and multiple effective bodies refuse this bounded
    /// frame projection. `notes_text` retains its aggregate-read behavior.
    pub fn notes_text_frame_text(&self) -> Result<Option<String>> {
        let Some(notes) = &self.record.notes else {
            return Ok(None);
        };
        let children = &notes.notes.common_slide_data.shape_tree.children;
        fn effective_bodies(children: &[ShapeTreeChild]) -> usize {
            children.iter().map(|child| {
                let body = usize::from(matches!(child, ShapeTreeChild::Shape(shape)
                    if shape.placeholder.as_ref().is_some_and(|ph| ph.effective_type() == PhType::Body)));
                body + effective_bodies(shape_ref(child).child_slice())
            }).sum()
        }
        let count = effective_bodies(children);
        if count == 0 {
            return Ok(None);
        }
        let direct = children.iter().find_map(|child| match child {
            ShapeTreeChild::Shape(shape)
                if shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|ph| ph.effective_type() == PhType::Body) =>
            {
                Some(shape)
            }
            _ => None,
        });
        match (count, direct) {
            (1, Some(shape)) => Ok(Some(shape.text_body.as_ref().map_or_else(String::new, CT_TextBody::plain_text))),
            _ => Err(Error::InvalidSlideMutation {
                operation: "notes text frame",
                message: "notes_text_frame requires one direct Body placeholder without grouped or alternate bodies; use Slide.notes_text for aggregate reads (its existing setter selects the first writable body)".to_owned(),
            }),
        }
    }

    /// Returns speaker-note text, distinguishing no notes part from empty notes.
    pub fn notes_text(&self) -> Option<String> {
        self.record
            .notes
            .as_ref()
            .map(|notes| notes.notes.notes_text())
    }
}

/// Writes one notes line and keeps the body's first regular run.
///
/// An empty line stores that run with empty text, so its formatting and
/// unmodelled content stay. A line with text uses the same run. Without one,
/// the general paragraph assignment applies, including its empty-text drop.
fn assign_notes_line(
    paragraph: &mut CT_TextParagraph,
    line: &str,
    formatting: Option<&CT_RegularTextRun>,
) {
    let has_regular_run = paragraph
        .runs
        .iter()
        .any(|run| matches!(run, TextRun::Run(_)));
    if (line.is_empty() || !has_regular_run)
        && let Some(source) = formatting
    {
        let mut run = source.clone();
        run.set_text("");
        if !paragraph.runs.is_empty() {
            paragraph.set_text("");
        }
        paragraph.add_run("");
        if let Some(slot) = paragraph.runs.first_mut() {
            *slot = TextRun::Run(run);
        }
        if line.is_empty() {
            return;
        }
    }
    paragraph.set_text(line);
}

fn first_notes_body_mut(children: &mut [ShapeTreeChild]) -> Option<&mut CT_TextBody> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape)
                if shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|placeholder| placeholder.effective_type() == PhType::Body) =>
            {
                return Some(shape.text_body.get_or_insert_with(CT_TextBody::new));
            }
            ShapeTreeChild::GroupShape(group) => {
                if let Some(body) = first_notes_body_mut(&mut group.children) {
                    return Some(body);
                }
            }
            ShapeTreeChild::Shape(_)
            | ShapeTreeChild::Picture(_)
            | ShapeTreeChild::GraphicFrame(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => {}
        }
    }
    None
}
