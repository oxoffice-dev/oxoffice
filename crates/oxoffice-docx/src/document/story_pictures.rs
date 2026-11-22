//! Pictures, text boxes and hyperlinks inside stories.

use super::*;

impl Document {
    fn append_fragment_to_story(
        &mut self,
        story: &StoryId,
        mut fragment: ContentFragment,
    ) -> Result<()> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let destination = ContentLocation::end(story.clone());
        let (boundary, _, _) = validated_content_boundary(&source_xml, &owner, &destination)?;
        fragment.source_part_name = Some(part_name.clone());
        fragment.source_story_kind = Some(story.kind);
        fragment.source_owner_index = Some(story.owner_index);
        let fragment_xml = content_fragment_for_insertion(self, story, &fragment)?;
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &owner, boundary, fragment_xml)?;
        set_story_source_xml(self, &part_name, updated)
    }

    /// Append an inline picture paragraph to one checked story owner.
    pub fn add_picture_to_story(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) -> Result<()> {
        self.insert_picture_to_story(
            story,
            None,
            image_data,
            image_filename,
            Some(width),
            Some(height),
        )?;
        Ok(())
    }

    /// Insert an inline picture at one checked physical run boundary.
    /// The media, owning relationship and drawing are published together.
    pub fn insert_picture_at(
        &mut self,
        position: &StoryRunPosition,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
    ) -> Result<()> {
        self.insert_picture_with_options_at(
            position,
            image_data,
            image_filename,
            width,
            height,
            None,
        )
        .map(|_| ())
    }

    /// Import picture metadata and placement through the same checked owner transaction.
    pub fn insert_picture_with_options_at(
        &mut self,
        position: &StoryRunPosition,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<PictureOccurrence> {
        let format = oxoffice_media::ImageFormat::sniff(image_data)
            .ok_or_else(|| Error::Other("picture image bytes are unsupported".to_owned()))?;
        if matches!(
            format,
            oxoffice_media::ImageFormat::Png
                | oxoffice_media::ImageFormat::Jpeg
                | oxoffice_media::ImageFormat::Gif
                | oxoffice_media::ImageFormat::Bmp
                | oxoffice_media::ImageFormat::Webp
        ) && oxoffice_media::probe(image_data).is_none()
        {
            return Err(Error::Other(
                "picture image bytes are unsupported".to_owned(),
            ));
        }
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) if width.to_emu() > 0 && height.to_emu() > 0 => {
                (width, height)
            }
            (None, None) => {
                let size = oxoffice_media::probe(image_data)
                    .and_then(|info| info.native_size(72.0))
                    .ok_or_else(|| Error::UnavailableImageDimensions {
                        filename: image_filename.to_owned(),
                    })?;
                (Length::emu(size.width_emu), Length::emu(size.height_emu))
            }
            _ => {
                return Err(Error::Other(
                    "picture dimensions require two positive lengths or neither".to_owned(),
                ));
            }
        };
        let mut options = options.unwrap_or(PictureOptions {
            width,
            height,
            crop: None,
            anchor: None,
            name: None,
            description: None,
            title: None,
            decorative: false,
        });
        options.width = width;
        options.height = height;
        validate_picture_options(&options)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let snapshot = candidate.story_range_paragraph_snapshot(&position.location)?;
        let original_paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let original = original_paragraph.accepted_view();
        if position.run_index > original.runs.len() {
            return Err(Error::Other("picture run boundary is out of range".into()));
        }
        let ordinal = original
            .runs
            .iter()
            .take(position.run_index)
            .flat_map(|run| {
                run.content
                    .iter()
                    .filter_map(|content| match content {
                        RunContent::Drawing(drawing) => Some(drawing),
                        _ => None,
                    })
                    .chain(run.alt_drawings.iter())
            })
            // The pictures `pictures()` lists: an embedded or a linked picture,
            // not a chart.
            .filter(|drawing| {
                drawing.inline.as_ref().is_some_and(|i| {
                    i.chart_rel_id.is_none() && (!i.embed_id.is_empty() || i.link_id.is_some())
                }) || drawing.anchor.as_ref().is_some_and(|a| {
                    a.chart_rel_id.is_none() && (!a.embed_id.is_empty() || a.link_id.is_some())
                })
            })
            .count();
        let relationship = candidate.add_image_relationship_checked(
            position.location.story.part_name(),
            image_data,
            image_filename,
        )?;
        let drawing = picture_drawing(
            &relationship,
            candidate.identifiers.reserve_drawing_id()?,
            &options,
        );
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        close_typed_story_drawing_namespaces(&mut paragraph)?;
        candidate.insert_story_run_staged(position, paragraph.runs.remove(0))?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let picture = reopened
            .pictures()?
            .into_iter()
            .find(|picture| {
                picture.location.story.kind == position.location.story.kind
                    && picture.location.story.part_name == position.location.story.part_name
                    && picture.location.story.owner_index == position.location.story.owner_index
                    && picture.location.index_path == position.location.index_path
                    && picture.ordinal == ordinal
            })
            .ok_or_else(|| Error::Other("inserted picture occurrence was not projected".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(picture)
    }

    /// Insert an inline picture paragraph after checked story content.
    ///
    /// Omitting `after` appends to the story. Width and height must either both
    /// be present or both be omitted. Omitted dimensions use the image's native
    /// size at 72 DPI.
    pub fn insert_picture_to_story(
        &mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
    ) -> Result<ContentLocation> {
        self.insert_picture_to_story_with_options(
            story,
            after,
            image_data,
            image_filename,
            width,
            height,
            None,
        )
    }

    /// Import picture metadata and placement through the same checked owner transaction.
    pub fn insert_picture_to_story_with_options(
        &mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<ContentLocation> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let candidate = self.clone_for_staging();
        let (updated, location) = Self::insert_picture_candidate(
            candidate,
            story,
            after,
            image_data,
            image_filename,
            options,
            None,
        )?;
        self.commit_staged_mutation(updated);
        Ok(location)
    }

    /// Consume a private candidate using the same checked picture transaction.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_picture_to_story_with_options_owned(
        mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<(Self, ContentLocation)> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let mut origin = self.take_staged_commit_origin();
        let (updated, location) = Self::insert_picture_candidate(
            self,
            story,
            after,
            image_data,
            image_filename,
            options,
            None,
        )?;
        Ok((
            Self::finish_staged_candidate(
                &origin.package,
                origin.signature_invalidated,
                &mut origin.normal_engine,
                &mut origin.bundled_engine,
                updated,
            ),
            location,
        ))
    }

    /// Consume a private candidate and return the complete checked picture inventory.
    /// Existing insertion-only methods do not perform this inventory.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_picture_to_story_with_options_and_inventory_owned(
        mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<(Self, ContentLocation, Vec<PictureOccurrence>)> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let mut origin = self.take_staged_commit_origin();
        let mut inventory = None;
        let (updated, location) = Self::insert_picture_candidate(
            self,
            story,
            after,
            image_data,
            image_filename,
            options,
            Some(&mut inventory),
        )?;
        let updated = Self::finish_staged_candidate(
            &origin.package,
            origin.signature_invalidated,
            &mut origin.normal_engine,
            &mut origin.bundled_engine,
            updated,
        );
        // None is only an explicit shared-preparation decline. Collector errors
        // propagate, never become a second attempted inventory.
        let inventory = match inventory {
            Some(inventory) => inventory,
            None => updated.pictures()?,
        };
        Ok((updated, location, inventory))
    }

    fn checked_insertion_picture_options(
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<PictureOptions> {
        let format = oxoffice_media::ImageFormat::sniff(image_data)
            .ok_or_else(|| Error::Other("picture image bytes are unsupported".to_owned()))?;
        if matches!(
            format,
            oxoffice_media::ImageFormat::Png
                | oxoffice_media::ImageFormat::Jpeg
                | oxoffice_media::ImageFormat::Gif
                | oxoffice_media::ImageFormat::Bmp
                | oxoffice_media::ImageFormat::Webp
        ) && oxoffice_media::probe(image_data).is_none()
        {
            return Err(Error::Other(
                "picture image bytes are unsupported".to_owned(),
            ));
        }
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) => (width, height),
            (None, None) => {
                let native_size = oxoffice_media::probe(image_data)
                    .and_then(|info| info.native_size(72.0))
                    .ok_or_else(|| Error::UnavailableImageDimensions {
                        filename: image_filename.to_owned(),
                    })?;
                (
                    Length::emu(native_size.width_emu),
                    Length::emu(native_size.height_emu),
                )
            }
            _ => {
                return Err(Error::Other(
                    "picture width and height must both be provided or both be omitted".to_owned(),
                ));
            }
        };
        let mut options = options.unwrap_or(PictureOptions {
            width,
            height,
            crop: None,
            anchor: None,
            name: None,
            description: None,
            title: None,
            decorative: false,
        });
        options.width = width;
        options.height = height;
        validate_picture_options(&options)?;
        Ok(options)
    }

    fn insert_picture_candidate(
        mut candidate: Self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        options: PictureOptions,
        inventory: Option<&mut Option<Vec<PictureOccurrence>>>,
    ) -> Result<(Self, ContentLocation)> {
        candidate.flush_to_package()?;
        let (source, mut story_owner) = candidate.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        // The ordinary story projection is typed XML. For picture insertion,
        // use the retained main part when it still describes the current model.
        if part_name == candidate.doc_part_name
            && let Some(retained) = candidate.package.get_part(&part_name)
            && retained != source_xml
            && candidate.main_part_parses_to_model(candidate.kept_main_part().as_deref())
        {
            source_xml = retained.to_vec();
            story_owner = scan_story_owners(&source_xml, StoryKind::Body)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
                .ok_or_else(|| Error::Other("picture story source disappeared".to_owned()))?;
        }
        // Typed projection can allocate private field identities. Field-bearing
        // sources keep the original scan schedule before and after publication.
        let shared_items = if !holds_field_words(&source_xml) {
            Some(scan_story_items(&source_xml, &story_owner, None)?)
        } else {
            None
        };
        let direct_items = match &shared_items {
            Some(items) => direct_story_content_items_in(&source_xml, &story_owner, items)?,
            None => direct_story_content_items(&source_xml, &story_owner)?,
        };
        let (boundary, inserted_direct_index) = match after {
            Some(after) => {
                if after.story != *story {
                    return Err(Error::Other(
                        "picture placement must belong to the target story".to_owned(),
                    ));
                }
                let item = validated_direct_content_item(&source_xml, &story_owner, after)?;
                let index = direct_items
                    .iter()
                    .position(|candidate| candidate.full == item.full)
                    .ok_or_else(|| Error::Other("picture placement was not found".to_owned()))?;
                (item.full.end, index + 1)
            }
            None => (
                match &shared_items {
                    Some(items) => story_owner_content_end_in(&source_xml, &story_owner, items)?,
                    None => story_owner_content_end(&source_xml, &story_owner)?,
                },
                direct_items.len(),
            ),
        };
        let relationship_id =
            candidate.add_image_relationship_checked(&part_name, image_data, image_filename)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;

        let drawing = picture_drawing(&relationship_id, drawing_id, &options);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        close_typed_story_drawing_namespaces(&mut paragraph)?;
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        fragment.source_part_name = Some(part_name.clone());
        fragment.source_story_kind = Some(story.kind);
        fragment.source_owner_index = Some(story.owner_index);
        let fragment_xml = content_fragment_for_insertion(&candidate, story, &fragment)?;
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &story_owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        if part_name == candidate.doc_part_name {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        let location = {
            let read = reopened.prepared_main_read()?;
            let related = reopened.related_story_sources()?;
            let refreshed_story = read
                .stories(&related)?
                .into_iter()
                .find(|candidate| {
                    candidate.kind == story.kind
                        && candidate.part_name == story.part_name
                        && candidate.owner_index == story.owner_index
                })
                .ok_or_else(|| Error::Other("inserted picture story was not found".to_owned()))?;
            let (source, owner, _) = read.owner_and_layout(&refreshed_story, false, &related)?;
            let shared_projected = if !holds_field_words(source.xml.as_ref()) {
                Some(scan_story_items(source.xml.as_ref(), &owner, None)?)
            } else {
                None
            };
            let direct = match &shared_projected {
                Some(items) => direct_story_content_items_in(source.xml.as_ref(), &owner, items)?,
                None => direct_story_content_items(source.xml.as_ref(), &owner)?,
            };
            let inserted = direct.get(inserted_direct_index).ok_or_else(|| {
                Error::Other("inserted picture paragraph was not found".to_owned())
            })?;
            let projected = match shared_projected {
                Some(items) => items,
                None => scan_story_items(source.xml.as_ref(), &owner, None)?,
            };
            let index = projected
                .iter()
                .position(|item| item.full == inserted.full)
                .ok_or_else(|| {
                    Error::Other("inserted picture paragraph was not projected".to_owned())
                })?;
            if let Some(inventory) = inventory
                && !holds_field_words(&read.xml)
                && related.iter().all(|source| !holds_field_words(&source.xml))
            {
                *inventory = Some(reopened.pictures_in_prepared(&read, &related)?);
            }
            ContentLocation::new(refreshed_story, StoryItemKind::Paragraph, vec![index])
        };
        Ok((reopened, location))
    }

    /// Append one configured picture paragraph to a checked story owner.
    pub fn add_picture_with_options(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        image_filename: &str,
        options: PictureOptions,
    ) -> Result<()> {
        validate_picture_options(&options)?;
        if oxoffice_media::probe(image_data).is_none() {
            return Err(Error::Other(format!(
                "picture `{image_filename}` is unsupported or malformed"
            )));
        }

        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            candidate.add_image_relationship_checked(&owner, image_data, image_filename)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;
        let drawing = picture_drawing(&relationship_id, drawing_id, &options);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        candidate.append_authored_paragraph_to_story(story, paragraph)?;
        if owner == candidate.doc_part_name && story.kind != StoryKind::Body {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn append_internal_hyperlink_staged(
        &mut self,
        location: &ContentLocation,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        for value in [Some(text), Some(bookmark), tooltip].into_iter().flatten() {
            oxoffice_core::xml::reject_non_xml_characters("internal hyperlink", value)?;
        }
        crate::comments::validate_bookmark_name(bookmark)?;
        let (ranges, across) = self.story_range_inventory()?;
        let matching = ranges
            .iter()
            .map(crate::StoryRangeRef::kind)
            .chain(&across)
            .filter(|kind| {
                matches!(kind, crate::StoryRangeKind::Bookmark { name, .. } if oxoffice_docx_oxml::text::same_bookmark_name(name, bookmark))
            })
            .count();
        if matching != 1 {
            return Err(Error::Other(
                "internal hyperlink bookmark is missing or ambiguous".into(),
            ));
        }
        let story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        let refreshed =
            ContentLocation::new(story, location.item_kind, location.index_path.clone());
        let run_index = self.edit_story_paragraph(&refreshed, |paragraph| {
            let index = paragraph.run_count();
            paragraph.add_internal_hyperlink(text, bookmark, tooltip);
            Ok(index)
        })?;
        let story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        Ok(StoryRunPosition {
            location: ContentLocation::new(story, location.item_kind, location.index_path.clone()),
            run_index,
        })
    }

    /// Add a checked internal link without allocating an external relationship.
    pub fn add_internal_hyperlink_at(
        &mut self,
        location: &ContentLocation,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        self.story_range_paragraph_snapshot(location)?;
        let mut candidate = self.clone_for_staging();
        let result =
            candidate.append_internal_hyperlink_staged(location, text, bookmark, tooltip)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let story = reopened
            .stories()?
            .into_iter()
            .find(|s| {
                s.kind == result.location.story.kind
                    && s.part_name == result.location.story.part_name
                    && s.owner_index == result.location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(StoryRunPosition {
            location: ContentLocation::new(
                story,
                result.location.item_kind,
                result.location.index_path,
            ),
            run_index: result.run_index,
        })
    }

    /// Stage a heading bookmark and source link together, with unique IDs and
    /// one caller publication. Existing complete heading bookmarks are reused.
    pub fn add_heading_hyperlink_at(
        &mut self,
        source: &ContentLocation,
        heading: &ContentLocation,
        text: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        self.story_range_paragraph_snapshot(source)?;
        let item = self.story_range_paragraph_snapshot(heading)?;
        let paragraph = CT_P::from_xml_fragment(&item.xml)?;
        let mut level = paragraph.properties.as_ref().and_then(|p| p.outline_lvl);
        let mut style = paragraph
            .properties
            .as_ref()
            .and_then(|p| p.style_id.as_deref());
        let mut seen = HashSet::new();
        while level.is_none() {
            let Some(id) = style else {
                break;
            };
            if !seen.insert(id) {
                return Err(Error::Other("heading style ancestry is cyclic".into()));
            }
            let Some(value) = self.styles.get_by_id(id) else {
                break;
            };
            level = value.ppr.as_ref().and_then(|p| p.outline_lvl);
            style = value.based_on.as_deref();
        }
        if !level.is_some_and(|level| level < 9)
            && self.detect_heading_level_for_toc(&paragraph).is_none()
        {
            return Err(Error::Other(
                "heading target is not an outline heading".into(),
            ));
        }
        let run_count = paragraph.accepted_run_paths().len();
        let (ranges, across) = self.story_range_inventory()?;
        let existing = ranges.iter().find_map(|entry| {
            let crate::StoryRangeKind::Bookmark { name, .. } = entry.kind() else {
                return None;
            };
            let range = entry.range();
            (crate::comments::validate_bookmark_name(name).is_ok()
                && range.start.location == *heading
                && range.end.location == *heading
                && range.start.run_index == 0
                && range.end.run_index == run_count)
                .then(|| name.clone())
        });
        let mut candidate = self.clone_for_staging();
        let name = match existing {
            Some(name) => name,
            None => {
                let names = ranges
                    .iter()
                    .map(crate::StoryRangeRef::kind)
                    .chain(&across)
                    .filter_map(|kind| match kind {
                        crate::StoryRangeKind::Bookmark { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect::<HashSet<_>>();
                let mut suffix = 1u32;
                let name = loop {
                    let name = format!("Heading_{suffix}");
                    if !names.contains(name.as_str()) {
                        break name;
                    }
                    suffix = suffix
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("heading bookmark names exhausted".into()))?;
                };
                candidate.add_story_bookmark(
                    &name,
                    crate::StoryRunRange {
                        start: crate::StoryRunPosition {
                            location: heading.clone(),
                            run_index: 0,
                        },
                        end: crate::StoryRunPosition {
                            location: heading.clone(),
                            run_index: run_count,
                        },
                    },
                )?;
                name
            }
        };
        let result = candidate.append_internal_hyperlink_staged(source, text, &name, tooltip)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let story = reopened
            .stories()?
            .into_iter()
            .find(|s| {
                s.kind == source.story.kind
                    && s.part_name == source.story.part_name
                    && s.owner_index == source.story.owner_index
            })
            .ok_or_else(|| Error::Other("heading hyperlink source disappeared".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(StoryRunPosition {
            location: ContentLocation::new(story, source.item_kind, source.index_path.clone()),
            run_index: result.run_index,
        })
    }

    /// Append one modeled DrawingML text box with a VML compatibility fallback.
    pub fn add_text_box_to_story(
        &mut self,
        story: &StoryId,
        text: &str,
        options: TextBoxOptions,
    ) -> Result<()> {
        validate_text_box_options(&options)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;
        let alternate = authored_text_box_alternate_content(drawing_id, text, &options)?;
        let drawing = oxoffice_docx_oxml::drawing::parse_alternate_content(&alternate, &[])
            .ok_or_else(|| {
                Error::Other("authored text box did not reopen as DrawingML".to_owned())
            })?;
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            properties: None,
            content: Vec::new(),
            extra_xml: vec![alternate],
            extra_xml_positions: vec![0],
            alt_drawings: vec![drawing],
        });
        candidate.append_authored_paragraph_to_story(story, paragraph)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    pub(super) fn append_authored_paragraph_to_story(
        &mut self,
        story: &StoryId,
        mut paragraph: CT_P,
    ) -> Result<()> {
        self.story_source_and_owner(story)?;
        if story.kind == StoryKind::Body && story.part_name == self.doc_part_name {
            self.document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
        } else {
            close_typed_story_drawing_namespaces(&mut paragraph)?;
            self.append_fragment_to_story(story, ContentFragment::paragraph(paragraph)?)?;
        }
        Ok(())
    }

    /// Append one external hyperlink paragraph to a checked story owner.
    pub fn add_hyperlink_to_story(&mut self, story: &StoryId, text: &str, url: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            candidate.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        let mut paragraph = CT_P::new();
        crate::Paragraph {
            inner: &mut paragraph,
        }
        .add_hyperlink(text, &relationship_id);
        paragraph
            .hyperlinks
            .last_mut()
            .expect("the authored hyperlink was appended")
            .extra_attributes
            .push(("xmlns:r".to_owned(), drawing_ns::R.to_owned()));
        let preserve_typed_body = story.kind == StoryKind::Body && owner == candidate.doc_part_name;
        if preserve_typed_body {
            candidate
                .document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
        } else {
            let fragment = ContentFragment::paragraph(paragraph)?;
            candidate.append_fragment_to_story(story, fragment)?;
        }
        if owner == candidate.doc_part_name {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
        }
        candidate.prepare_staged_package()?;
        let reopened = candidate.clone_for_staging().reopen_prepared_staged()?;
        if candidate.doc_part_name == reopened.doc_part_name
            && candidate.package.get_part(&candidate.doc_part_name)
                == reopened.package.get_part(&reopened.doc_part_name)
            && candidate.root_namespace_declarations == reopened.root_namespace_declarations
            && candidate.body_namespace_declarations == reopened.body_namespace_declarations
            && candidate.body_namespace_bindings == reopened.body_namespace_bindings
        {
            // Keep the authored typed body, but retain the actual validated
            // opening and its package provenance instead of discarding them.
            candidate.package = reopened.package;
            candidate.parsed = reopened.parsed;
            candidate.identifiers.scanned_parts = reopened.identifiers.scanned_parts;
            candidate.install_carried_blocks(reopened.main_part_blocks);
        }
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Add an external hyperlink relationship to a checked story owner.
    pub fn add_hyperlink_relationship_to_story(
        &mut self,
        story: &StoryId,
        url: &str,
    ) -> Result<String> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            candidate.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(relationship_id)
    }

    /// Resolve and validate one internal relationship in a checked story scope.
    pub fn validate_internal_relationship_for_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
        expected_type: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        self.validate_internal_relationship_in_checked_story(story, relationship_id, expected_type)
    }

    // The caller must have checked this owner against the current immutable
    // source. Relationship policy stays shared with the public checked entry.
    fn validate_internal_relationship_in_checked_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
        expected_type: &str,
    ) -> Result<String> {
        let relationships = self
            .package
            .get_part_rels(&story.part_name)
            .ok_or_else(|| {
                Error::Other(format!(
                    "story owner {} has no relationship scope",
                    story.part_name
                ))
            })?;
        let relationship = relationships.get_by_id(relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "story owner {} has no relationship {relationship_id}",
                story.part_name
            ))
        })?;
        if relationship.rel_type != expected_type {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} has type {}, expected {expected_type}",
                relationship.rel_type
            )));
        }
        if !relationship_is_internal(relationship) {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} must be internal"
            )));
        }
        let target = OpcPackage::resolve_rel_target(&story.part_name, &relationship.target);
        if !self.package.contains_part(&target) {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} targets missing part {target}"
            )));
        }
        Ok(target)
    }

    /// Inventory supported pictures once per package source, in story and
    /// physical source order. Only the existing accepted paragraph projection
    /// admits a drawing. Unknown XML and unselected alternatives stay opaque.
    pub fn pictures(&self) -> Result<Vec<PictureOccurrence>> {
        let prepared = self.prepared_main_read()?;
        let related = self.related_story_sources()?;
        self.pictures_in_prepared(&prepared, &related)
    }

    /// The same complete collector over one actual immutable document phase.
    /// Callers must prepare from this document and keep it unchanged throughout.
    fn pictures_in_prepared(
        &self,
        prepared: &PreparedMainRead<'_>,
        related: &[StorySource<'_>],
    ) -> Result<Vec<PictureOccurrence>> {
        debug_assert!(std::ptr::eq(self, prepared.document));
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let sources = prepared.sources(related);
        // Optional, read-only qualification. No child reader or memo filling.
        let plain_proofs = prepared
            .writer
            .as_ref()
            .and_then(|record| record.picture_free_block_ranges(&prepared.xml));
        for source in sources {
            let owners = scan_story_owners(&source.xml, source.root_kind)?
                .into_iter()
                .map(|owner| {
                    let story = StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    };
                    (story, owner)
                })
                .collect::<Vec<_>>();
            // This is optional preparation. A later owner's malformed source
            // must not replace an earlier paragraph's authoritative error.
            let batch = (|| -> Result<_> {
                let spans = story_range_paragraph_spans(&source.xml, &owners)?;
                let scopes = story_namespace_scopes_at(
                    &source.xml,
                    spans.iter().flatten().map(|(_, span)| span.start),
                )?;
                Ok((spans, scopes))
            })()
            .ok();
            for (index, (story, owner)) in owners.iter().enumerate() {
                let spans = match &batch {
                    Some((spans, _)) => std::borrow::Cow::Borrowed(spans[index].as_slice()),
                    None => std::borrow::Cow::Owned(
                        story_range_paragraph_spans(
                            &source.xml,
                            &[(story.clone(), owner.clone())],
                        )?
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>(),
                    ),
                };
                for (location, span) in spans.as_ref() {
                    let scope = match batch
                        .as_ref()
                        .and_then(|(_, scopes)| scopes.get(&span.start))
                    {
                        Some(inherited) => {
                            let mut reader =
                                quick_xml::Reader::from_reader(&source.xml[span.clone()]);
                            match reader.read_event().map_err(|error| {
                                Error::Other(format!("story namespace scope scan failed: {error}"))
                            })? {
                                Event::Start(element) | Event::Empty(element) => {
                                    changed_story_namespace_scope(inherited, &element)?
                                        .unwrap_or_else(|| inherited.clone())
                                }
                                _ => story_namespace_scope_at(&source.xml, span.start)?,
                            }
                        }
                        None => story_namespace_scope_at(&source.xml, span.start)?,
                    };
                    let closed =
                        close_content_fragment_namespaces(&source.xml[span.clone()], &scope)?;
                    let proof = (source.part_name == self.doc_part_name
                        && source.root_kind == StoryKind::Body)
                        .then(|| {
                            plain_proofs
                                .as_ref()
                                .and_then(|ranges| picture_free_span_kind(ranges, owner, span))
                        })
                        .flatten();
                    if proof.is_some() {
                        continue;
                    }
                    let paragraph = CT_P::from_xml_fragment(&closed)?;
                    let accepted = paragraph.accepted_view();
                    let drawings = accepted.runs.iter().flat_map(|run| {
                        run.content
                            .iter()
                            .filter_map(|content| match content {
                                RunContent::Drawing(drawing) => Some(drawing),
                                _ => None,
                            })
                            .chain(run.alt_drawings.iter())
                    });
                    let mut occurrences = Vec::new();
                    let mut claimed = HashSet::new();
                    for drawing in drawings {
                        let (
                            raw,
                            embed,
                            link,
                            chart,
                            _projected_width,
                            _projected_height,
                            name,
                            description,
                            title,
                            decorative,
                            anchored,
                        ) = if let Some(inline) = &drawing.inline {
                            (
                                &inline.raw_xml,
                                &inline.embed_id,
                                &inline.link_id,
                                &inline.chart_rel_id,
                                inline.extent_cx.0,
                                inline.extent_cy.0,
                                &inline.name,
                                &inline.description,
                                &inline.title,
                                inline.decorative,
                                false,
                            )
                        } else if let Some(anchor) = &drawing.anchor {
                            (
                                &anchor.raw_xml,
                                &anchor.embed_id,
                                &anchor.link_id,
                                &anchor.chart_rel_id,
                                anchor.extent_cx.0,
                                anchor.extent_cy.0,
                                &anchor.name,
                                &anchor.description,
                                &anchor.title,
                                anchor.decorative,
                                true,
                            )
                        } else {
                            continue;
                        };
                        if chart.is_some() || (embed.is_empty() && link.is_none()) {
                            continue;
                        }
                        let raw = raw.as_ref().ok_or_else(|| {
                            Error::Other("picture has no physical source identity".into())
                        })?;
                        if raw.is_empty() {
                            return Err(Error::Other("picture source is empty".into()));
                        }
                        let Some((width, height)) = picture_source_extent(raw, &scope, anchored)?
                        else {
                            continue;
                        };
                        let matches = source.xml[span.clone()]
                            .windows(raw.len())
                            .enumerate()
                            .filter(|(_, bytes)| *bytes == raw.as_slice())
                            .map(|(offset, _)| span.start + offset)
                            .collect::<Vec<_>>();
                        // Physical copies that cannot be distinguished by the
                        // admitted projection are not silently reinterpreted.
                        let count = accepted
                            .runs
                            .iter()
                            .flat_map(|run| {
                                run.content
                                    .iter()
                                    .filter_map(|c| match c {
                                        RunContent::Drawing(d) => Some(d),
                                        _ => None,
                                    })
                                    .chain(run.alt_drawings.iter())
                            })
                            .filter(|d| {
                                d.inline.as_ref().and_then(|i| i.raw_xml.as_ref()) == Some(raw)
                                    || d.anchor.as_ref().and_then(|a| a.raw_xml.as_ref())
                                        == Some(raw)
                            })
                            .count();
                        if matches.len() != count {
                            return Err(Error::Other(
                                "picture has ambiguous alternate physical copies".into(),
                            ));
                        }
                        let start = matches
                            .into_iter()
                            .find(|start| !claimed.contains(start))
                            .ok_or_else(|| {
                                Error::Other("picture source correspondence is missing".into())
                            })?;
                        claimed.insert(start);
                        if width <= 0 || height <= 0 {
                            return Err(Error::Other("picture extent must be positive".into()));
                        }
                        let relationship_id = link.as_deref().unwrap_or(embed);
                        let (media_part_name, external_url, bytes, content_type) = if link.is_some()
                        {
                            let relationship = self
                                .package
                                .get_part_rels(&story.part_name)
                                .and_then(|rels| rels.get_by_id(relationship_id))
                                .ok_or_else(|| {
                                    Error::Other("linked picture relationship is missing".into())
                                })?;
                            if relationship.rel_type != rel_types::IMAGE
                                || relationship_is_internal(relationship)
                            {
                                return Err(Error::Other(
                                    "linked picture must have an external image relationship"
                                        .into(),
                                ));
                            }
                            (
                                None,
                                Some(relationship.target.clone()),
                                Vec::new(),
                                String::new(),
                            )
                        } else {
                            // This owner came from the current source above.
                            // The immutable inventory cannot stale it between
                            // drawings, so do not rebuild that source per image.
                            let target = self.validate_internal_relationship_in_checked_story(
                                story,
                                embed,
                                rel_types::IMAGE,
                            )?;
                            let target = self
                                .package
                                .parts
                                .keys()
                                .filter(|name| {
                                    OpcPackage::resolve_rel_target("/", name)
                                        .eq_ignore_ascii_case(&target)
                                })
                                .min()
                                .cloned()
                                .ok_or_else(|| {
                                    Error::Other("picture media identity is missing".into())
                                })?;
                            let bytes = self
                                .package
                                .get_part(&target)
                                .ok_or_else(|| Error::Other("picture media is missing".into()))?
                                .to_vec();
                            let content_type = self
                                .package
                                .content_types
                                .content_type_for(&target)
                                .filter(|kind| kind.starts_with("image/"))
                                .ok_or_else(|| {
                                    Error::Other("picture media content type is unsupported".into())
                                })?
                                .to_owned();
                            (Some(target), None, bytes, content_type)
                        };
                        occurrences.push((
                            start,
                            PictureOccurrence {
                                location: location.clone(),
                                ordinal: 0,
                                relationship_id: relationship_id.to_owned(),
                                media_part_name,
                                external_url,
                                width,
                                height,
                                name: name.clone(),
                                description: description.clone(),
                                title: title.clone(),
                                decorative,
                                anchored,
                                content_type,
                                bytes,
                                source: raw.clone(),
                            },
                        ));
                    }
                    occurrences.sort_by_key(|(start, _)| *start);
                    for (ordinal, (start, mut picture)) in occurrences.into_iter().enumerate() {
                        picture.ordinal = ordinal;
                        if seen.insert((source.part_name.clone(), start)) {
                            result.push(picture);
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    /// Resize one frozen occurrence, preserving shared media and every other
    /// physical drawing. Changed source or unsupported copies refuse atomically.
    pub fn set_picture_size_at(
        &mut self,
        picture: &PictureOccurrence,
        width: Length,
        height: Length,
    ) -> Result<()> {
        if picture.external_url.is_some() {
            return Err(Error::Other(
                "linked picture resizing is unsupported".into(),
            ));
        }
        let (width, height) = (width.to_emu(), height.to_emu());
        if width <= 0 || height <= 0 {
            return Err(Error::Other("picture dimensions must be positive".into()));
        }
        let current = self
            .pictures()?
            .into_iter()
            .find(|current| {
                current.location == picture.location && current.ordinal == picture.ordinal
            })
            .ok_or_else(|| Error::Other("picture occurrence is stale or foreign".into()))?;
        if current != *picture {
            return Err(Error::Other("picture occurrence source changed".into()));
        }
        if (width, height) == (picture.width, picture.height) {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&picture.location.story)?;
        let part_name = source.part_name.clone();
        let spans = story_range_paragraph_spans(
            &source.xml,
            &[(picture.location.story.clone(), owner.clone())],
        )?
        .into_iter()
        .flatten();
        let span = spans
            .into_iter()
            .find(|(location, _)| location == &picture.location)
            .map(|(_, span)| span)
            .ok_or_else(|| Error::Other("picture paragraph disappeared".into()))?;
        let scope = story_namespace_scope_at(&source.xml, span.start)?;
        let paragraph = &source.xml[span.clone()];
        let matches = paragraph
            .windows(picture.source.len())
            .enumerate()
            .filter(|(_, bytes)| *bytes == picture.source.as_slice())
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        let equal_before = candidate
            .pictures()?
            .into_iter()
            .filter(|other| {
                other.location == picture.location
                    && other.ordinal < picture.ordinal
                    && other.source == picture.source
            })
            .count();
        let offset = *matches
            .get(equal_before)
            .ok_or_else(|| Error::Other("picture physical source disappeared".into()))?;
        let closed = close_content_fragment_namespaces(&picture.source, &scope)?;
        let (resized, count) =
            resize_story_pictures(&closed, &picture.relationship_id, width, height)?;
        if count != 1 {
            return Err(Error::Other(
                "picture occurrence has unsupported nested representation".into(),
            ));
        }
        // Namespace closure is projection-only. Numeric edits are transported
        // back onto the exact physical owner without rewriting its bindings.
        let (boundaries, _) = comment_namespace_source_boundaries(&picture.source, &closed)?
            .ok_or_else(|| Error::Other("picture namespace closure changed source".into()))?;
        let mut edits = Vec::new();
        let mut before_reader = NsReader::from_reader(closed.as_slice());
        let mut after_reader = NsReader::from_reader(resized.as_slice());
        loop {
            let start = before_reader.buffer_position() as usize;
            let before = before_reader
                .read_event()
                .map_err(|e| Error::Other(e.to_string()))?;
            let after = after_reader
                .read_event()
                .map_err(|e| Error::Other(e.to_string()))?;
            if before != after {
                let physical_start = boundaries
                    .iter()
                    .find_map(|(raw, closed)| (*closed == start).then_some(*raw))
                    .ok_or_else(|| Error::Other("picture edit has no physical boundary".into()))?;
                let end = before_reader.buffer_position() as usize;
                let physical_end = boundaries
                    .iter()
                    .find_map(|(raw, closed)| (*closed == end).then_some(*raw))
                    .ok_or_else(|| Error::Other("picture edit has no physical boundary".into()))?;
                // Only numeric values change, the original element spelling
                // and all other attributes remain byte-exact.
                let Event::Empty(ref before_tag) = before else {
                    return Err(Error::Other("picture extent source is unsupported".into()));
                };
                let Event::Empty(ref after_tag) = after else {
                    return Err(Error::Other("picture extent source is unsupported".into()));
                };
                let mut tag = picture.source[physical_start..physical_end].to_vec();
                let mut values = Vec::new();
                for (a, b) in before_tag.attributes().zip(after_tag.attributes()) {
                    let (a, b) = (
                        a.map_err(|e| Error::Other(e.to_string()))?,
                        b.map_err(|e| Error::Other(e.to_string()))?,
                    );
                    if a.key != b.key {
                        return Err(Error::Other(
                            "picture resize changed attribute ownership".into(),
                        ));
                    }
                    if a.value != b.value {
                        let (a_start, a_end) = story_attribute_value_span(&tag, a.key.as_ref())
                            .ok_or_else(|| {
                                Error::Other("picture extent attribute missing".into())
                            })?;
                        values.push((a_start..a_end, b.value.to_vec()));
                    }
                }
                values.sort_by_key(|(range, _)| range.start);
                for (range, value) in values.into_iter().rev() {
                    tag.splice(range, value);
                }
                edits.push((physical_start..physical_end, tag));
            }
            if matches!(before, Event::Eof) {
                break;
            }
        }
        let mut raw = picture.source.clone();
        for (range, value) in edits.into_iter().rev() {
            raw.splice(range, value);
        }
        let mut updated = source.xml.into_owned();
        updated.splice(
            span.start + offset..span.start + offset + picture.source.len(),
            raw,
        );
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Fetch image bytes through a checked story-local relationship.
    pub fn image_data_for_story(&self, story: &StoryId, relationship_id: &str) -> Result<Vec<u8>> {
        let target = self.validate_internal_relationship_for_story(
            story,
            relationship_id,
            rel_types::IMAGE,
        )?;
        self.package
            .get_part(&target)
            .map(ToOwned::to_owned)
            .ok_or_else(|| Error::Other(format!("image relationship target {target} is missing")))
    }

    /// Replace a picture through a checked story-local relationship.
    pub fn replace_image_for_story(
        &mut self,
        story: &StoryId,
        relationship_id: &str,
        image_data: &[u8],
    ) -> Result<()> {
        self.validate_internal_relationship_for_story(story, relationship_id, rel_types::IMAGE)?;
        self.replace_image_relationship(&story.part_name, relationship_id, image_data)
    }

    pub(super) fn replace_image_relationship(
        &mut self,
        owner: &str,
        relationship_id: &str,
        image_data: &[u8],
    ) -> Result<()> {
        let format = oxoffice_media::ImageFormat::sniff(image_data).ok_or_else(|| {
            Error::Other("replacement bytes are not a supported image".to_owned())
        })?;
        let old_target = self
            .package
            .get_part_rels(owner)
            .and_then(|relationships| relationships.get_by_id(relationship_id))
            .filter(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && relationship_is_internal(relationship)
            })
            .map(|relationship| OpcPackage::resolve_rel_target(owner, &relationship.target))
            .ok_or_else(|| {
                Error::Other(format!(
                    "relationship owner {owner} has no internal image relationship {relationship_id}"
                ))
            })?;
        if !self.package.contains_part(&old_target) {
            return Err(Error::Other(format!(
                "image relationship target {old_target} is missing"
            )));
        }

        let mut candidate = self.clone_for_staging();
        let reference_count = std::iter::once(("/", &candidate.package.package_rels))
            .chain(
                candidate
                    .package
                    .part_rels
                    .iter()
                    .map(|(source, relationships)| (source.as_str(), relationships)),
            )
            .flat_map(|(source, relationships)| {
                relationships
                    .items
                    .iter()
                    .map(move |relationship| (source, relationship))
            })
            .filter(|(source, relationship)| {
                relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(source, &relationship.target) == old_target
            })
            .count();
        let compatible_extension = old_target
            .rsplit_once('.')
            .and_then(|(_, extension)| oxoffice_media::ImageFormat::from_extension(extension))
            == Some(format);
        let new_target = if reference_count == 1 && compatible_extension {
            old_target.clone()
        } else {
            candidate
                .identifiers
                .reserve_part_name("/word/media", "image", format.extension())?
        };

        if new_target != old_target {
            let relationship = candidate
                .package
                .get_part_rels_mut(owner)
                .and_then(|relationships| {
                    relationships
                        .items
                        .iter_mut()
                        .find(|relationship| relationship.id == relationship_id)
                })
                .ok_or_else(|| {
                    Error::Other(format!(
                        "image relationship {relationship_id} disappeared during replacement"
                    ))
                })?;
            relationship.target = relative_target(owner, &new_target);
        }
        candidate.install_reserved_image_part(&new_target, image_data, format);

        if new_target != old_target {
            let old_target_is_referenced = std::iter::once(("/", &candidate.package.package_rels))
                .chain(
                    candidate
                        .package
                        .part_rels
                        .iter()
                        .map(|(source, relationships)| (source.as_str(), relationships)),
                )
                .any(|(source, relationships)| {
                    relationships.items.iter().any(|relationship| {
                        relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(source, &relationship.target)
                                == old_target
                    })
                });
            if !old_target_is_referenced {
                candidate.package.remove_part(&old_target);
                candidate.package.remove_part_rels(&old_target);
                candidate.package.content_types.remove_override(&old_target);
                candidate.identifiers.retire_authored_part(&old_target);
                if let Some((_, extension)) = old_target.rsplit_once('.') {
                    let extension = extension.to_ascii_lowercase();
                    let default_is_used = candidate.package.parts.keys().any(|part_name| {
                        !candidate.package.content_types.contains_override(part_name)
                            && part_name.rsplit_once('.').is_some_and(|(_, candidate)| {
                                candidate.eq_ignore_ascii_case(&extension)
                            })
                    });
                    if !default_is_used
                        && candidate
                            .identifiers
                            .authored_content_type_defaults
                            .remove(&extension)
                    {
                        candidate.package.content_types.remove_default(&extension);
                        candidate
                            .identifiers
                            .content_type_defaults
                            .remove(&extension);
                    }
                }
            }
        }
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Resolve an external hyperlink through a checked story-local relationship.
    pub fn hyperlink_url_for_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        self.hyperlink_url_for_validated_story(story, relationship_id)
    }

    pub(super) fn hyperlink_url_for_validated_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
    ) -> Result<String> {
        let relationships = self
            .package
            .get_part_rels(&story.part_name)
            .ok_or_else(|| {
                Error::Other(format!(
                    "story owner {} has no relationship scope",
                    story.part_name
                ))
            })?;
        let relationship = relationships.get_by_id(relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "story owner {} has no relationship {relationship_id}",
                story.part_name
            ))
        })?;
        if relationship.rel_type != rel_types::HYPERLINK
            || relationship.target_mode.as_deref() != Some("External")
        {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} is not an external hyperlink"
            )));
        }
        Ok(relationship.target.clone())
    }
}
