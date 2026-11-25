//! Pictures of a direct body paragraph added and resized in place, read from
//! that paragraph's block alone, and the pictures of an image relationship
//! resized in the direct body blocks that show it.
//!
//! See `specs/2026-11-24-staged-edits-in-place.md`, S3b.

use super::story_pictures::{
    checked_run_picture_options, pictures_before_run, resized_picture_source,
};
use super::*;

/// The most direct body blocks a relationship's pictures are resized in,
/// in place. A relationship shown in more takes the staged route, which
/// writes the main part once.
const MAX_PICTURE_BLOCKS_IN_PLACE: usize = 16;

/// `location` without its story's fingerprint: an in-place route names its
/// story without listing the stories, which reads the whole main part.
fn unfingerprinted(location: &ContentLocation) -> ContentLocation {
    let story = location.story();
    ContentLocation::new(
        StoryId::new(story.kind(), story.part_name(), story.owner_index()),
        location.item_kind(),
        location.index_path().to_vec(),
    )
}

/// `picture` with its location's story unfingerprinted, so that a picture
/// read from one block compares with one read from the whole part.
fn unfingerprinted_picture(picture: &PictureOccurrence) -> PictureOccurrence {
    PictureOccurrence {
        location: unfingerprinted(&picture.location),
        ..picture.clone()
    }
}

impl Document {
    /// The pictures of the direct body paragraph that `location` names and
    /// `block` holds, read from that block written alone, numbered as
    /// [`Self::pictures`] numbers them, their story unfingerprinted.
    /// `None` when `location` is not such a paragraph. The caller asks
    /// first whether a save of the body is known to fail
    /// ([`Self::body_block_item`]).
    fn block_paragraph_pictures(
        &self,
        location: &ContentLocation,
        block: crate::ControlBlock,
    ) -> Result<Option<(BodyBlockItem, Vec<PictureOccurrence>)>> {
        if self.direct_body_paragraph_ref(location, block).is_none() {
            return Ok(None);
        }
        let Some(item) = self.framed_body_block_item(
            block.slot,
            (StoryKind::Body, 0),
            0,
            StoryItemKind::Paragraph,
        )?
        else {
            return Ok(None);
        };
        let scope = story_namespace_scope_at(&item.xml, item.item.start)?;
        let location = unfingerprinted(location);
        let pictures = self
            .paragraph_picture_occurrences(
                location.story(),
                &location,
                &item.xml,
                item.item.clone(),
                &scope,
            )?
            .into_iter()
            .map(|(_, picture)| picture)
            .collect();
        Ok(Some((item, pictures)))
    }

