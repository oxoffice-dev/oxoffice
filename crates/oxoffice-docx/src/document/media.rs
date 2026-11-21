//! Pictures, charts and hyperlinks: adding, reading and replacing them.

use super::*;

impl Document {
    // ---- Image support ----

    /// Add an inline image to the document.
    ///
    /// Embeds the image data (PNG, JPEG, etc.) into the package and adds a
    /// paragraph containing the image. Returns a mutable reference to the
    /// paragraph for further configuration.
    ///
    /// `width` and `height` specify the display size.
    pub fn add_picture(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) -> Paragraph<'_> {
        self.try_add_picture(image_data, image_filename, width, height)
            .expect("an in-memory document cannot exhaust image identifiers");
        let Some(BodyContent::Paragraph(paragraph)) = self.document.body.content.last_mut() else {
            unreachable!("picture staging appends one paragraph")
        };
        Paragraph { inner: paragraph }
    }

    fn try_add_picture(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let owner = candidate.doc_part_name.clone();
        let rel_id =
            candidate.add_image_relationship_checked(&owner, image_data, image_filename)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;

        let mut inline = CT_Inline::new(&rel_id, width.to_emu(), height.to_emu());
        inline.doc_pr_id = drawing_id;

        let drawing = CT_Drawing::inline(inline);
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };

        let mut p = CT_P::new();
        p.runs.push(run);
        candidate
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(p)));
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    fn chart_legend_owner(
        &self,
        paragraph: usize,
    ) -> Result<(ContentLocation, Vec<u8>, WordChartReference, String)> {
        if self.content_index_of_paragraph(paragraph).is_none() {
            return Err(Error::Other(
                "chart legend requires a direct body paragraph".into(),
            ));
        }
        let location = self
            .paragraph_story_location(paragraph)?
            .ok_or_else(|| Error::Other("chart paragraph is absent".into()))?;
        let xml = self.paragraph_element_xml(&location)?;
        let references = word_chart_references(&xml)?
            .into_iter()
            .filter(|reference| reference.owned)
            .collect::<Vec<_>>();
        let [reference] = references.as_slice() else {
            return Err(Error::Other(
                "chart paragraph requires exactly one owned chart".into(),
            ));
        };
        let id = &reference.relationship_id;
        if self
            .package
            .get_part_rels(&self.doc_part_name)
            .map_or(0, |rels| rels.items.iter().filter(|r| r.id == *id).count())
            != 1
        {
            return Err(Error::Other(
                "chart relationship is missing or ambiguous".into(),
            ));
        }
        let relationship = self
            .package
            .get_part_rels(&self.doc_part_name)
            .and_then(|r| r.get_by_id(id))
            .filter(|r| r.rel_type == rel_types::CHART && relationship_is_internal(r))
            .ok_or_else(|| {
                Error::Other("chart relationship is missing, external or invalid".into())
            })?;
        let part = OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
        if self.package.content_types.content_type_for(&part) != Some(content_types::CHART)
            || self.package.get_part(&part).is_none()
        {
            return Err(Error::Other(
                "chart target is missing or has invalid content type".into(),
            ));
        }
        if let Some(rels) = self.package.get_part_rels(&part) {
            for rel in &rels.items {
                if relationship_is_internal(rel)
                    && self
                        .package
                        .get_part(&OpcPackage::resolve_rel_target(&part, &rel.target))
                        .is_none()
                {
                    return Err(Error::Other("chart related part is missing".into()));
                }
            }
        }
        Ok((location, xml, reference.clone(), part))
    }

    /// Owned chart snapshots in direct main-body paragraph and occurrence order.
    /// Related stories, tables and opaque wrappers are outside this finite scope.
    pub fn charts(&self) -> Result<Vec<(Option<i64>, oxoffice_chart::ChartData)>> {
        let mut charts = Vec::new();
        let source = self.document.to_xml()?;
        let owner = scan_story_owners(&source, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("chart body owner is absent".into()))?;
        let paragraphs = scan_story_items(&source, &owner, None)?
            .into_iter()
            .filter(|item| item.direct_owner_child && item.kind == StoryItemKind::Paragraph)
            .collect::<Vec<_>>();
        let scopes =
            story_namespace_scopes_at(&source, paragraphs.iter().map(|item| item.full.start))?;
        for paragraph in paragraphs {
            let scope = scopes
                .get(&paragraph.full.start)
                .ok_or_else(|| Error::Other("chart paragraph namespace scope is absent".into()))?;
            let xml = close_content_fragment_namespaces(&source[paragraph.full], scope)?;
            for reference in word_chart_references(&xml)?.into_iter().filter(|r| r.owned) {
                let relationships = self
                    .package
                    .get_part_rels(&self.doc_part_name)
                    .ok_or_else(|| Error::Other("chart relationships are absent".into()))?;
                let mut matching = relationships
                    .items
                    .iter()
                    .filter(|r| r.id == reference.relationship_id);
                let relationship = matching
                    .next()
                    .filter(|r| r.rel_type == rel_types::CHART && relationship_is_internal(r))
                    .ok_or_else(|| {
                        Error::Other("chart relationship is missing, external or invalid".into())
                    })?;
                if matching.next().is_some() {
                    return Err(Error::Other("chart relationship is ambiguous".into()));
                }
                let part =
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                if self.package.content_types.content_type_for(&part) != Some(content_types::CHART)
                {
                    return Err(Error::Other(format!(
                        "chart {part} has invalid content type"
                    )));
                }
                let source = self
                    .package
                    .get_part(&part)
                    .ok_or_else(|| Error::Other(format!("chart {part} is absent")))?;
                let kind = oxoffice_chart::read_chart_type(source)
                    .map_err(|e| Error::Other(format!("chart {part}: {e}")))?;
                let (_, data) = oxoffice_chart::read_chart_data(source)
                    .map_err(|e| Error::Other(format!("chart {part}: {e}")))?;
                charts.push((kind, data));
            }
        }
        Ok(charts)
    }

    /// Read effective legend settings from exactly one owned direct-body chart.
    pub fn chart_legend(&self, paragraph: usize) -> Result<oxoffice_chart::LegendSettings> {
        let (_, _, _, part) = self.chart_legend_owner(paragraph)?;
        oxoffice_chart::chart_legend_settings(
            self.package
                .get_part(&part)
                .ok_or_else(|| Error::Other("chart part disappeared".into()))?,
        )
        .map_err(|e| Error::Other(format!("invalid chart legend: {e}")))
    }

    /// Atomically edit one chart legend, isolating any shared ChartML part.
    pub fn set_chart_legend(
        &mut self,
        paragraph: usize,
        visible: Option<bool>,
        position: Option<oxoffice_chart::LegendPosition>,
        overlay: Option<bool>,
    ) -> Result<()> {
        let (location, xml, reference, part) = self.chart_legend_owner(paragraph)?;
        let id = &reference.relationship_id;
        let source = self
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("chart part disappeared".into()))?;
        let updated = oxoffice_chart::set_chart_legend(source, visible, position, overlay)
            .map_err(|e| Error::Other(format!("invalid chart legend: {e}")))?;
        if updated == source {
            return Ok(());
        }
        let references = word_chart_references(&self.document.to_xml()?)?;
        let shared = references
            .iter()
            .filter(|r| &r.relationship_id == id)
            .count()
            > 1
            || self
                .package
                .part_rels
                .iter()
                .flat_map(|(owner, rels)| rels.items.iter().map(move |r| (owner, r)))
                .filter(|(owner, r)| {
                    relationship_is_internal(r)
                        && OpcPackage::resolve_rel_target(owner, &r.target)
                            .eq_ignore_ascii_case(&part)
                })
                .count()
                > 1;
        let mut candidate = self.clone_for_staging();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let target = if shared {
            candidate
                .identifiers
                .reserve_part_name("/word/charts", "chart", "xml")?
        } else {
            part.clone()
        };
        if shared {
            if let Some(mut rels) = candidate.package.get_part_rels(&part).cloned() {
                for rel in &mut rels.items {
                    if relationship_is_internal(rel) {
                        let dest = OpcPackage::resolve_rel_target(&part, &rel.target);
                        if candidate.package.get_part(&dest).is_none() {
                            return Err(Error::Other("chart related part is missing".into()));
                        }
                        rel.target = relative_target(&target, &dest);
                    }
                }
                candidate.package.set_part_rels(&target, rels);
            }
            let owner = candidate.doc_part_name.clone();
            let isolated = candidate.add_relative_internal_relationship_checked(
                &owner,
                rel_types::CHART,
                &target,
            )?;
            let mut rewritten = xml;
            rewritten.splice(reference.value_span, isolated.bytes());
            let rewritten =
                std::str::from_utf8(&rewritten).map_err(|e| Error::Other(e.to_string()))?;
            candidate.package.set_part(&target, updated);
            candidate
                .package
                .content_types
                .add_override(&target, content_types::CHART);
            candidate.replace_paragraph_xml(&location, rewritten)?;
        } else {
            candidate.package.set_part(&target, updated);
        }
        candidate.invalidate_layout();
        let candidate = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Add an editable inline chart to the document.
    ///
    /// The ChartML cache and embedded workbook are authored from the same
    /// validated data. Invalid dimensions or data leave the document unchanged.
    pub fn add_chart(
        &mut self,
        kind: ChartKind,
        width: Length,
        height: Length,
        data: &ChartData,
    ) -> Result<Paragraph<'_>> {
        self.add_chart_package(ChartPackageSource::Authored { kind, data }, width, height)?;
        let Some(BodyContent::Paragraph(paragraph)) = self.document.body.content.last_mut() else {
            unreachable!("chart package assembly appends one paragraph");
        };
        Ok(Paragraph { inner: paragraph })
    }

    /// Insert an editable chart immediately after a direct body paragraph.
    /// Validation and package assembly complete on a candidate before publication.
    pub fn add_chart_after(
        &mut self,
        after: usize,
        kind: ChartKind,
        width: Length,
        height: Length,
        data: &ChartData,
    ) -> Result<Paragraph<'_>> {
        let index = self.content_index_of_paragraph(after).ok_or_else(|| {
            Error::Other("chart insertion requires a direct body paragraph".to_owned())
        })? + 1;
        let mut candidate = self.clone_for_staging();
        candidate.add_chart(kind, width, height, data)?;
        let paragraph =
            candidate.document.body.content.pop().ok_or_else(|| {
                Error::Other("chart assembly did not create a paragraph".to_owned())
            })?;
        candidate.document.body.content.insert(index, paragraph);
        candidate.document.to_xml()?;
        self.commit_staged_mutation(candidate);
        let BodyContent::Paragraph(paragraph) = &mut self.document.body.content[index] else {
            unreachable!("chart assembly creates a paragraph");
        };
        Ok(Paragraph { inner: paragraph })
    }

    /// Add an inline image at its native size using 72 DPI when none is declared.
    ///
    /// Returns an error without changing the document when the image dimensions
    /// cannot be determined.
    pub fn add_picture_auto(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
    ) -> Result<Paragraph<'_>> {
        let native_size = oxoffice_media::probe(image_data)
            .and_then(|info| info.native_size(72.0))
            .ok_or_else(|| Error::UnavailableImageDimensions {
                filename: image_filename.to_owned(),
            })?;

        self.try_add_picture(
            image_data,
            image_filename,
            Length::emu(native_size.width_emu),
            Length::emu(native_size.height_emu),
        )?;
        let Some(BodyContent::Paragraph(paragraph)) = self.document.body.content.last_mut() else {
            unreachable!("picture staging appends one paragraph")
        };
        Ok(Paragraph { inner: paragraph })
    }

    /// Add a full-page background image behind text.
    ///
    /// The image is placed at position (0,0) relative to the page with
    /// dimensions matching the page size from section properties.
    /// It is inserted at the beginning of the document body so it renders
    /// behind all other content.
    pub fn add_background_image(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
    ) -> Paragraph<'_> {
        let mut candidate = self.clone_for_staging();
        let rel_id = candidate.embed_image(image_data, image_filename);

        // Get page dimensions from section properties (default US Letter)
        let sect = candidate
            .document
            .body
            .sect_pr
            .as_ref()
            .cloned()
            .unwrap_or_else(CT_SectPr::default_letter);
        let page_width_emu = sect
            .page_width
            .unwrap_or(oxoffice_docx_oxml::units::Twips(12240))
            .to_emu()
            .0;
        let page_height_emu = sect
            .page_height
            .unwrap_or(oxoffice_docx_oxml::units::Twips(15840))
            .to_emu()
            .0;

        let mut anchor = CT_Anchor::background(&rel_id, page_width_emu, page_height_emu);
        // A full-page background is decoration: screen readers skip it and
        // Word's accessibility checker asks no alt text of it.
        anchor.decorative = true;
        anchor.doc_pr_id = candidate
            .identifiers
            .reserve_drawing_id()
            .expect("an in-memory document cannot exhaust every drawing identifier");
        let drawing = CT_Drawing::anchor(anchor);
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };

        let mut p = CT_P::new();
        p.runs.push(run);
        candidate.document.body.insert_paragraph(0, p);
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        match &mut self.document.body.content[0] {
            BodyContent::Paragraph(p) => Paragraph { inner: p },
            _ => unreachable!(),
        }
    }

    /// Add an anchored (floating) image to the document.
    ///
    /// If `behind_text` is true, the image renders behind text content.
    /// The image is inserted at the beginning of the document body.
    pub fn add_anchored_image(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
        behind_text: bool,
    ) -> Paragraph<'_> {
        let mut candidate = self.clone_for_staging();
        let rel_id = candidate.embed_image(image_data, image_filename);

        let mut anchor = CT_Anchor::background(&rel_id, width.to_emu(), height.to_emu());
        anchor.doc_pr_id = candidate
            .identifiers
            .reserve_drawing_id()
            .expect("an in-memory document cannot exhaust every drawing identifier");
        anchor.behind_doc = behind_text;

        let drawing = CT_Drawing::anchor(anchor);
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };

        let mut p = CT_P::new();
        p.runs.push(run);
        candidate.document.body.insert_paragraph(0, p);
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        match &mut self.document.body.content[0] {
            BodyContent::Paragraph(p) => Paragraph { inner: p },
            _ => unreachable!(),
        }
    }

    /// Store image bytes as a new media part and declare its content type.
    ///
    /// Returns the relationship target to use when referencing it, e.g.
    /// `media/image3.png`. No relationship is created here: an image referenced
    /// from a header or footer must be related to *that* part, not the
    /// document, so the caller decides where it is attached.
    pub(super) fn store_image_part(&mut self, image_data: &[u8], filename: &str) -> String {
        let (part_name, format) = self
            .reserve_image_part(image_data, filename)
            .expect("an in-memory package cannot exhaust every media part suffix");
        self.install_reserved_image_part(&part_name, image_data, format)
    }

    pub(super) fn reserve_image_part(
        &mut self,
        image_data: &[u8],
        filename: &str,
    ) -> Result<(String, oxoffice_media::ImageFormat)> {
        let format = oxoffice_media::resolve(image_data, filename);
        let part_name =
            self.identifiers
                .reserve_part_name("/word/media", "image", format.extension())?;
        Ok((part_name, format))
    }

    pub(super) fn install_reserved_image_part(
        &mut self,
        part_name: &str,
        image_data: &[u8],
        format: oxoffice_media::ImageFormat,
    ) -> String {
        let extension = format.extension();

        self.package.set_part(part_name, image_data.to_vec());
        let content_type = format.content_type();
        match self.package.content_types.content_type_for(part_name) {
            Some(existing) if existing == content_type => {}
            Some(_) => self
                .package
                .content_types
                .add_override(part_name, content_type),
            None => {
                self.package
                    .content_types
                    .add_default(extension, content_type);
                self.identifiers.register_content_type_default(extension);
            }
        }
        if self.package.content_types.contains_override(part_name) {
            self.identifiers.register_content_type_override(part_name);
        }

        part_name
            .strip_prefix("/word/")
            .unwrap_or(part_name)
            .to_owned()
    }

    /// Embed an image into the OPC package and return the relationship ID.
    ///
    /// Public so callers can pre-embed an image and then pass the returned
    /// `rel_id` to [`crate::Cell::add_picture`] for inline cell images.
    pub fn embed_image(&mut self, image_data: &[u8], filename: &str) -> String {
        let mut candidate = self.clone_for_staging();
        let owner = candidate.doc_part_name.clone();
        let relationship_id = candidate
            .add_image_relationship_checked(&owner, image_data, filename)
            .expect("an in-memory package cannot exhaust every relationship identifier");
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        relationship_id
    }
}

