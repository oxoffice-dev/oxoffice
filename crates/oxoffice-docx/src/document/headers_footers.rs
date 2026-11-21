//! Headers, footers and watermarks: their parts, images and inheritance.

use super::*;

impl Document {
    // ---- Header/Footer ----

    /// Set the default header text.
    ///
    /// Replaces the header the last section shows. When it inherits that
    /// header from an earlier section, the shared header takes the text, so
    /// every section showing it changes, as Word does for a header linked to
    /// the previous one. When the last section has a header of its own, only
    /// that one changes. When no section has one, the first section gets it
    /// and every later section inherits it. The other document-level header
    /// and footer setters, text, image or raw XML, choose the same way, except
    /// that a new first-page or even header goes on the last section.
    pub fn set_header(&mut self, text: &str) {
        self.try_set_header(text)
            .expect("an in-memory document can install a header");
    }

    /// Replace the header atomically, refusing unsafe comment removal.
    pub fn try_set_header(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate.set_header_footer_part(text, true, HdrFtrType::Default)?;
        candidate.reconcile_comment_removal(self)?;
        candidate.clone_for_staging().prepare_and_reopen_staged()?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Set the default footer text, as [`Self::set_header`] sets the header.
    pub fn set_footer(&mut self, text: &str) {
        self.try_set_footer(text)
            .expect("an in-memory document can install a footer");
    }

    /// Replace the footer atomically, refusing unsafe comment removal.
    pub fn try_set_footer(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate.set_header_footer_part(text, false, HdrFtrType::Default)?;
        candidate.reconcile_comment_removal(self)?;
        candidate.clone_for_staging().prepare_and_reopen_staged()?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Set the first-page header text, and give the last section a different
    /// first page.
    ///
    /// The header is chosen as [`Self::set_header`] chooses it. A first-page
    /// header the last section inherits is shared only when the last section
    /// already showed it, with a different first page; otherwise the last
    /// section gets one of its own.
    pub fn set_first_page_header(&mut self, text: &str) {
        self.try_set_first_page_header(text)
            .expect("an in-memory document can install a first page header");
    }

    /// Replace the first page header atomically, refusing unsafe comment removal.
    pub fn try_set_first_page_header(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate.set_header_footer_part(text, true, HdrFtrType::First)?;
        candidate.set_different_first_page(true);
        candidate.reconcile_comment_removal(self)?;
        candidate.clone_for_staging().prepare_and_reopen_staged()?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Set the first-page footer text.
    pub fn set_first_page_footer(&mut self, text: &str) {
        self.try_set_first_page_footer(text)
            .expect("an in-memory document can install a first page footer");
    }

    /// Replace the first page footer atomically, refusing unsafe comment removal.
    pub fn try_set_first_page_footer(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate.set_header_footer_part(text, false, HdrFtrType::First)?;
        candidate.set_different_first_page(true);
        candidate.reconcile_comment_removal(self)?;
        candidate.clone_for_staging().prepare_and_reopen_staged()?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Get the default header text the last section shows, its own or the
    /// one it inherits from an earlier section, if any.
    pub fn header_text(&self) -> Option<String> {
        self.get_header_footer_text(true, HdrFtrType::Default)
    }

    /// Get the default footer text the last section shows, its own or the
    /// one it inherits from an earlier section, if any.
    pub fn footer_text(&self) -> Option<String> {
        self.get_header_footer_text(false, HdrFtrType::Default)
    }

    /// Whether the document references any header or footer part.
    ///
    /// This is broader than [`Self::header_text`] and [`Self::footer_text`]:
    /// a referenced part can contain drawings, fields, tables, or other visible
    /// content without contributing literal text.
    pub fn has_header_footer_content(&self) -> bool {
        !self.header_footer_rel_ids().is_empty()
    }

    /// Set the default header to an inline image.
    ///
    /// Creates a header part with an image paragraph. The image is embedded
    /// in the header part's relationships.
    pub fn set_header_image(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_header_footer_image_part(
                image_data,
                image_filename,
                width,
                height,
                true,
                HdrFtrType::Default,
            )
            .expect("an in-memory document can install a header image");
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Set a Word-compatible text watermark in every active header variant.
    pub fn set_text_watermark(&mut self, text: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.apply_watermark(|_, _, _| {
            Ok((
                VmlWatermark::Text {
                    text: text.to_owned(),
                    width_pt: 468.0,
                    height_pt: 117.0,
                    rotation_degrees: 315.0,
                    color: "D9D9D9".to_owned(),
                    font_family: Some("Calibri".to_owned()),
                    opacity: 0.5,
                },
                None,
            ))
        })?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Set the API-owned text watermark for one explicit section header variant.
    pub fn set_text_watermark_for(
        &mut self,
        section_index: usize,
        hdr_type: HdrFtrType,
        text: &str,
        options: TextWatermarkOptions,
    ) -> Result<()> {
        validate_text_watermark_options(&options)?;
        let mut candidate = self.clone_for_staging();
        let story =
            candidate.create_section_story(section_index, HeaderFooterKind::Header, hdr_type)?;
        let part_name = story.part_name.clone();
        let xml = candidate
            .package
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("header part {part_name} is missing")))?
            .to_vec();
        let updated = replace_authored_watermark(
            &xml,
            &VmlWatermark::Text {
                text: text.to_owned(),
                width_pt: options.width.to_pt(),
                height_pt: options.height.to_pt(),
                rotation_degrees: options.rotation_degrees,
                color: options.color,
                font_family: options.font_family,
                opacity: options.opacity,
            },
        )?;
        candidate.package.set_part(&part_name, updated);
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Set real DrawingML watermark text in one explicit header variant.
    /// Only unchanged API-owned DrawingML is replaced. Opaque edits refuse atomically.
    pub fn set_drawingml_text_watermark_for(
        &mut self,
        section_index: usize,
        hdr_type: HdrFtrType,
        text: &str,
        options: TextWatermarkOptions,
        font_size: Length,
    ) -> Result<()> {
        validate_text_watermark_options(&options)?;
        if options.opacity != 0.5 {
            return Err(Error::Other(
                "DrawingML text watermark opacity other than 0.5 is unsupported by the qualified native text-fill representation".into(),
            ));
        }
        let font_emu = font_size.to_emu();
        if font_emu <= 0 || font_emu % 6350 != 0 || font_emu / 6350 > 3276 {
            return Err(Error::Other(
                "watermark font size requires positive half-points up to 1638 points".into(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        let story =
            match candidate.section_story(section_index, HeaderFooterKind::Header, hdr_type)? {
                Some(story) => story.story,
                None => candidate.create_section_story(
                    section_index,
                    HeaderFooterKind::Header,
                    hdr_type,
                )?,
            };
        candidate.flush_to_package()?;
        let original = candidate
            .package
            .get_part(&story.part_name)
            .ok_or_else(|| Error::Other("watermark header source is missing".into()))?
            .to_vec();
        let owned = drawingml_watermark_span(&original)?;
        let drawing_id = match &owned {
            Some((_, id)) => *id,
            None => candidate.identifiers.reserve_drawing_id()?,
        };
        let box_options = TextBoxOptions {
            width: options.width,
            height: options.height,
            anchor: PictureAnchor {
                horizontal_relative_from: DrawingHorizontalRelativeFrom::Page,
                horizontal_offset: Length::emu(0),
                horizontal_alignment: Some(DrawingHorizontalAlignment::Center),
                vertical_relative_from: DrawingVerticalRelativeFrom::Page,
                vertical_offset: Length::emu(0),
                vertical_alignment: Some(DrawingVerticalAlignment::Center),
                wrap: DrawingWrap::None,
                distance_top: Length::emu(0),
                distance_bottom: Length::emu(0),
                distance_left: Length::emu(0),
                distance_right: Length::emu(0),
                relative_height: 0,
                behind_text: true,
            },
            rotation_degrees: options.rotation_degrees,
            text_direction: TextBoxDirection::Horizontal,
            fill_color: None,
        };
        let mut alternate = String::from_utf8(authored_text_box_alternate_content(
            drawing_id,
            text,
            &box_options,
        )?)
        .map_err(|error| Error::Other(error.to_string()))?;
        // Plain text warp keeps the watermark as real DrawingML shape text.
        // It also lets importers preserve the shape rotation instead of
        // projecting this owner as an unrotated Writer text frame.
        alternate = alternate.replace(
            "<wps:bodyPr vert=\"horz\"/>",
            "<wps:bodyPr vert=\"horz\"><a:prstTxWarp prst=\"textPlain\"><a:avLst/></a:prstTxWarp></wps:bodyPr>",
        );
        let family = options.font_family.as_deref().unwrap_or("Calibri");
        let family = quick_xml::escape::escape(family);
        let color = &options.color;
        let alpha = (options.opacity * 100_000.0).round() as u32;
        let size = font_emu / 6350;
        let properties = format!(
            "<w:rPr><w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" w:eastAsia=\"{family}\" w:cs=\"{family}\"/><w:color w:val=\"{color}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/><w14:textFill xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"><w14:solidFill><w14:srgbClr w14:val=\"{color}\"><w14:alpha w14:val=\"{alpha}\"/></w14:srgbClr></w14:solidFill></w14:textFill></w:rPr>"
        );
        if alternate.matches("<w:r>").count() != 2 {
            return Err(Error::Other(
                "watermark requires both real text representations".into(),
            ));
        }
        alternate = alternate.replace("<w:r>", &format!("<w:r>{properties}"));
        alternate = alternate.replace(
            &format!("name=\"Text Box {drawing_id}\""),
            "name=\"RDOCX DrawingML Watermark\"",
        );
        // Word preserves solid text-fill opacity, while the supported WPS
        // importer preserves the equivalent constant-gradient opacity. Keep
        // both real text owners under their actual declared capability choices.
        let choice_start = alternate
            .find("<mc:Choice Requires=\"wps\">")
            .ok_or_else(|| Error::Other("watermark capability choice is missing".into()))?;
        let choice_end = alternate[choice_start..]
            .find("</mc:Choice>")
            .map(|offset| choice_start + offset + "</mc:Choice>".len())
            .ok_or_else(|| Error::Other("watermark capability choice is incomplete".into()))?;
        let original_choice = &alternate[choice_start..choice_end];
        let word_choice = original_choice.replacen("Requires=\"wps\"", "Requires=\"wps w16\" xmlns:w16=\"http://schemas.microsoft.com/office/word/2018/wordml\"", 1);
        alternate.replace_range(choice_start..choice_end, &word_choice);
        let mut fallback = String::from_utf8(
            VmlWatermark::Text {
                text: text.to_owned(),
                width_pt: options.width.to_pt(),
                height_pt: options.height.to_pt(),
                rotation_degrees: options.rotation_degrees,
                color: options.color.clone(),
                font_family: options.font_family.clone(),
                opacity: options.opacity,
            }
            .to_pict_xml(),
        )
        .map_err(|_| Error::Other("watermark textpath source is not UTF-8".into()))?;
        // Word's authored type 136 uses this formula-driven straight text path.
        // Keep the legacy VML authoring writer unchanged and adapt only this fallback.
        let shape_type_start = fallback
            .find("<v:shapetype ")
            .ok_or_else(|| Error::Other("watermark textpath shape type is missing".into()))?;
        let shape_type_end = fallback[shape_type_start..]
            .find("</v:shapetype>")
            .map(|offset| shape_type_start + offset + "</v:shapetype>".len())
            .ok_or_else(|| Error::Other("watermark textpath shape type is incomplete".into()))?;
        let shape_type = concat!(
            "<v:shapetype id=\"rdocx-watermark-type-text\" coordsize=\"21600,21600\" o:spt=\"136\" adj=\"10800\" path=\"m@7,l@8,m@5,21600l@6,21600e\">",
            "<v:formulas><v:f eqn=\"sum #0 0 10800\"/><v:f eqn=\"prod #0 2 1\"/><v:f eqn=\"sum 21600 0 @1\"/><v:f eqn=\"sum 0 0 @2\"/><v:f eqn=\"sum 21600 0 @3\"/><v:f eqn=\"if @0 @3 0\"/><v:f eqn=\"if @0 21600 @1\"/><v:f eqn=\"if @0 0 @2\"/><v:f eqn=\"if @0 @4 21600\"/><v:f eqn=\"mid @5 @6\"/><v:f eqn=\"mid @8 @5\"/><v:f eqn=\"mid @7 @8\"/><v:f eqn=\"mid @6 @7\"/><v:f eqn=\"sum @6 0 @5\"/></v:formulas>",
            "<v:path textpathok=\"t\" o:connecttype=\"custom\" o:connectlocs=\"@9,0;@10,10800;@11,21600;@12,10800\" o:connectangles=\"270,180,90,0\"/><v:textpath on=\"t\" fitshape=\"t\"/><v:handles><v:h position=\"#0,bottomRight\" xrange=\"6629,14971\"/></v:handles><o:lock v:ext=\"edit\" text=\"t\" shapetype=\"t\"/></v:shapetype>"
        );
        fallback.replace_range(shape_type_start..shape_type_end, shape_type);
        fallback = fallback
            .replace(
                "font-size:1pt",
                &format!("font-size:{}pt", font_size.to_pt()),
            )
            .replace("relative:margin", "relative:page")
            .replace(
                &format!("fillcolor=\"{color}\""),
                &format!("fillcolor=\"#{color}\""),
            )
            .replace(
                "id=\"rdocx-watermark\"",
                &format!("id=\"rdocx-watermark-{drawing_id}\""),
            )
            .replace(
                "rdocx-watermark-type-text",
                &format!("rdocx-watermark-type-text-{drawing_id}"),
            );
        let fallback_start = alternate
            .find("<mc:Fallback>")
            .ok_or_else(|| Error::Other("watermark fallback source is missing".into()))?;
        let fallback_end = alternate[fallback_start..]
            .find("</mc:Fallback>")
            .map(|offset| fallback_start + offset + "</mc:Fallback>".len())
            .ok_or_else(|| Error::Other("watermark fallback source is incomplete".into()))?;
        alternate.replace_range(
            fallback_start..fallback_end,
            &format!("<mc:Fallback>{fallback}</mc:Fallback>"),
        );
        let checksum = structural_fingerprint(alternate.as_bytes())?;
        alternate = alternate.replace(
            "name=\"RDOCX DrawingML Watermark\"",
            &format!(
                "name=\"RDOCX DrawingML Watermark\" descr=\"rdocx-watermark-text-v1:{checksum}\""
            ),
        );
        let alternate = alternate.into_bytes();
        // Validate the actual style projection before publishing either representation.
        let drawing = oxoffice_docx_oxml::drawing::parse_alternate_content(&alternate, &[])
            .ok_or_else(|| {
                Error::Other("watermark DrawingML representation did not reopen".into())
            })?;
        if let Some((span, _)) = owned {
            let mut updated = original.clone();
            updated.splice(span.clone(), alternate);
            candidate
                .package
                .set_part(&story.part_name, updated.clone());
            candidate.prune_replaced_watermark_media(
                &story.part_name,
                &original[span],
                &updated,
            )?;
        } else {
            let mut paragraph = CT_P::new();
            paragraph.runs.push(CT_R {
                properties: None,
                content: Vec::new(),
                extra_xml: vec![alternate],
                extra_xml_positions: vec![0],
                alt_drawings: vec![drawing],
            });
            candidate.append_authored_paragraph_to_story(&story, paragraph)?;
        }
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Set a page-centered DrawingML image in the selected actual header owner.
    pub fn set_drawingml_image_watermark_for(
        &mut self,
        section_index: usize,
        hdr_type: HdrFtrType,
        image_data: &[u8],
        image_filename: &str,
        options: ImageWatermarkOptions,
    ) -> Result<()> {
        if options.width.to_emu() <= 0
            || options.height.to_emu() <= 0
            || !options.rotation_degrees.is_finite()
            || !options.opacity.is_finite()
            || !(0.0..=1.0).contains(&options.opacity)
        {
            return Err(Error::Other(
                "watermark image geometry or opacity is invalid".into(),
            ));
        }
        if oxoffice_media::probe(image_data).is_none() {
            return Err(Error::Other("watermark image bytes are unsupported".into()));
        }
        let mut candidate = self.clone_for_staging();
        let story =
            match candidate.section_story(section_index, HeaderFooterKind::Header, hdr_type)? {
                Some(story) => story.story,
                None => candidate.create_section_story(
                    section_index,
                    HeaderFooterKind::Header,
                    hdr_type,
                )?,
            };
        candidate.flush_to_package()?;
        let original = candidate
            .package
            .get_part(&story.part_name)
            .ok_or_else(|| Error::Other("watermark header source is missing".into()))?
            .to_vec();
        let owned = drawingml_watermark_span(&original)?;
        let id = match &owned {
            Some((_, id)) => *id,
            None => candidate.identifiers.reserve_drawing_id()?,
        };
        // Retire only the already qualified API owner before allocating its
        // replacement. Otherwise final relationship canonicalization can reuse
        // the retired ID after the new owner's fingerprint has been written.
        if let Some((span, _)) = &owned {
            let mut without_owner = original.clone();
            without_owner.drain(span.clone());
            candidate
                .package
                .set_part(&story.part_name, without_owner.clone());
            candidate.prune_replaced_watermark_media(
                &story.part_name,
                &original[span.clone()],
                &without_owner,
            )?;
        }
        let relationship = candidate.add_image_relationship_checked(
            &story.part_name,
            image_data,
            image_filename,
        )?;
        let mut anchor = CT_Anchor::background(
            &relationship,
            options.width.to_emu(),
            options.height.to_emu(),
        );
        anchor.doc_pr_id = id;
        anchor.name = Some("RDOCX DrawingML Watermark".into());
        anchor.description = None;
        anchor.pos_h_align = Some(oxoffice_docx_oxml::drawing::AnchorAlignH::Center);
        anchor.pos_v_align = Some(oxoffice_docx_oxml::drawing::AnchorAlignV::Center);
        let mut writer = Writer::new(Vec::new());
        CT_Drawing::anchor(anchor).to_xml(&mut writer)?;
        let mut drawing =
            String::from_utf8(writer.into_inner()).map_err(|e| Error::Other(e.to_string()))?;
        drawing = drawing.replacen("<w:drawing>", "<w:drawing xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">", 1);
        let blip = format!("<a:blip r:embed=\"{relationship}\"/>");
        let alpha = (options.opacity * 100_000.0).round() as u32;
        if !drawing.contains(&blip) || !drawing.contains("<a:xfrm>") {
            return Err(Error::Other(
                "watermark image writer shape is unsupported".into(),
            ));
        }
        drawing = drawing.replacen(
            &blip,
            &format!(
                "<a:blip r:embed=\"{relationship}\"><a:alphaModFix amt=\"{alpha}\"/></a:blip>"
            ),
            1,
        );
        let rotation = (options.rotation_degrees.rem_euclid(360.0) * 60_000.0).round() as i32;
        drawing = drawing.replacen("<a:xfrm>", &format!("<a:xfrm rot=\"{rotation}\">"), 1);
        let checksum = structural_fingerprint(drawing.as_bytes())?;
        drawing = drawing.replacen(
            "name=\"RDOCX DrawingML Watermark\"",
            &format!(
                "name=\"RDOCX DrawingML Watermark\" descr=\"rdocx-watermark-image-v1:{checksum}\""
            ),
            1,
        );
        if let Some((span, _)) = owned {
            let mut updated = original.clone();
            updated.splice(span.clone(), drawing.into_bytes());
            candidate
                .package
                .set_part(&story.part_name, updated.clone());
        } else {
            let mut paragraph = CT_P::new();
            paragraph.runs.push(CT_R {
                properties: None,
                content: Vec::new(),
                extra_xml: vec![drawing.into_bytes()],
                extra_xml_positions: vec![0],
                alt_drawings: Vec::new(),
            });
            candidate.append_authored_paragraph_to_story(&story, paragraph)?;
        }
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove only a source-qualified API watermark, keeping its surrounding content.
    pub fn remove_drawingml_watermark_for(
        &mut self,
        section_index: usize,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        let Some(story) = self.section_story(section_index, HeaderFooterKind::Header, hdr_type)?
        else {
            return Ok(());
        };
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let original = candidate
            .package
            .get_part(&story.story.part_name)
            .ok_or_else(|| Error::Other("watermark header source is missing".into()))?
            .to_vec();
        let Some((span, _)) = drawingml_watermark_span(&original)? else {
            return Ok(());
        };
        let mut updated = original.clone();
        updated.drain(span.clone());
        candidate
            .package
            .set_part(&story.story.part_name, updated.clone());
        candidate.prune_replaced_watermark_media(
            &story.story.part_name,
            &original[span],
            &updated,
        )?;
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn prune_replaced_watermark_media(
        &mut self,
        part: &str,
        removed: &[u8],
        updated: &[u8],
    ) -> Result<()> {
        let removed_ids = xml_relationship_ids_in_order(removed)?;
        let referenced = xml_relationship_ids_in_order(updated)?
            .into_iter()
            .collect::<HashSet<_>>();
        for id in removed_ids {
            if referenced.contains(&id) {
                continue;
            }
            let image_part = self
                .package
                .get_part_rels(part)
                .and_then(|rels| {
                    rels.items.iter().find(|rel| {
                        rel.id == id
                            && rel.rel_type == rel_types::IMAGE
                            && relationship_is_internal(rel)
                    })
                })
                .map(|rel| OpcPackage::resolve_rel_target(part, &rel.target));
            if let Some(preserved) = self
                .identifiers
                .preserved_relationship_ids
                .get_mut(&relationship_owner_identity(part))
            {
                preserved.remove(&id);
            }
            if let Some(image_part) = image_part {
                self.identifiers
                    .preserved_part_names
                    .remove(&part_name_identity(&image_part));
            }
        }
        self.prune_authored_story_relationships(part, &referenced);
        Ok(())
    }

    /// Set an image watermark in every active header variant.
    pub fn set_image_watermark(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) -> Result<()> {
        if width.to_emu() <= 0 || height.to_emu() <= 0 {
            return Err(Error::Other(
                "watermark image width and height must be positive".to_owned(),
            ));
        }

        let mut candidate = self.clone_for_staging();
        let image_target = candidate.store_image_part(image_data, image_filename);
        let image_part_name =
            OpcPackage::resolve_rel_target(&candidate.doc_part_name, &image_target);
        candidate.apply_watermark(|package, identifiers, part_name| {
            let target = relative_target(part_name, &image_part_name);
            let relationship_id = identifiers
                .reserve_relationship_id_checked(part_name)
                .map_err(|error| {
                    Error::Other(format!(
                        "watermark relationship allocation failed for {part_name}: {error}"
                    ))
                })?;
            package.get_or_create_part_rels(part_name).add_with_id(
                &relationship_id,
                rel_types::IMAGE,
                &target,
            );
            Ok((
                VmlWatermark::Image {
                    relationship_id: relationship_id.clone(),
                    width_pt: width.to_pt(),
                    height_pt: height.to_pt(),
                    rotation_degrees: 0.0,
                    opacity: 0.5,
                },
                Some(relationship_id),
            ))
        })?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    fn apply_watermark(
        &mut self,
        mut watermark_for_part: impl FnMut(
            &mut OpcPackage,
            &mut DocumentIdentifiers,
            &str,
        ) -> Result<(VmlWatermark, Option<String>)>,
    ) -> Result<()> {
        self.ensure_watermark_header_inheritance()?;
        let relationships = self.package.get_part_rels(&self.doc_part_name);
        let header_ids = self
            .header_footer_rel_ids()
            .into_iter()
            .filter_map(|(relationship_id, is_header)| {
                (is_header
                    && relationships.is_some_and(|relationships| {
                        relationships
                            .get_by_id(&relationship_id)
                            .is_some_and(|relationship| {
                                relationship.rel_type == rel_types::HEADER
                                    && relationship_is_internal(relationship)
                            })
                    }))
                .then_some(relationship_id)
            })
            .collect::<Vec<_>>();

        for relationship_id in header_ids {
            let target = self
                .package
                .get_part_rels(&self.doc_part_name)
                .and_then(|relationships| relationships.get_by_id(&relationship_id))
                .filter(|relationship| relationship_is_internal(relationship))
                .map(|relationship| relationship.target.clone())
                .ok_or_else(|| {
                    Error::Other(format!(
                        "active header relationship {relationship_id} is missing"
                    ))
                })?;
            let part_name = OpcPackage::resolve_rel_target(&self.doc_part_name, &target);
            let xml = self
                .package
                .get_part(&part_name)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| {
                    Error::Other(format!("active header part {part_name} is missing"))
                })?;
            let fully_authored = self.identifiers.authored_story_parts.contains(&part_name);
            let (watermark, authored_relationship) =
                watermark_for_part(&mut self.package, &mut self.identifiers, &part_name)?;
            let updated = replace_authored_watermark(&xml, &watermark)?;
            let referenced = xml_relationship_ids_in_order(&updated)?
                .into_iter()
                .collect::<HashSet<_>>();
            self.prune_authored_story_relationships(&part_name, &referenced);
            self.package.set_part(&part_name, updated);
            if !fully_authored {
                let authored = self
                    .identifiers
                    .authored_story_relationship_ids
                    .entry(relationship_owner_identity(&part_name))
                    .or_default();
                authored.retain(|id| referenced.contains(id));
                if let Some(id) = authored_relationship {
                    authored.insert(id);
                }
                if authored.is_empty() {
                    self.identifiers
                        .authored_story_relationship_ids
                        .remove(&relationship_owner_identity(&part_name));
                }
            }
        }
        self.invalidate_layout();
        Ok(())
    }

    fn ensure_watermark_header_inheritance(&mut self) -> Result<()> {
        self.section_properties_mut();
        let even_enabled = self.even_headers_enabled();
        let internal_header_ids = self
            .package
            .get_part_rels(&self.doc_part_name)
            .map(|relationships| {
                relationships
                    .items
                    .iter()
                    .filter(|relationship| {
                        relationship.rel_type == rel_types::HEADER
                            && relationship_is_internal(relationship)
                    })
                    .map(|relationship| relationship.id.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let insertions = {
            let mut effective = [false; 3];
            let mut insertions = Vec::new();
            let mut inspect = |location: Option<usize>, section: &CT_SectPr| {
                for reference in &section.header_refs {
                    if internal_header_ids.contains(&reference.rel_id) {
                        effective[header_type_index(reference.hdr_ftr_type)] = true;
                    }
                }
                for hdr_type in [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even] {
                    let active = match hdr_type {
                        HdrFtrType::Default => true,
                        HdrFtrType::First => section.title_pg.unwrap_or(false),
                        HdrFtrType::Even => even_enabled,
                    };
                    let index = header_type_index(hdr_type);
                    if active && !effective[index] {
                        insertions.push((location, hdr_type));
                        effective[index] = true;
                    }
                }
            };
            for (index, content) in self.document.body.content.iter().enumerate() {
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some(section) = paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.sect_pr.as_ref())
                {
                    inspect(Some(index), section);
                }
            }
            if let Some(section) = self.document.body.sect_pr.as_ref() {
                inspect(None, section);
            }
            insertions
        };

        for (location, hdr_type) in insertions {
            let relationship_id = self.create_watermark_header_relationship(hdr_type)?;
            let reference = HdrFtrRef {
                hdr_ftr_type: hdr_type,
                rel_id: relationship_id,
            };
            match location {
                Some(index) => {
                    let BodyContent::Paragraph(paragraph) = &mut self.document.body.content[index]
                    else {
                        unreachable!("recorded section owner changed")
                    };
                    let references = &mut paragraph
                        .properties
                        .as_mut()
                        .and_then(|properties| properties.sect_pr.as_mut())
                        .expect("recorded section disappeared")
                        .header_refs;
                    references.insert(
                        story_reference_slot(references, reference.hdr_ftr_type),
                        reference,
                    );
                }
                None => {
                    let references = &mut self
                        .document
                        .body
                        .sect_pr
                        .as_mut()
                        .expect("final section disappeared")
                        .header_refs;
                    references.insert(
                        story_reference_slot(references, reference.hdr_ftr_type),
                        reference,
                    );
                }
            }
        }
        Ok(())
    }

    fn create_watermark_header_relationship(&mut self, hdr_type: HdrFtrType) -> Result<String> {
        let part_name = self.identifiers.reserve_part_name(
            "/word",
            &format!(
                "headerWatermark{}",
                match hdr_type {
                    HdrFtrType::Default => "Default",
                    HdrFtrType::First => "First",
                    HdrFtrType::Even => "Even",
                }
            ),
            "xml",
        )?;
        let empty_header = CT_HdrFtr::new().to_xml_header()?;
        self.package.set_part(&part_name, empty_header);
        self.package.content_types.add_override(
            &part_name,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
        );
        self.identifiers.register_content_type_override(&part_name);
        self.identifiers
            .authored_story_parts
            .insert(part_name.clone());
        let target = relative_target(&self.doc_part_name, &part_name);
        let owner = self.doc_part_name.clone();
        let id = self
            .identifiers
            .reserve_relationship_id_checked(&owner)
            .map_err(|error| {
                Error::Other(format!(
                    "watermark header relationship allocation failed: {error}"
                ))
            })?;
        self.package
            .get_or_create_part_rels(&owner)
            .add_with_id(&id, rel_types::HEADER, &target);
        Ok(id)
    }

    /// Set the default footer to an inline image.
    pub fn set_footer_image(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_header_footer_image_part(
                image_data,
                image_filename,
                width,
                height,
                false,
                HdrFtrType::Default,
            )
            .expect("an in-memory document can install a footer image");
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Set a header from raw XML bytes with associated images.
    ///
    /// This is useful for copying complex headers from template documents
    /// that contain grouped shapes, VML, or other elements not easily
    /// recreated through the high-level API.
    ///
    /// Each entry in `images` is `(rel_id, image_data, image_filename)`:
    /// - `rel_id`: the relationship ID referenced in the header XML (e.g. "rId1")
    /// - `image_data`: the raw image bytes
    /// - `image_filename`: used to derive the part name and content type (e.g. "image5.png")
    pub fn set_raw_header_with_images(
        &mut self,
        header_xml: Vec<u8>,
        images: &[(&str, &[u8], &str)],
        hdr_type: HdrFtrType,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_raw_hdr_ftr_with_images(header_xml, images, true, hdr_type)
            .expect("raw header package preflight failed");
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Set a footer from raw XML bytes with associated images.
    pub fn set_raw_footer_with_images(
        &mut self,
        footer_xml: Vec<u8>,
        images: &[(&str, &[u8], &str)],
        hdr_type: HdrFtrType,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_raw_hdr_ftr_with_images(footer_xml, images, false, hdr_type)
            .expect("raw footer package preflight failed");
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Set the default header to an inline image with a colored background.
    ///
    /// Creates a header part where the paragraph has shading fill set to
    /// `bg_color` (hex string, e.g. "000000" for black) and contains the
    /// inline image.
    pub fn set_header_image_with_background(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
        bg_color: &str,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_header_footer_image_bg_part(
                image_data,
                image_filename,
                width,
                height,
                Some(bg_color),
                true,
                HdrFtrType::Default,
            )
            .expect("an in-memory document can install a header background image");
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Set the first-page header to an inline image.
    pub fn set_first_page_header_image(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        candidate
            .set_header_footer_image_part(
                image_data,
                image_filename,
                width,
                height,
                true,
                HdrFtrType::First,
            )
            .expect("an in-memory document can install a first-page header image");
        candidate.set_different_first_page(true);
        candidate
            .reconcile_comment_removal(self)
            .expect("whole-story replacement cannot remove partial or malformed comment ownership");
        candidate
            .clone_for_staging()
            .prepare_and_reopen_staged()
            .expect("whole-story replacement package preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Where a header/footer of this kind lives, and how to declare it.
    ///
    /// All four public entry points differ only in what goes *inside* the part;
    /// the surrounding bookkeeping — part name, content type, relationship,
    /// section reference — is identical, and lives here.
    ///
    /// Note the fixed `1` in the part name: oxoffice manages one header and one
    /// footer per [`HdrFtrType`] for the document's single section. Setting a
    /// header of the same type again replaces the existing part.
    fn hdr_ftr_slot_metadata(
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> (String, &'static str, &'static str) {
        let type_suffix = match hdr_type {
            HdrFtrType::Default => "",
            HdrFtrType::First => "First",
            HdrFtrType::Even => "Even",
        };
        if is_header {
            (
                format!("/word/header{type_suffix}1.xml"),
                rel_types::HEADER,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
            )
        } else {
            (
                format!("/word/footer{type_suffix}1.xml"),
                rel_types::FOOTER,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
            )
        }
    }

    /// Install a header/footer part: store its bytes, declare the content type,
    /// relate it to the document, and point the section properties at it.
    ///
    /// Any previous reference of the same [`HdrFtrType`] is replaced.
    fn reserve_hdr_ftr_part_name(
        &mut self,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<String> {
        self.identifiers.observe_package_graph(&self.package)?;
        let (preferred, expected_rel_type, _) = Self::hdr_ftr_slot_metadata(is_header, hdr_type);
        let reference = self.document.body.sect_pr.as_ref().and_then(|section| {
            let references = if is_header {
                &section.header_refs
            } else {
                &section.footer_refs
            };
            references
                .iter()
                .find(|reference| reference.hdr_ftr_type == hdr_type)
        });
        if let Some(part_name) = reference
            .and_then(|reference| {
                self.package
                    .get_part_rels(&self.doc_part_name)
                    .and_then(|relationships| relationships.get_by_id(&reference.rel_id))
            })
            .filter(|relationship| {
                relationship.rel_type == expected_rel_type && relationship_is_internal(relationship)
            })
            .map(|relationship| {
                OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
            })
        {
            return Ok(part_name);
        }
        self.identifiers.reserve_preferred_part_name(&preferred)
    }

    fn install_hdr_ftr_part(
        &mut self,
        part_name: String,
        section: usize,
        xml: Vec<u8>,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<String> {
        let (_, rel_type, content_type) = Self::hdr_ftr_slot_metadata(is_header, hdr_type);

        // Setting the same header twice must not leave the first relationship
        // behind pointing at the same part.
        let rel_target = relative_target(&self.doc_part_name, &part_name);
        let existing = self
            .package
            .get_part_rels(&self.doc_part_name)
            .and_then(|rels| {
                rels.items
                    .iter()
                    .find(|relationship| {
                        relationship.rel_type == rel_type
                            && relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(
                                &self.doc_part_name,
                                &relationship.target,
                            ) == part_name
                    })
                    .map(|relationship| relationship.id.clone())
            });
        let pending_relationship = match existing {
            Some(existing) => (existing, false),
            None => (
                self.identifiers
                    .reserve_relationship_id_checked(&self.doc_part_name)
                    .map_err(|error| {
                        Error::Other(format!(
                            "header or footer relationship allocation failed: {error}"
                        ))
                    })?,
                true,
            ),
        };

        self.package.set_part(&part_name, xml);
        self.identifiers
            .authored_story_parts
            .insert(part_name.clone());
        self.package
            .content_types
            .add_override(&part_name, content_type);
        self.identifiers.register_content_type_override(&part_name);
        let (rel_id, install_relationship) = pending_relationship;
        if install_relationship {
            self.package
                .get_or_create_part_rels(&self.doc_part_name)
                .add_with_id(&rel_id, rel_type, &rel_target);
        }

        let sect = if section + 1 < self.section_count() {
            self.section_mut(section)
                .expect("the target section exists")
                .inner
        } else {
            self.section_properties_mut()
        };
        let refs = if is_header {
            &mut sect.header_refs
        } else {
            &mut sect.footer_refs
        };
        place_story_reference(
            refs,
            HdrFtrRef {
                hdr_ftr_type: hdr_type,
                rel_id,
            },
        );

        Ok(part_name)
    }

    pub(super) fn clear_authored_story_relationships(&mut self, part_name: &str) {
        self.prune_authored_story_relationships(part_name, &HashSet::new());
    }

    fn prune_authored_story_relationships(
        &mut self,
        part_name: &str,
        referenced: &HashSet<String>,
    ) {
        let preserved = self
            .identifiers
            .preserved_relationship_ids
            .get(&relationship_owner_identity(part_name))
            .cloned()
            .unwrap_or_default();
        let Some(relationships) = self.package.get_part_rels_mut(part_name) else {
            return;
        };
        let mut removed = Vec::new();
        relationships.items.retain(|relationship| {
            if preserved.contains(&relationship.id) || referenced.contains(&relationship.id) {
                true
            } else {
                removed.push(relationship.clone());
                false
            }
        });
        let relationships_are_empty = relationships.items.is_empty();
        if relationships_are_empty {
            self.package.remove_part_rels(part_name);
        }
        self.identifiers.retire_authored_story_relationships(
            part_name,
            removed.iter().map(|relationship| relationship.id.clone()),
        );
        self.remove_unreachable_image_parts(part_name, &removed);
    }

    /// Drop image relationships of `owner` that the removed content named and
    /// that no remaining attribute names. A part the owner did not name before
    /// the edit stays, as does every non-image relationship.
    pub(super) fn release_unreferenced_images(
        &mut self,
        owner: &str,
        referenced_before: &HashSet<String>,
        referenced_after: &HashSet<String>,
    ) {
        let Some(relationships) = self.package.get_part_rels_mut(owner) else {
            return;
        };
        let mut removed = Vec::new();
        relationships.items.retain(|relationship| {
            let released = relationship.rel_type == rel_types::IMAGE
                && relationship_is_internal(relationship)
                && referenced_before.contains(&relationship.id)
                && !referenced_after.contains(&relationship.id);
            if released {
                removed.push(relationship.clone());
            }
            !released
        });
        if removed.is_empty() {
            return;
        }
        if relationships.items.is_empty() {
            self.package.remove_part_rels(owner);
        }
        if let Some(preserved) = self
            .identifiers
            .preserved_relationship_ids
            .get_mut(&relationship_owner_identity(owner))
        {
            for relationship in &removed {
                preserved.remove(&relationship.id);
            }
        }
        self.identifiers.retire_authored_story_relationships(
            owner,
            removed.iter().map(|relationship| relationship.id.clone()),
        );
        for relationship in &removed {
            self.identifiers
                .preserved_part_names
                .remove(&part_name_identity(&OpcPackage::resolve_rel_target(
                    owner,
                    &relationship.target,
                )));
        }
        self.remove_unreachable_image_parts(owner, &removed);
    }

    /// Remove internal image targets that no remaining relationship reaches.
    fn remove_unreachable_image_parts(
        &mut self,
        part_name: &str,
        removed: &[oxoffice_opc::relationship::Relationship],
    ) {
        let mut image_parts = removed
            .iter()
            .filter(|relationship| {
                relationship.rel_type == rel_types::IMAGE && relationship_is_internal(relationship)
            })
            .map(|relationship| OpcPackage::resolve_rel_target(part_name, &relationship.target))
            .collect::<HashSet<_>>();
        image_parts.retain(|candidate| {
            !self.package.part_rels.iter().any(|(owner, relationships)| {
                relationships.items.iter().any(|relationship| {
                    relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(owner, &relationship.target) == *candidate
                })
            })
        });
        for image_part in image_parts {
            if self.identifiers.part_is_preserved(&image_part) {
                continue;
            }
            self.package.remove_part(&image_part);
            self.package.remove_part_rels(&image_part);
            self.package.content_types.remove_override(&image_part);
            self.identifiers.retire_authored_part(&image_part);
            let Some((_, extension)) = image_part.rsplit_once('.') else {
                continue;
            };
            let default_is_used = self.package.parts.keys().any(|part_name| {
                !self.package.content_types.contains_override(part_name)
                    && part_name
                        .rsplit_once('.')
                        .is_some_and(|(_, candidate)| candidate == extension)
            });
            if !default_is_used
                && self
                    .identifiers
                    .authored_content_type_defaults
                    .remove(extension)
            {
                self.package.content_types.remove_default(extension);
                self.identifiers.content_type_defaults.remove(extension);
            }
        }
    }

    /// Serialize a header/footer body, choosing the right root element.
    pub(super) fn serialize_hdr_ftr(hdr_ftr: &CT_HdrFtr, is_header: bool) -> Result<Vec<u8>> {
        let xml = if is_header {
            hdr_ftr.to_xml_header()
        } else {
            hdr_ftr.to_xml_footer()
        };
        Ok(xml?)
    }

    fn set_header_footer_part(
        &mut self,
        text: &str,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        let mut hdr_ftr = CT_HdrFtr::new();
        let mut p = CT_P::new();
        if !text.is_empty() {
            p.add_run(text);
        }
        hdr_ftr.paragraphs.push(p);

        let xml = Self::serialize_hdr_ftr(&hdr_ftr, is_header)?;
        let (part_name, section) = self.reserve_hdr_ftr_target(is_header, hdr_type)?;
        self.clear_authored_story_relationships(&part_name);
        self.install_hdr_ftr_target(part_name, section, xml, is_header, hdr_type)?;
        Ok(())
    }

    /// Where a document-level header or footer setter writes: the part, and
    /// the section whose reference to it is installed, or `None` when the
    /// last section shows that part by inheritance and keeps inheriting.
    ///
    /// The last section's own reference, usable or not, keeps its part. When
    /// it has none and shows the kind (always for the default one, with a
    /// different first page for the first-page one, with even and odd
    /// headers for the even one), the part it inherits from an earlier
    /// section takes the content, so every section showing it changes, as
    /// Word and python-docx do for a header linked to the previous one. When
    /// no section has a default header, a new one goes on the first section
    /// and every later section inherits it; a new first-page or even header
    /// goes on the last section.
    fn reserve_hdr_ftr_target(
        &mut self,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<(String, Option<usize>)> {
        let refs_of = |section: &CT_SectPr| {
            if is_header {
                section.header_refs.clone()
            } else {
                section.footer_refs.clone()
            }
        };
        let last = self.section_count().saturating_sub(1);
        let last_section = self.document.body.sect_pr.as_ref();
        let has_own = last_section.is_none_or(|section| {
            refs_of(section)
                .iter()
                .any(|reference| reference.hdr_ftr_type == hdr_type)
        });
        if has_own || last == 0 {
            return Ok((
                self.reserve_hdr_ftr_part_name(is_header, hdr_type)?,
                Some(last),
            ));
        }
        let shows = match hdr_type {
            HdrFtrType::Default => true,
            HdrFtrType::First => last_section.is_some_and(|section| section.title_pg == Some(true)),
            HdrFtrType::Even => self.even_and_odd_headers(),
        };
        let source = self
            .section_properties_in_order()
            .take(last)
            .filter_map(|section| {
                self.first_usable_header_footer_reference(&refs_of(section), hdr_type, is_header)
                    .map(|reference| reference.rel_id.clone())
            })
            .last();
        let section = match source {
            Some(rel_id) if shows => {
                if let Some(part_name) = self.header_footer_part_name(&rel_id, is_header) {
                    return Ok((part_name, None));
                }
                last
            }
            Some(_) => last,
            // A first-page or even header put on the first section would show
            // on any earlier section with that kind of page, a blank cover
            // page among them.
            None if hdr_type == HdrFtrType::Default => 0,
            None => last,
        };
        Ok((
            self.reserve_hdr_ftr_part_name(is_header, hdr_type)?,
            Some(section),
        ))
    }

    /// Write a header or footer part where [`Self::reserve_hdr_ftr_target`]
    /// put it: in place for a shared part, else installed and referenced from
    /// its section.
    fn install_hdr_ftr_target(
        &mut self,
        part_name: String,
        section: Option<usize>,
        xml: Vec<u8>,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<String> {
        let Some(section) = section else {
            self.package.set_part(&part_name, xml);
            self.identifiers
                .authored_story_parts
                .insert(part_name.clone());
            return Ok(part_name);
        };
        self.install_hdr_ftr_part(part_name, section, xml, is_header, hdr_type)
    }

    fn set_raw_hdr_ftr_with_images(
        &mut self,
        xml: Vec<u8>,
        images: &[(&str, &[u8], &str)],
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        let (part_name, section) = self.reserve_hdr_ftr_target(is_header, hdr_type)?;
        let mut requested = HashSet::new();
        for &(rel_id, _, _) in images {
            if !requested.insert(rel_id) {
                return Err(Error::Other(format!(
                    "duplicate supplied header or footer relationship id {rel_id}"
                )));
            }
        }
        self.clear_authored_story_relationships(&part_name);
        let mut relationship_remap = HashMap::new();
        for &(rel_id, _, _) in images {
            let allocated = self
                .identifiers
                .reserve_requested_relationship_id_checked(&part_name, rel_id)
                .map_err(|error| {
                    Error::Other(format!(
                        "header or footer image relationship allocation failed: {error}"
                    ))
                })?;
            if allocated != rel_id {
                relationship_remap.insert(rel_id.to_owned(), allocated);
            }
        }
        let xml = remap_xml_relationship_ids(&xml, &relationship_remap)?;
        // The given drawings take their ids now, as a cloned section story
        // does: the save keeps every id it finds.
        let xml = rewrite_authored_doc_pr_ids(&xml, &mut self.identifiers.drawing_ids)?;
        let part_name =
            self.install_hdr_ftr_target(part_name, section, xml, is_header, hdr_type)?;

        for &(rel_id, image_data, image_filename) in images {
            let img_rel_target = self.store_image_part(image_data, image_filename);
            let allocated = relationship_remap
                .get(rel_id)
                .map_or(rel_id, String::as_str);
            self.package
                .get_or_create_part_rels(&part_name)
                .add_with_id(allocated, rel_types::IMAGE, &img_rel_target);
        }
        Ok(())
    }

    fn set_header_footer_image_part(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        self.set_header_footer_image_bg_part(
            image_data,
            image_filename,
            width,
            height,
            None,
            is_header,
            hdr_type,
        )
    }

    fn set_header_footer_image_bg_part(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
        bg_color: Option<&str>,
        is_header: bool,
        hdr_type: HdrFtrType,
    ) -> Result<()> {
        use oxoffice_docx_oxml::properties::CT_Shd;

        let (part_name, section) = self.reserve_hdr_ftr_target(is_header, hdr_type)?;
        self.clear_authored_story_relationships(&part_name);

        let img_rel_id = self
            .add_image_relationship_checked(&part_name, image_data, image_filename)
            .map_err(|error| {
                Error::Other(format!(
                    "header or footer image relationship allocation failed: {error}"
                ))
            })?;

        let mut inline = CT_Inline::new(&img_rel_id, width.to_emu(), height.to_emu());
        inline.doc_pr_id = self.identifiers.reserve_drawing_id()?;
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(CT_Drawing::inline(inline))],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };

        let mut p = CT_P::new();
        p.runs.push(run);
        if let Some(color) = bg_color {
            p.properties = Some(CT_PPr {
                shading: Some(Box::new(CT_Shd {
                    val: "clear".to_string(),
                    color: Some("auto".to_string()),
                    fill: Some(color.to_string()),
                    ..Default::default()
                })),
                ..Default::default()
            });
        }

        let mut hdr_ftr = CT_HdrFtr::new();
        hdr_ftr.paragraphs.push(p);

        let xml = Self::serialize_hdr_ftr(&hdr_ftr, is_header)?;
        self.install_hdr_ftr_target(part_name, section, xml, is_header, hdr_type)?;
        Ok(())
    }

    /// The text of the header or footer the last section shows: its own,
    /// else the one it inherits from the nearest section before it that has
    /// one, as [`Self::section_story`] resolves it.
    fn get_header_footer_text(&self, is_header: bool, hdr_type: HdrFtrType) -> Option<String> {
        // From the last section back, the first one with a usable reference:
        // one header part read in the common case.
        let sections: Vec<&CT_SectPr> = self.section_properties_in_order().collect();
        let reference = sections.into_iter().rev().find_map(|section| {
            let refs = if is_header {
                &section.header_refs
            } else {
                &section.footer_refs
            };
            self.first_usable_header_footer_reference(refs, hdr_type, is_header)
        })?;
        Some(
            self.load_header_footer(&reference.rel_id, is_header)?
                .text(),
        )
    }
}

impl Document {
    /// Relationship IDs of every section's headers and footers, with a flag
    /// saying which kind each one is.
    pub(super) fn header_footer_rel_ids(&self) -> Vec<(String, bool)> {
        let mut rel_ids = Vec::new();
        let mut seen = HashSet::new();
        let mut collect = |section: &CT_SectPr| {
            for reference in &section.header_refs {
                let rel_id = (reference.rel_id.clone(), true);
                if seen.insert(rel_id.clone()) {
                    rel_ids.push(rel_id);
                }
            }
            for reference in &section.footer_refs {
                let rel_id = (reference.rel_id.clone(), false);
                if seen.insert(rel_id.clone()) {
                    rel_ids.push(rel_id);
                }
            }
        };
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
            {
                collect(section);
            }
        });
        if let Some(section) = &self.document.body.sect_pr {
            collect(section);
        }

        rel_ids
    }

    pub(super) fn header_footer_rel_ids_for_layout(&self) -> HashSet<String> {
        let even_headers_enabled = self.even_headers_enabled();
        let mut rel_ids = HashSet::new();
        let sections = oxoffice_docx_layout::engine::document_sections(&self.document);
        for section in sections {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if reference.hdr_ftr_type != HdrFtrType::Even || even_headers_enabled {
                    rel_ids.insert(reference.rel_id.clone());
                }
            }
        }
        rel_ids
    }

    pub(super) fn even_headers_enabled(&self) -> bool {
        self.even_and_odd_headers()
    }
}

impl Document {
    /// Load a header/footer part by its relationship ID.
    pub(super) fn header_footer_part_name(&self, rel_id: &str, is_header: bool) -> Option<String> {
        let expected_type = if is_header {
            rel_types::HEADER
        } else {
            rel_types::FOOTER
        };
        let relationship = self
            .package
            .get_part_rels(&self.doc_part_name)?
            .get_by_id(rel_id)
            .filter(|relationship| {
                relationship.rel_type == expected_type && relationship_is_internal(relationship)
            })?;
        Some(OpcPackage::resolve_rel_target(
            &self.doc_part_name,
            &relationship.target,
        ))
    }

    /// Load a header/footer part by its relationship ID and section-reference kind.
    pub(super) fn load_header_footer(&self, rel_id: &str, is_header: bool) -> Option<CT_HdrFtr> {
        let part_name = self.header_footer_part_name(rel_id, is_header)?;
        let xml = self.package.get_part(&part_name)?;
        if !header_footer_story_has_expected_root(xml, is_header) {
            return None;
        }
        CT_HdrFtr::from_xml(xml).ok()
    }

    pub(super) fn first_usable_header_footer_reference<'a>(
        &self,
        references: &'a [HdrFtrRef],
        hdr_type: HdrFtrType,
        is_header: bool,
    ) -> Option<&'a HdrFtrRef> {
        references.iter().find(|reference| {
            reference.hdr_ftr_type == hdr_type
                && self
                    .load_header_footer(&reference.rel_id, is_header)
                    .is_some()
        })
    }
}

impl Document {
    /// Save a header/footer part back to the OPC package.
    pub(super) fn save_header_footer(
        &mut self,
        rel_id: &str,
        hf: &CT_HdrFtr,
        is_header: bool,
    ) -> Result<()> {
        let part_name = self
            .header_footer_part_name(rel_id, is_header)
            .ok_or_else(|| {
                Error::Other(format!(
                    "header or footer relationship {rel_id} is missing, non-internal, or has the wrong type"
                ))
            })?;
        let xml = Self::serialize_hdr_ftr(hf, is_header)?;
        self.package.set_part(&part_name, xml);
        Ok(())
    }
}

pub(super) fn header_type_index(hdr_type: HdrFtrType) -> usize {
    match hdr_type {
        HdrFtrType::Default => 0,
        HdrFtrType::First => 1,
        HdrFtrType::Even => 2,
    }
}

pub(super) fn header_footer_story_has_expected_root(xml: &[u8], is_header: bool) -> bool {
    let expected_local_name = if is_header {
        b"hdr".as_slice()
    } else {
        b"ftr".as_slice()
    };
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    loop {
        let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) else {
            return false;
        };
        match event {
            Event::Start(element) | Event::Empty(element) => {
                return matches!(
                    namespace,
                    ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes()
                ) && matches_local_name(element.name().as_ref(), expected_local_name);
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof
            | Event::Text(_)
            | Event::CData(_)
            | Event::DocType(_)
            | Event::GeneralRef(_)
            | Event::End(_) => return false,
        }
        buffer.clear();
    }
}

pub(super) fn materialize_header_footer_inheritance(
    document: &mut CT_Document,
    even_headers_enabled: bool,
    available_headers: &HashSet<String>,
    available_footers: &HashSet<String>,
) {
    let mut effective_headers: [Option<HdrFtrRef>; 3] = [None, None, None];
    let mut effective_footers: [Option<HdrFtrRef>; 3] = [None, None, None];
    let inherit = |references: &mut Vec<HdrFtrRef>,
                   effective: &mut [Option<HdrFtrRef>; 3],
                   available: &HashSet<String>,
                   materialize_blank_even: bool| {
        for hdr_type in [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even] {
            let index = header_type_index(hdr_type);
            if let Some(reference) = references
                .iter()
                .find(|reference| {
                    reference.hdr_ftr_type == hdr_type && available.contains(&reference.rel_id)
                })
                .cloned()
            {
                effective[index] = Some(reference);
            } else if let Some(reference) = effective[index].clone() {
                references.push(reference);
            } else if hdr_type == HdrFtrType::Even && materialize_blank_even {
                references.push(HdrFtrRef {
                    hdr_ftr_type: HdrFtrType::Even,
                    rel_id: String::new(),
                });
            }
        }
    };
    let mut apply = |section: &mut CT_SectPr| {
        inherit(
            &mut section.header_refs,
            &mut effective_headers,
            available_headers,
            even_headers_enabled,
        );
        inherit(
            &mut section.footer_refs,
            &mut effective_footers,
            available_footers,
            false,
        );
    };
    // Match document_sections: only main-story paragraphs and block controls
    // end sections. A paragraph in a table cell does not end a body section.
    fn control_sections(control: &mut CT_Sdt) -> Vec<&mut CT_SectPr> {
        let mut sections = Vec::new();
        for content in &mut control.content {
            match content {
                SdtContent::Paragraph(paragraph) => {
                    if let Some(section) = paragraph
                        .properties
                        .as_mut()
                        .and_then(|properties| properties.sect_pr.as_mut())
                    {
                        sections.push(section);
                    }
                }
                SdtContent::ContentControl(control) => sections.extend(control_sections(control)),
                _ => {}
            }
        }
        sections
    }
    for content in &mut document.body.content {
        match content {
            BodyContent::Paragraph(paragraph) => {
                if let Some(section) = paragraph
                    .properties
                    .as_mut()
                    .and_then(|properties| properties.sect_pr.as_mut())
                {
                    apply(section);
                }
            }
            BodyContent::ContentControl(control) => {
                for section in control_sections(control) {
                    apply(section);
                }
            }
            BodyContent::Table(_) | BodyContent::RawXml(_) => {}
        }
    }
    if let Some(section) = document.body.sect_pr.as_mut() {
        apply(section);
    }
}