    /// [`Self::insert_picture_with_options_at`] on a direct body paragraph of
    /// the main part (`position.location`, held by `block`, which the caller
    /// found), in place under one `BodyBlocks` unit: the media part, its
    /// content type, the image relationship and the drawing id are reserved
    /// at insert, and the picture is read from that block alone, its story
    /// unfingerprinted. `None`, changing nothing, for any other location or
    /// a body whose save is known to fail: the caller takes the staged
    /// route.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)] // The operands of the staged route.
    pub fn insert_picture_in_block(
        &mut self,
        position: &StoryRunPosition,
        block: crate::ControlBlock,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<Option<PictureOccurrence>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.insert_picture_in_block_unchecked(
            position,
            block,
            image_data,
            image_filename,
            width,
            height,
            options.clone(),
        );
        #[cfg(debug_assertions)]
        match &result {
            Ok(None) => assert!(
                self.clone_for_staging().to_bytes().ok()
                    == before.clone_for_staging().to_bytes().ok(),
                "a picture insertion left to the staged route changed the document"
            ),
            _ => self.assert_in_place_matches_staged_declarations_aside(
                before,
                &result
                    .as_ref()
                    .map(|picture| picture.as_ref().map(unfingerprinted_picture))
                    .map_err(|error| Error::Other(error.to_string())),
                |staged| {
                    let position = StoryRunPosition {
                        location: staged.fingerprinted(&position.location),
                        run_index: position.run_index,
                    };
                    staged
                        .insert_picture_with_options_at(
                            &position,
                            image_data,
                            image_filename,
                            width,
                            height,
                            options,
                        )
                        .map(|picture| Some(unfingerprinted_picture(&picture)))
                },
            ),
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_picture_in_block_unchecked(
        &mut self,
        position: &StoryRunPosition,
        block: crate::ControlBlock,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<Option<PictureOccurrence>> {
        let Some(current) = self.direct_body_paragraph_ref(&position.location, block) else {
            return Ok(None);
        };
        if self.unsavable_body_edit().is_some()
            || self.block_holds_nested_namespace_owner(&self.document.body.content[block.slot])?
        {
            return Ok(None);
        }
        // The staged route's checks, in its order.
        let options =
            checked_run_picture_options(image_data, image_filename, width, height, options)?;
        let ordinal = pictures_before_run(current, position.run_index)?;
        let mut paragraph = current.clone();
        let slot = block.slot;
        let savable_before = self.unsavable_body_edit_cache.get() == Some(&None);
        let part_name = self.doc_part_name.clone();
        let picture =
            self.edit_in_place(&[EditUnit::BodyBlocks { slot, count: 1 }], |document| {
                let relationship = document.add_image_relationship_checked(
                    &part_name,
                    image_data,
                    image_filename,
                )?;
                let drawing = picture_drawing(
                    &relationship,
                    document.identifiers.reserve_drawing_id()?,
                    &options,
                );
                let mut holder = CT_P::new();
                holder.runs.push(CT_R {
                    alt_drawings: Vec::new(),
                    properties: None,
                    content: vec![RunContent::Drawing(drawing)],
                    extra_xml: Vec::new(),
                    extra_xml_positions: Vec::new(),
                });
                close_typed_story_drawing_namespaces(&mut holder)?;
                paragraph
                    .insert_accepted_run(position.run_index, holder.runs.remove(0))
                    .map_err(|error| Error::Other(format!("field insertion boundary: {error}")))?;
                document.document.body.content[slot] = BodyContent::Paragraph(Box::new(paragraph));
                document.invalidate_layout();
                // Read inside the edit, so that a picture that does not read back
                // undoes it.
                document
                    .block_paragraph_pictures(&position.location, block)?
                    .and_then(|(_, pictures)| {
                        pictures
                            .into_iter()
                            .find(|picture| picture.ordinal == ordinal)
                    })
                    .ok_or_else(|| {
                        Error::Other("inserted picture occurrence was not projected".into())
                    })
            })?;
        if self.take_last_edit_changed() {
            self.keep_body_savable_after_typed_insert(slot, savable_before);
        }
        Ok(Some(picture))
    }

    /// [`Self::set_picture_size_at`] on a picture of a direct body paragraph
    /// of the main part (held by `block`, which the caller found), in place
    /// under one `BodyBlocks` unit: the picture is found in that block alone
    /// and its extents written as the staged route writes them. The caller
    /// vouches that `picture` was read from the document as it is now (the
    /// binding's revision check): its story's fingerprint, which only a read
    /// of the whole main part gives, is not compared. `Some(())` when made
    /// (or when the size is already that one); `None`, changing nothing,
    /// for any other picture, a body whose save is known to fail, or a block
    /// that does not read back as it writes: the caller takes the staged
    /// route.
    #[doc(hidden)]
    pub fn set_picture_size_in_block(
        &mut self,
        picture: &PictureOccurrence,
        block: crate::ControlBlock,
        width: Length,
        height: Length,
    ) -> Result<Option<()>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.set_picture_size_in_block_unchecked(picture, block, width, height);
        #[cfg(debug_assertions)]
        match &result {
            Ok(None) => assert!(
                self.clone_for_staging().to_bytes().ok()
                    == before.clone_for_staging().to_bytes().ok(),
                "a picture resize left to the staged route changed the document"
            ),
            _ => self.assert_in_place_matches_staged(before, &result, |staged| {
                // The staged route takes the fingerprinted occurrence of the
                // document as it is.
                let wanted = unfingerprinted_picture(picture);
                let current = staged
                    .pictures()?
                    .into_iter()
                    .find(|current| unfingerprinted_picture(current) == wanted)
                    .unwrap_or_else(|| picture.clone());
                staged
                    .set_picture_size_at(&current, width, height)
                    .map(Some)
            }),
        }
        result
    }

    fn set_picture_size_in_block_unchecked(
        &mut self,
        picture: &PictureOccurrence,
        block: crate::ControlBlock,
        width: Length,
        height: Length,
    ) -> Result<Option<()>> {
        if self
            .direct_body_paragraph_ref(&picture.location, block)
            .is_none()
        {
            return Ok(None);
        }
        // The staged route's checks, in its order.
        if picture.external_url.is_some() {
            return Err(Error::Other(
                "linked picture resizing is unsupported".into(),
            ));
        }
        let (width, height) = (width.to_emu(), height.to_emu());
        if width <= 0 || height <= 0 {
            return Err(Error::Other("picture dimensions must be positive".into()));
        }
        let current = &self.document.body.content[block.slot];
        if self.unsavable_body_edit().is_some()
            || current.holds_unallocated_drawing_id()
            || self.block_holds_nested_namespace_owner(current)?
        {
            return Ok(None);
        }
        let Some((item, pictures)) = self.block_paragraph_pictures(&picture.location, block)?
        else {
            return Ok(None);
        };
        // A picture whose paragraph changed since it was read: the staged
        // route finds the story's fingerprint changed first, and answers
        // that the occurrence is stale.
        let wanted = unfingerprinted_picture(picture);
        if !pictures.contains(&wanted) {
            return Err(Error::Other(
                "picture occurrence is stale or foreign".into(),
            ));
        }
        if (width, height) == (picture.width, picture.height) {
            return Ok(Some(()));
        }
        // The physical copy of the source, counted among the equal sources
        // before it in the paragraph, as the staged route finds it.
        let equal_before = pictures
            .iter()
            .filter(|other| other.ordinal < picture.ordinal && other.source == picture.source)
            .count();
        let span = item.item.clone();
        let offset = item.xml[span.clone()]
            .windows(picture.source.len())
            .enumerate()
            .filter(|(_, bytes)| *bytes == picture.source.as_slice())
            .map(|(offset, _)| offset)
            .nth(equal_before)
            .ok_or_else(|| Error::Other("picture physical source disappeared".into()))?;
        let scope = story_namespace_scope_at(&item.xml, span.start)?;
        let raw = resized_picture_source(picture, &scope, width, height)?;
        let mut updated = item.xml.clone();
        let start = span.start + offset;
        updated.splice(start..start + picture.source.len(), raw);
        let Some(resized) =
            Self::reparse_body_block(&item.head, item.tail_bytes(), item.block_of(&updated))
        else {
            return Ok(None);
        };
        self.replace_body_block_in_place(block.slot, resized)?;
        Ok(Some(()))
    }

    /// [`Self::set_picture_size`] in the direct body blocks that show
    /// `rel_id`, in place under one `BodyBlocks` unit each, when at most
    /// [`MAX_PICTURE_BLOCKS_IN_PLACE`] blocks may show it: the blocks are
    /// found by a walk of the typed body (a drawing naming the relationship,
    /// or a piece kept as raw XML holding its bytes,
    /// [`BodyContent::may_show_relationship`]), each is written alone and
    /// resized as the staged route resizes the whole part. `None`, changing
    /// nothing, for a relationship the page background names, more blocks,
    /// a body whose save is known to fail, or a block that does not read
    /// back as it writes or reads back with other runs. The caller checked
    /// the size and the relationship.
    pub(super) fn set_picture_size_in_blocks(
        &mut self,
        rel_id: &str,
        cx: i64,
        cy: i64,
    ) -> Result<Option<usize>> {
        let needle = rel_id.as_bytes();
        if self
            .document
            .background_xml
            .as_deref()
            .is_some_and(|xml| xml.windows(needle.len()).any(|window| window == needle))
            || self
                .document
                .background_extra_xml
                .iter()
                .any(|xml| xml.windows(needle.len()).any(|window| window == needle))
        {
            return Ok(None);
        }
        let mut slots = Vec::new();
        for (slot, block) in self.document.body.content.iter().enumerate() {
            if block.may_show_relationship(rel_id) {
                // A block read back from its bytes takes the drawing id its
                // placeholder was written with, which the registry does not
                // hold: the staged route reads the whole part again.
                if slots.len() == MAX_PICTURE_BLOCKS_IN_PLACE
                    || block.holds_unallocated_drawing_id()
                {
                    return Ok(None);
                }
                slots.push(slot);
            }
        }
        if !slots.is_empty() && self.unsavable_body_edit().is_some() {
            return Ok(None);
        }
        for slot in &slots {
            if self.block_holds_nested_namespace_owner(&self.document.body.content[*slot])? {
                return Ok(None);
            }
        }
        let mut resized_blocks = Vec::new();
        let mut resized = 0;
        for slot in slots {
            let Some(part) = self.framed_body_block(slot)? else {
                return Ok(None);
            };
            let (updated, count) = resize_story_pictures(&part.xml, rel_id, cx, cy)?;
            if count == 0 {
                continue;
            }
            let Some(block) = Self::reparse_body_block(
                &part.head,
                &updated[updated.len() - part.tail..],
                part.block_of(&updated),
            ) else {
                return Ok(None);
            };
            // Nothing is published: a block that reads back with other
            // runs (a run edited live that a read splits otherwise) would
            // give a kept handle another owner. The staged route retires
            // them all.
            if block.owner_shape() != self.document.body.content[slot].owner_shape() {
                return Ok(None);
            }
            resized += count;
            resized_blocks.push((slot, block));
        }
        if resized == 0 {
            return Err(Error::Other(format!(
                "no picture drawing shows image relationship {rel_id}"
            )));
        }
        let units = resized_blocks
            .iter()
            .map(|(slot, _)| EditUnit::BodyBlocks {
                slot: *slot,
                count: 1,
            })
            .collect::<Vec<_>>();
        // An extent edit writes the elements the blocks had: the body saves
        // as it did before.
        let savable = self.unsavable_body_edit_cache.get().cloned();
        self.edit_in_place(&units, |document| {
            for (slot, block) in resized_blocks {
                document.document.body.content[slot] = block;
            }
            document.invalidate_layout();
            Ok(())
        })?;
        if self.take_last_edit_changed()
            && let Some(savable) = savable
        {
            self.unsavable_body_edit_cache = std::sync::OnceLock::from(savable);
        }
        Ok(Some(resized))
    }
}

#[cfg(test)]
mod tests {
    use super::super::headers_footers::tests::PNG;
    use super::*;

    fn body_paragraph(document: &Document, index: usize) -> ContentLocation {
        ContentLocation::new(
            StoryId::new(StoryKind::Body, document.doc_part_name.clone(), 0),
            StoryItemKind::Paragraph,
            vec![index],
        )
    }

    fn block(slot: usize) -> crate::ControlBlock {
        crate::ControlBlock {
            slot,
            owner_index: 0,
            item_index: 0,
        }
    }

    #[test]
    fn a_run_picture_added_in_its_block_is_the_one_the_document_lists() {
        let mut document = Document::new();
        document.add_paragraph("first");
        document.add_paragraph("alpha ").add_run("beta");
        let position = StoryRunPosition {
            location: body_paragraph(&document, 1),
            run_index: 1,
        };
        let picture = document
            .insert_picture_in_block(
                &position,
                block(1),
                PNG,
                "one.png",
                Some(Length::emu(9_525)),
                Some(Length::emu(19_050)),
                None,
            )
            .unwrap()
            .expect("a direct body paragraph takes the in-place route");
        let listed = document.pictures().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(unfingerprinted_picture(&listed[0]), picture);
        assert_eq!((picture.width, picture.height), (9_525, 19_050));
        let mut resized = document.clone_for_staging();
        assert_eq!(
            resized
                .set_picture_size_in_blocks(&picture.relationship_id, 28_575, 9_525)
                .unwrap(),
            Some(1)
        );
        assert_eq!(resized.pictures().unwrap()[0].width, 28_575);
        // A boundary past the runs is refused before anything is reserved.
        let before = document.to_bytes().unwrap();
        let past = StoryRunPosition {
            location: body_paragraph(&document, 1),
            run_index: 9,
        };
        assert!(
            document
                .insert_picture_in_block(&past, block(1), PNG, "two.png", None, None, None)
                .is_err()
        );
        assert_eq!(document.to_bytes().unwrap(), before);
        // The picture resizes in its block, and the next read sees it.
        assert_eq!(
            document
                .set_picture_size_in_block(
                    &picture,
                    block(1),
                    Length::emu(19_050),
                    Length::emu(9_525)
                )
                .unwrap(),
            Some(())
        );
        let resized = &document.pictures().unwrap()[0];
        assert_eq!((resized.width, resized.height), (19_050, 9_525));
        // The handle read before holds the old size: it is refused.
        assert!(
            document
                .set_picture_size_in_block(&picture, block(1), Length::emu(1), Length::emu(1))
                .unwrap_err()
                .to_string()
                .contains("stale or foreign")
        );
    }

    #[test]
    fn a_paragraph_that_is_not_a_direct_body_paragraph_is_left_to_the_staged_route() {
        let mut document = Document::new();
        document.add_paragraph("alpha");
        document.add_table(1, 1);
        let position = StoryRunPosition {
            location: body_paragraph(&document, 1),
            run_index: 0,
        };
        let before = document.to_bytes().unwrap();
        assert_eq!(
            document
                .insert_picture_in_block(&position, block(1), PNG, "one.png", None, None, None)
                .unwrap(),
            None
        );
        assert_eq!(document.to_bytes().unwrap(), before);
    }

    /// `count` paragraphs that each show one image relationship.
    fn shared_pictures(count: usize) -> (Document, String) {
        let mut document = Document::new();
        let relationship = document.embed_image(PNG, "shared.png");
        for _ in 0..count {
            document.add_paragraph("shown ").add_run("").add_picture(
                &relationship,
                Length::emu(9_525),
                Length::emu(9_525),
            );
        }
        document.add_paragraph("last");
        // A save gives the pictures their drawing ids.
        let document = Document::from_bytes(&document.to_bytes().unwrap()).unwrap();
        (document, relationship)
    }

    #[test]
    fn a_relationship_shown_in_more_blocks_than_the_bound_takes_the_staged_route() {
        for count in [MAX_PICTURE_BLOCKS_IN_PLACE, MAX_PICTURE_BLOCKS_IN_PLACE + 1] {
            let (mut document, relationship) = shared_pictures(count);
            let mut staged = document.clone_for_staging();
            let in_place = document
                .set_picture_size_in_blocks(&relationship, 19_050, 19_050)
                .unwrap();
            assert_eq!(
                in_place,
                (count <= MAX_PICTURE_BLOCKS_IN_PLACE).then_some(count)
            );
            if in_place.is_none() {
                continue;
            }
            assert_eq!(
                staged
                    .set_picture_size_staged(&relationship, 19_050, 19_050)
                    .unwrap(),
                count
            );
            assert_eq!(document.to_bytes().unwrap(), staged.to_bytes().unwrap());
        }
        // The public call answers the same on either route.
        let (mut document, relationship) = shared_pictures(MAX_PICTURE_BLOCKS_IN_PLACE + 1);
        assert_eq!(
            document
                .set_picture_size(&relationship, Length::emu(1), Length::emu(2))
                .unwrap(),
            MAX_PICTURE_BLOCKS_IN_PLACE + 1
        );
    }

    #[test]
    fn a_drawing_with_no_source_yet_is_found_by_the_typed_walk() {
        let mut document = Document::new();
        document.add_paragraph("after");
        document.add_anchored_image(
            PNG,
            "typed.png",
            Length::emu(9_525),
            Length::emu(9_525),
            false,
        );
        let BodyContent::Paragraph(paragraph) = &document.document.body.content[0] else {
            unreachable!("the anchored image's paragraph comes first");
        };
        let RunContent::Drawing(drawing) = &paragraph.runs[0].content[0] else {
            unreachable!("the paragraph holds the drawing");
        };
        let relationship = drawing.anchor.as_ref().unwrap().embed_id.clone();
        let block = &document.document.body.content[0];
        assert!(!block.raw_xml_contains(relationship.as_bytes()));
        assert!(block.may_show_relationship(&relationship));
        assert!(!document.document.body.content[1].may_show_relationship(&relationship));
        assert_eq!(
            document
                .set_picture_size(&relationship, Length::emu(19_050), Length::emu(9_525))
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_drawing_waiting_for_its_id_takes_the_staged_route() {
        let mut document = Document::new();
        let relationship = document.embed_image(PNG, "typed.png");
        document.add_paragraph("typed ").add_run("").add_picture(
            &relationship,
            Length::emu(9_525),
            Length::emu(9_525),
        );
        let before = document.to_bytes().unwrap();
        assert_eq!(
            document
                .set_picture_size_in_blocks(&relationship, 19_050, 9_525)
                .unwrap(),
            None
        );
        assert_eq!(document.to_bytes().unwrap(), before);
    }

    #[test]
    fn a_relationship_the_page_background_names_takes_the_staged_route() {
        let (mut document, relationship) = shared_pictures(1);
        document.document.background_xml = Some(
            format!(
                r#"<w:background xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{relationship}"/>"#
            )
            .into_bytes(),
        );
        assert_eq!(
            document
                .set_picture_size_in_blocks(&relationship, 19_050, 19_050)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_relationship_no_picture_shows_is_refused_as_the_staged_route_refuses_it() {
        let mut document = Document::new();
        document.add_paragraph("alpha");
        let relationship = document.embed_image(PNG, "unused.png");
        let before = document.to_bytes().unwrap();
        let error = document
            .set_picture_size(&relationship, Length::emu(1), Length::emu(1))
            .unwrap_err();
        assert!(error.to_string().contains("no picture drawing shows"));
        assert_eq!(document.to_bytes().unwrap(), before);
    }
}