impl Document {
    /// Append an external hyperlink to the last paragraph (creating one if
    /// the document is empty): adds the External relationship and wraps the
    /// new run in a hyperlink span.
    pub fn append_hyperlink(&mut self, text: &str, url: &str) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        let owner = candidate.doc_part_name.clone();
        let rel_id = candidate
            .add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)
            .expect("an in-memory document can install a hyperlink relationship");

        if !matches!(
            candidate.document.body.content.last(),
            Some(BodyContent::Paragraph(_))
        ) {
            candidate
                .document
                .body
                .content
                .push(BodyContent::Paragraph(Box::default()));
        }
        let Some(BodyContent::Paragraph(p)) = candidate.document.body.content.last_mut() else {
            unreachable!();
        };
        crate::Paragraph { inner: p }.add_hyperlink(text, &rel_id);
        self.commit_staged_mutation(candidate);
    }

    /// Add an external hyperlink relationship and return its relationship ID.
    ///
    /// Use this with [`crate::Paragraph::add_hyperlink`] when the target
    /// paragraph is not the last body paragraph, such as a paragraph inside a
    /// table cell.
    pub fn add_hyperlink_relationship(&mut self, url: &str) -> String {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        let owner = candidate.doc_part_name.clone();
        let id = candidate
            .add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)
            .expect("an in-memory document can install a hyperlink relationship");
        self.commit_staged_mutation(candidate);
        id
    }

    /// Get a builder for the last paragraph in the body, if any. Lets
    /// callers interleave plain runs with `append_hyperlink` calls.
    pub fn last_paragraph_mut(&mut self) -> Option<Paragraph<'_>> {
        self.invalidate_layout();
        match self.document.body.content.last_mut() {
            Some(BodyContent::Paragraph(p)) => Some(Paragraph { inner: p }),
            _ => None,
        }
    }

    /// Fetch the raw bytes of an embedded image by its relationship ID.
    pub fn image_data(&self, rel_id: &str) -> Option<Vec<u8>> {
        let rels = self.package.get_part_rels(&self.doc_part_name)?;
        let rel = rels.items.iter().find(|relationship| {
            relationship.id == rel_id
                && relationship.rel_type == rel_types::IMAGE
                && relationship_is_internal(relationship)
        })?;
        let target = OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
        self.package.get_part(&target).map(|b| b.to_vec())
    }

    /// Replace an embedded body picture by its relationship ID.
    ///
    /// The relationship and every drawing that shows the picture, with its
    /// extent, alt text and position, stay as they are. Shared target parts use
    /// copy-on-write, and the target name and content type follow the new bytes.
    pub fn replace_image(&mut self, rel_id: &str, image_data: &[u8]) -> Result<()> {
        let owner = self.doc_part_name.clone();
        self.replace_image_relationship(&owner, rel_id, image_data)
    }

    /// Resize every main-document picture that shows one image relationship.
    ///
    /// Each inline or anchored `pic:pic` drawing whose blip names `rel_id`
    /// gets the new `wp:extent` and `pic:spPr` `a:ext`, so pictures that share
    /// the relationship are all resized. Its `wp:effectExtent` scales with the
    /// extent on each axis, and anchor positions stay as they are. Returns the
    /// number of drawings resized.
    ///
    /// A VML `w:pict` picture that uses the relationship is not resized. The
    /// relationship of an SVG blip extension is not matched, so pass the
    /// relationship of its `a:blip` instead. A picture inside a group is not
    /// resized, so a relationship that only grouped pictures use is an error.
    pub fn set_picture_size(
        &mut self,
        rel_id: &str,
        width: Length,
        height: Length,
    ) -> Result<usize> {
        const MAX_POSITIVE_COORDINATE: i64 = 27_273_042_316_900;
        let (cx, cy) = (width.to_emu(), height.to_emu());
        if !(1..=MAX_POSITIVE_COORDINATE).contains(&cx)
            || !(1..=MAX_POSITIVE_COORDINATE).contains(&cy)
        {
            return Err(Error::Other(format!(
                "picture size {cx} x {cy} EMU must be positive and within the DrawingML coordinate range"
            )));
        }
        let owner = self.doc_part_name.clone();
        let is_image = self
            .package
            .get_part_rels(&owner)
            .and_then(|relationships| relationships.get_by_id(rel_id))
            .is_some_and(|relationship| relationship.rel_type == rel_types::IMAGE);
        if !is_image {
            return Err(Error::Other(format!(
                "relationship owner {owner} has no image relationship {rel_id}"
            )));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (updated, resized) =
            resize_story_pictures(&candidate.document.to_xml()?, rel_id, cx, cy)?;
        if resized == 0 {
            return Err(Error::Other(format!(
                "no picture drawing shows image relationship {rel_id}"
            )));
        }
        set_story_source_xml(&mut candidate, &owner, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(resized)
    }

    /// Resolve a hyperlink relationship ID to its external URL.
    pub fn hyperlink_url(&self, rel_id: &str) -> Option<String> {
        use oxoffice_opc::relationship::rel_types;
        let rels = self.package.get_part_rels(&self.doc_part_name)?;
        rels.items
            .iter()
            .find(|relationship| {
                relationship.id == rel_id
                    && relationship.rel_type == rel_types::HYPERLINK
                    && relationship.target_mode.as_deref() == Some("External")
            })
            .map(|r| r.target.clone())
    }
}

#[derive(Clone)]
struct WordChartReference {
    relationship_id: String,
    value_span: Range<usize>,
    owned: bool,
}

fn word_chart_references(xml: &[u8]) -> Result<Vec<WordChartReference>> {
    let mut reader = NsReader::from_reader(xml);
    let mut references = Vec::new();
    let mut parents: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| Error::Other(e.to_string()))?;
        let namespace = match namespace {
            ResolveResult::Bound(Namespace(uri)) => quick_xml::escape::unescape(
                std::str::from_utf8(uri).map_err(|e| Error::Other(e.to_string()))?,
            )
            .map_err(|e| Error::Other(e.to_string()))?
            .as_bytes()
            .to_vec(),
            _ => Vec::new(),
        };
        let after = reader.buffer_position() as usize;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let local = element.local_name().as_ref().to_vec();
                // Only the supported direct paragraph/run/drawing owner grants
                // authority. A qualified lookalike under any opaque ancestor does not.
                let owned = namespace == oxoffice_chart::C_NS.as_bytes()
                    && local == b"chart"
                    && parents.len() == 6
                    && parents[0].0 == W_NS.as_bytes()
                    && parents[0].1 == b"p"
                    && parents[1].0 == W_NS.as_bytes()
                    && parents[1].1 == b"r"
                    && parents[2].0 == W_NS.as_bytes()
                    && parents[2].1 == b"drawing"
                    && parents[3].0 == drawing_ns::WP.as_bytes()
                    && matches!(parents[3].1.as_slice(), b"inline" | b"anchor")
                    && parents[4].0 == oxoffice_chart::A_NS.as_bytes()
                    && parents[4].1 == b"graphic"
                    && parents[5].0 == oxoffice_chart::A_NS.as_bytes()
                    && parents[5].1 == b"graphicData"
                    && parents[5].2;
                let previous_references = references.len();
                let mut chart_data = false;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|e| Error::Other(e.to_string()))?;
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|e| Error::Other(e.to_string()))?;
                    if attribute.key.as_ref() == b"uri" && value == oxoffice_chart::C_NS {
                        chart_data = true;
                    }
                    let (ns, name) = reader.resolver().resolve_attribute(attribute.key);
                    if matches!(ns, ResolveResult::Bound(Namespace(uri)) if
                        quick_xml::escape::unescape(std::str::from_utf8(uri)
                            .map_err(|e| Error::Other(e.to_string()))?)
                            .map_err(|e| Error::Other(e.to_string()))?.as_ref() == oxoffice_docx_oxml::namespace::R_NS)
                    {
                        let (start, end) =
                            story_attribute_value_span(&xml[before..after], attribute.key.as_ref())
                                .ok_or_else(|| {
                                    Error::Other("chart relationship source span is absent".into())
                                })?;
                        references.push(WordChartReference {
                            relationship_id: value.into_owned(),
                            value_span: before + start..before + end,
                            owned: owned && name.as_ref() == b"id",
                        });
                    }
                }
                if owned
                    && (references[previous_references..]
                        .iter()
                        .filter(|r| r.owned)
                        .count()
                        != 1
                        || references[previous_references..]
                            .iter()
                            .any(|r| r.owned && r.relationship_id.is_empty()))
                {
                    return Err(Error::Other(
                        "owned chart requires exactly one relationship id".into(),
                    ));
                }
                if !empty {
                    parents.push((namespace, local, chart_data));
                }
            }
            Event::End(_) => {
                parents.pop();
            }
            Event::Eof => return Ok(references),
            _ => {}
        }
    }
}

pub(super) fn chart_with_workbook_relationship(
    chart: &CT_ChartSpace,
    relationship_id: &str,
) -> Result<Vec<u8>> {
    const END: &[u8] = b"</c:chartSpace>";
    let mut xml = chart
        .to_xml()
        .map_err(|error| Error::Other(format!("invalid chart part: {error}")))?;
    if chart_has_external_data(&xml)? {
        return Err(Error::Other(
            "chart already contains an external workbook relationship".to_owned(),
        ));
    }
    if !xml.ends_with(END) {
        return Err(Error::Other(
            "serialized chart lacks the chartSpace closing element".to_owned(),
        ));
    }

    xml.truncate(xml.len() - END.len());
    xml.extend_from_slice(
        format!(
            r#"<c:externalData r:id="{relationship_id}"><c:autoUpdate val="0"/></c:externalData>"#
        )
        .as_bytes(),
    );
    xml.extend_from_slice(END);
    CT_ChartSpace::from_xml(&xml)
        .and_then(|validated| validated.to_xml())
        .map_err(|error| Error::Other(format!("invalid chart part: {error}")))
}

fn chart_has_external_data(xml: &[u8]) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut chart_space_depth = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid chart part XML: {error}")))?;
        match event {
            Event::Start(ref element) => {
                if chart_space_depth.is_none()
                    && chart_namespace_matches(&namespace)
                    && matches_local_name(element.name().as_ref(), b"chartSpace")
                {
                    chart_space_depth = Some(depth);
                } else if chart_space_depth.is_some_and(|root| depth == root + 1)
                    && chart_namespace_matches(&namespace)
                    && matches_local_name(element.name().as_ref(), b"externalData")
                {
                    return Ok(true);
                }
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("chart part XML depth overflow".to_owned()))?;
            }
            Event::Empty(ref element)
                if chart_space_depth.is_some_and(|root| depth == root + 1)
                    && chart_namespace_matches(&namespace)
                    && matches_local_name(element.name().as_ref(), b"externalData") =>
            {
                return Ok(true);
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn chart_namespace_matches(namespace: &ResolveResult<'_>) -> bool {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => *uri == oxoffice_chart::C_NS.as_bytes(),
        ResolveResult::Unknown(prefix) => *prefix == b"c",
        ResolveResult::Unbound => false,
    }
}
