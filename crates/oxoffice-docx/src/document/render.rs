//! Layout and its caches, and rendering to PDF, HTML, Markdown, PNG and SVG.

use super::*;

/// Caller-width layout measurement for one supported story item.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentMeasurement {
    /// Total occupied height in points, including paragraph spacing or table rows.
    pub height_points: f64,
    /// Ordered approximation and fallback diagnostics from production layout.
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
}

fn owned_font_aliases(font_aliases: &[(&str, &str)]) -> Vec<(String, String)> {
    font_aliases
        .iter()
        .map(|(requested, target)| ((*requested).to_owned(), (*target).to_owned()))
        .collect()
}

impl Document {
    /// Clear layouts derived from the current document state.
    pub(crate) fn invalidate_layout(&mut self) {
        // Checked body refusal reads this cache. A later typed edit that
        // already drops derived layout must not keep a savable or unsavable
        // answer from the previous model. The refusal check itself does not
        // call this function.
        self.unsavable_body_edit_cache = OnceLock::new();
        self.layout_cache
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.deterministic_layout_cache
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    /// Return the layout of a tier that may use system fonts, computing it
    /// once after each mutation and each change of tier.
    fn cached_layout(
        &self,
        policy: FontPolicy,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        let mut cache = self
            .layout_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut engine = self
            .normal_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let engine = Self::engine_for_policy(&mut engine, policy)?;
        let tier = engine.system_fonts();
        if let Some((cached_tier, layout)) = cache.as_ref()
            && *cached_tier == tier
        {
            return Ok(Arc::clone(layout));
        }

        let input = self.build_layout_input()?;
        let mut layout =
            oxoffice_docx_layout::layout_document_with_reusable_engine(engine, &input)?;
        annotate_undrawable_images(&mut layout, &input);
        let layout = Arc::new(layout);
        *cache = Some((tier, Arc::clone(&layout)));
        Ok(layout)
    }

    /// The reusable engine of a tier that may use system fonts, replacing
    /// the retained engine when it serves the other tier.
    fn engine_for_policy(
        slot: &mut Option<oxoffice_docx_layout::engine::Engine>,
        policy: FontPolicy,
    ) -> Result<&mut oxoffice_docx_layout::engine::Engine> {
        let system_first = matches!(policy, FontPolicy::SystemWithBundledFallback);
        let reusable = slot.as_ref().is_some_and(|engine| {
            (engine.system_fonts() == oxoffice_layout::SystemFonts::First) == system_first
        });
        if !reusable {
            *slot = Some(if system_first {
                oxoffice_docx_layout::engine::Engine::new()
            } else {
                oxoffice_docx_layout::engine::Engine::new_with_system_fallback()?
            });
        }
        Ok(slot.as_mut().expect("the engine slot was just filled"))
    }

    /// Return the bundled-font-only layout, computing it once after mutation.
    fn cached_deterministic_layout(&self) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        let mut cache = self
            .deterministic_layout_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(layout) = cache.as_ref() {
            return Ok(Arc::clone(layout));
        }

        let input = self.build_layout_input()?;
        let mut layout =
            oxoffice_docx_layout::layout_document_deterministic_with_provenance(&input)?;
        annotate_undrawable_images(&mut layout, &input);
        let layout = Arc::new(layout);
        *cache = Some(Arc::clone(&layout));
        Ok(layout)
    }

    /// One private owner for the font tier and caller-font selection.
    ///
    /// Empty caller fonts use the cached layouts: the policy cache `Arc` is returned
    /// unchanged. A tracked view stays uncached. Nonempty caller fonts keep
    /// caller priority and return `Arc::new` of the owned public method.
    fn layout_with_font_policy(
        &self,
        policy: FontPolicy,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        if options.use_system_fonts && options.bundled_fonts_only {
            return Err(Error::Other(FONT_SWITCHES_EXCLUSIVE.to_owned()));
        }
        if matches!(policy, FontPolicy::SystemWithBundledFallback)
            && !cfg!(feature = "system-fonts")
        {
            return Err(Error::Other(SYSTEM_FONTS_UNAVAILABLE.to_owned()));
        }
        if font_files.is_empty() {
            return self.layout_for_policy(options, policy);
        }
        let owned = match policy {
            FontPolicy::BundledOnly => {
                self.layout_with_fonts_and_bundled_fallback_and_options(font_files, options)?
            }
            FontPolicy::BundledWithSystemFallback => {
                self.layout_with_fonts_and_system_fallback_and_options(font_files, options)?
            }
            FontPolicy::SystemWithBundledFallback => {
                self.layout_with_additional_fonts_and_options(font_files, options)?
            }
        };
        Ok(Arc::new(owned))
    }

    fn layout_for_policy(
        &self,
        options: RenderOptions,
        policy: FontPolicy,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        if options.revision_style != oxoffice_docx_layout::RevisionStyle::Plain
            && options.revision_view != oxoffice_docx_layout::RevisionView::Tracked
        {
            return Err(Error::Other(
                "revision style applies only with tracked view".to_owned(),
            ));
        }
        if options.fillable_forms
            && options.revision_view == oxoffice_docx_layout::RevisionView::Tracked
        {
            return Err(Error::Other(FILLABLE_FORMS_NEED_ACCEPTED_VIEW.to_owned()));
        }
        // The cached layouts carry no form fields.
        if options.revision_view == oxoffice_docx_layout::RevisionView::Accepted
            && !options.fillable_forms
        {
            return match policy {
                FontPolicy::BundledOnly => self.cached_deterministic_layout(),
                _ => self.cached_layout(policy),
            };
        }

        let mut input = self.build_layout_input()?;
        input.revision_view = options.revision_view;
        input.revision_style = options.revision_style;
        input.form_fields = options.fillable_forms;
        let mut layout = if policy == FontPolicy::BundledOnly {
            oxoffice_docx_layout::layout_document_deterministic_with_provenance(&input)?
        } else {
            let mut engine = self
                .normal_layout_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let engine = Self::engine_for_policy(&mut engine, policy)?;
            oxoffice_docx_layout::layout_document_with_reusable_engine(engine, &input)?
        };
        annotate_undrawable_images(&mut layout, &input);
        Ok(Arc::new(layout))
    }
}

impl Document {
    // ---- Layout and PDF conversion ----

    /// Transfer compatible reusable normal-layout work from another document.
    ///
    /// The transfer succeeds only when every retained-work input other than
    /// body content matches exactly. A rejected transfer leaves both engines
    /// unchanged. Completed layout results remain owned by their documents.
    pub fn transfer_reusable_layout_from(&mut self, source: &mut Document) -> bool {
        let Ok(input) = self.build_layout_input() else {
            return false;
        };
        let transferred = {
            let mut source_engine = source
                .normal_layout_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            oxoffice_docx_layout::engine::Engine::take_if_compatible(&mut source_engine, &input)
        };
        let Some(transferred) = transferred else {
            return false;
        };
        let mut receiver_engine = self
            .normal_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *receiver_engine = Some(transferred);
        true
    }

    /// Transfer compatible bundled-fallback layout work from another document.
    ///
    /// The exact caller-font slice is part of the compatibility check. A
    /// rejected transfer leaves both private engines unchanged. This method
    /// never exposes the engine or permits system-font discovery.
    pub fn transfer_reusable_bundled_fallback_layout_from(
        &mut self,
        source: &mut Document,
        font_files: &[(&str, &[u8])],
    ) -> bool {
        let Ok(input) = self.build_layout_input_with_fonts(font_files, RenderOptions::default())
        else {
            return false;
        };
        let transferred = {
            let mut source_engine = source
                .bundled_fallback_layout_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            oxoffice_docx_layout::engine::Engine::take_if_compatible(&mut source_engine, &input)
        };
        let Some(transferred) = transferred else {
            return false;
        };
        let mut receiver_engine = self
            .bundled_fallback_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *receiver_engine = Some(transferred);
        true
    }

    /// Transfer compatible alias-aware bundled-fallback layout work.
    ///
    /// Caller-font bytes and the exact ordered alias slice are both part of
    /// compatibility. A rejected transfer preserves both private engines.
    pub fn transfer_reusable_bundled_fallback_layout_from_with_aliases(
        &mut self,
        source: &mut Document,
        font_files: &[(&str, &[u8])],
        font_aliases: &[(&str, &str)],
    ) -> bool {
        let Ok(input) = self.build_layout_input_with_fonts(font_files, RenderOptions::default())
        else {
            return false;
        };
        let font_aliases = owned_font_aliases(font_aliases);
        let transferred = {
            let mut source_engine = source
                .bundled_fallback_layout_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            oxoffice_docx_layout::engine::Engine::take_if_compatible_with_caller_aliases(
                &mut source_engine,
                &input,
                &font_aliases,
            )
        };
        let Some(transferred) = transferred else {
            return false;
        };
        let mut receiver_engine = self
            .bundled_fallback_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *receiver_engine = Some(transferred);
        true
    }

    /// Return the cached default layout with its Word source map.
    ///
    /// Families resolve in the embedded and bundled fonts, and a system font
    /// only draws the characters they cannot (see [`Self::to_pdf`]). Set
    /// [`RenderOptions::bundled_fonts_only`] or
    /// [`RenderOptions::use_system_fonts`] through
    /// [`Self::layout_with_options`] for the other tiers. Repeated calls share
    /// the same accepted-view result until the document is mutated.
    pub fn layout(&self) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        self.layout_with_options(RenderOptions::default())
    }

    /// Return a layout with the selected revision view and font tier.
    ///
    /// Accepted-view calls are cached per tier: the bundled-only cache, or one
    /// shared by the default tier and system fonts first, which holds the last
    /// of the two asked for. Tracked-view calls remain uncached because they
    /// do not replace the accepted-view cache.
    pub fn layout_with_options(
        &self,
        options: RenderOptions,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        self.layout_with_font_policy(font_policy(options), &[], options)
    }

    /// Return the bundled-font-only layout with its Word source map.
    ///
    /// Repeated accepted-view calls share the deterministic layout cache. This
    /// is the same snapshot used by deterministic PDF and raster render helpers.
    pub fn layout_deterministic(&self) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        self.layout_deterministic_with_options(RenderOptions::default())
    }

    /// Return a bundled-font-only layout with the selected revision view.
    ///
    /// Accepted-view calls share the deterministic layout cache. Tracked-view
    /// calls remain uncached because they do not replace the accepted-view cache.
    pub fn layout_deterministic_with_options(
        &self,
        options: RenderOptions,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        self.layout_for_policy(options, FontPolicy::BundledOnly)
    }

    /// Measure one checked paragraph or table at a positive caller width.
    ///
    /// Measurement uses deterministic bundled fonts and the same layout input,
    /// revision projection, and block rules as whole-document layout. It does
    /// not mutate the document or either reusable layout cache.
    pub fn measure_content(
        &self,
        location: &ContentLocation,
        width: Length,
        options: RenderOptions,
    ) -> Result<ContentMeasurement> {
        if width.to_emu() <= 0 {
            return Err(Error::Other(
                "content measurement width must be positive".to_owned(),
            ));
        }
        let (source, item) = self.story_item_source(location)?;
        if !matches!(item.kind, StoryItemKind::Paragraph | StoryItemKind::Table) {
            return Err(Error::Other(format!(
                "story item kind {:?} cannot be measured",
                item.kind
            )));
        }
        let namespace_scope = story_namespace_scope_at(source.xml.as_ref(), item.full.start)?;
        let fragment = close_content_fragment_namespaces(
            &source.xml.as_ref()[item.full.clone()],
            &namespace_scope,
        )?;
        let mut wrapped =
            format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
        wrapped.extend(fragment);
        wrapped.extend_from_slice(b"</w:body></w:document>");
        let mut parsed = CT_Document::from_xml(&wrapped)?;
        let content = parsed
            .body
            .content
            .pop()
            .ok_or_else(|| Error::Other("measurable story item did not parse".to_owned()))?;
        if !parsed.body.content.is_empty()
            || !matches!(
                (&content, item.kind),
                (BodyContent::Paragraph(_), StoryItemKind::Paragraph)
                    | (BodyContent::Table(_), StoryItemKind::Table)
            )
        {
            return Err(Error::Other(
                "measurable story item did not parse as one matching block".to_owned(),
            ));
        }
        let related_story_scope = self
            .package
            .get_part_rels(&self.doc_part_name)
            .and_then(|relationships| {
                relationships.items.iter().find(|relationship| {
                    relationship_is_internal(relationship)
                        && matches!(
                            relationship.rel_type.as_str(),
                            rel_types::HEADER | rel_types::FOOTER
                        )
                        && OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
                            == location.story.part_name
                })
            })
            .map(|relationship| relationship.id.as_str());
        let input = self.build_layout_input_with_fonts(&[], options)?;
        let (height_points, diagnostics) = oxoffice_docx_layout::measure_content_deterministic(
            &input,
            &content,
            width.to_pt(),
            related_story_scope,
        )?;
        Ok(ContentMeasurement {
            height_points,
            diagnostics,
        })
    }

    /// Return an uncached layout using user-provided font files.
    ///
    /// User-provided fonts take highest priority in font resolution. The
    /// returned owned result retains the exact font bytes and Word source map
    /// used by layout.
    pub fn layout_with_fonts(
        &self,
        font_files: &[(&str, &[u8])],
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        self.layout_with_fonts_and_options(font_files, RenderOptions::default())
    }

    /// Return an uncached caller-font layout with the selected revision view.
    pub fn layout_with_fonts_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        let input = self.build_layout_input_with_fonts(font_files, options)?;
        let mut layout =
            oxoffice_docx_layout::layout_document_with_caller_fonts_and_provenance(&input)?;
        annotate_undrawable_images(&mut layout, &input);
        Ok(layout)
    }

    /// Return an uncached layout using caller fonts over bundled fallbacks.
    ///
    /// Caller faces have highest priority. Missing families resolve from the
    /// deterministic bundled inventory, never from the system-font snapshot.
    /// The returned result owns its source map and shares its heavy page and
    /// font payloads internally.
    pub fn layout_with_fonts_and_bundled_fallback(
        &self,
        font_files: &[(&str, &[u8])],
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        self.layout_with_fonts_and_bundled_fallback_and_options(
            font_files,
            RenderOptions::default(),
        )
    }

    /// Return a bundled-fallback caller-font layout for one revision view.
    pub fn layout_with_fonts_and_bundled_fallback_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        let input = self.build_layout_input_with_fonts(font_files, options)?;
        let mut engine = self
            .bundled_fallback_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let engine = match engine.as_mut() {
            Some(engine) => engine,
            None => engine.insert(oxoffice_docx_layout::engine::Engine::new_deterministic()?),
        };
        engine.set_caller_font_aliases(&[]);
        let mut layout =
            oxoffice_docx_layout::layout_document_with_reusable_engine(engine, &input)?;
        annotate_undrawable_images(&mut layout, &input);
        Ok(layout)
    }

    /// Return the layout every render with these caller fonts and options
    /// draws, as [`Self::to_pdf_with_diagnostics`] does.
    ///
    /// Caller faces keep highest priority. Then, by default, families resolve
    /// in the embedded and bundled fonts and a system font only draws the
    /// characters they cannot; [`RenderOptions::bundled_fonts_only`] never
    /// takes a system font, and [`RenderOptions::use_system_fonts`] resolves
    /// families in the system fonts first. Without caller fonts the result is
    /// cached until the next edit.
    pub fn layout_for_render(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Arc<oxoffice_docx_layout::WordLayoutResult>> {
        self.layout_with_font_policy(font_policy(options), font_files, options)
    }

    /// An uncached caller-font layout over the bundled fonts, with a system
    /// font only for characters neither draws.
    fn layout_with_fonts_and_system_fallback_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        let input = self.build_layout_input_with_fonts(font_files, options)?;
        let mut layout =
            oxoffice_docx_layout::layout_document_with_system_fallback_and_provenance(&input)?;
        annotate_undrawable_images(&mut layout, &input);
        Ok(layout)
    }

    /// Return a reusable bundled-fallback layout with byte-free font aliases.
    ///
    /// Exact embedded families retain priority. A caller alias is tried next,
    /// before the existing mapped and generic fallbacks.
    pub fn layout_with_fonts_aliases_and_bundled_fallback(
        &self,
        font_files: &[(&str, &[u8])],
        font_aliases: &[(&str, &str)],
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        self.layout_with_fonts_aliases_and_bundled_fallback_and_options(
            font_files,
            font_aliases,
            RenderOptions::default(),
        )
    }

    /// Return an alias-aware bundled-fallback layout for one revision view.
    pub fn layout_with_fonts_aliases_and_bundled_fallback_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        font_aliases: &[(&str, &str)],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        let input = self.build_layout_input_with_fonts(font_files, options)?;
        let font_aliases = owned_font_aliases(font_aliases);
        let mut engine = self
            .bundled_fallback_layout_engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let engine = match engine.as_mut() {
            Some(engine) => engine,
            None => engine.insert(oxoffice_docx_layout::engine::Engine::new_deterministic()?),
        };
        engine.set_caller_font_aliases(&font_aliases);
        let mut layout =
            oxoffice_docx_layout::layout_document_with_reusable_engine(engine, &input)?;
        annotate_undrawable_images(&mut layout, &input);
        Ok(layout)
    }

    fn build_layout_input_with_fonts(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::LayoutInput> {
        if options.revision_style != oxoffice_docx_layout::RevisionStyle::Plain
            && options.revision_view != oxoffice_docx_layout::RevisionView::Tracked
        {
            return Err(Error::Other(
                "revision style applies only with tracked view".to_owned(),
            ));
        }
        if options.fillable_forms
            && options.revision_view == oxoffice_docx_layout::RevisionView::Tracked
        {
            return Err(Error::Other(FILLABLE_FORMS_NEED_ACCEPTED_VIEW.to_owned()));
        }
        let mut input = self.build_layout_input()?;
        input.revision_view = options.revision_view;
        input.revision_style = options.revision_style;
        input.form_fields = options.fillable_forms;
        input.fonts.extend(font_files.iter().map(|(family, data)| {
            oxoffice_docx_layout::FontFile {
                family: (*family).to_owned(),
                data: data.to_vec(),
            }
        }));
        Ok(input)
    }

    /// Render the document to PDF bytes.
    ///
    /// This performs a full layout pass (font shaping, line breaking, pagination)
    /// and then renders the result to a PDF document.
    ///
    /// Font resolution order:
    /// 1. Fonts embedded in the DOCX file (word/fonts/)
    /// 2. The bundled fonts: metric-compatible twins of the common Office
    ///    fonts, and faces for Arabic, Hebrew, Devanagari and Thai
    /// 3. For a character none of these can draw, such as most Chinese,
    ///    Japanese and Korean text (the bundled CJK faces hold only a few
    ///    characters), a system font that has it, which a layout diagnostic
    ///    names. No family name selects a system font, and in body text only
    ///    the missing graphemes take it, so text the first two draw there is
    ///    the same on every machine (a chart label, list marker or watermark
    ///    holding such a character takes the system font for all its text).
    ///    Without such a font the text is drawn as empty boxes, which a
    ///    diagnostic names too.
    ///
    /// [`RenderOptions::with_bundled_fonts_only`] in
    /// [`Self::to_pdf_with_options`] skips step 3, for the same output on
    /// every machine; [`RenderOptions::with_system_fonts`] resolves families
    /// in the system fonts first.
    pub fn to_pdf(&self) -> Result<Vec<u8>> {
        self.to_pdf_with_options(RenderOptions::default())
    }

    /// Render the document to PDF bytes with the selected revision view.
    pub fn to_pdf_with_options(&self, options: RenderOptions) -> Result<Vec<u8>> {
        let layout = self.layout_with_options(options)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout.layout)?)
    }

    /// Render the document to PDF bytes using bundled fonts without system
    /// font discovery.
    ///
    /// The deterministic layout is cached independently from the normal-font
    /// layout and is suitable for reproducible render baselines.
    pub fn to_pdf_deterministic(&self) -> Result<Vec<u8>> {
        self.to_pdf_deterministic_with_options(RenderOptions::default())
    }

    /// Render the document to the selected archival PDF profile using bundled fonts.
    pub fn to_pdfa_deterministic(&self, profile: oxoffice_pdf::PdfConformance) -> Result<Vec<u8>> {
        let layout = self.layout_for_policy(RenderOptions::default(), FontPolicy::BundledOnly)?;
        Ok(oxoffice_pdf::render_to_pdf_with_options(
            &layout.layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?)
    }

    /// Encode a deterministic archival PDF with these render options, such
    /// as fillable form fields.
    pub fn to_pdfa_deterministic_with_options(
        &self,
        profile: oxoffice_pdf::PdfConformance,
        options: RenderOptions,
    ) -> Result<Vec<u8>> {
        let layout = self.layout_for_policy(options, FontPolicy::BundledOnly)?;
        Ok(oxoffice_pdf::render_to_pdf_with_options(
            &layout.layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?)
    }

    /// Encode one bundled-fallback caller-font archival bundle with its owned diagnostics.
    pub fn to_pdfa_with_diagnostics(
        &self,
        profile: oxoffice_pdf::PdfConformance,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout =
            self.layout_with_fonts_and_bundled_fallback_and_options(font_files, options)?;
        let bytes = oxoffice_pdf::render_to_pdf_with_options(
            &layout.layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?;
        Ok((bytes, layout.layout.diagnostics.clone()))
    }

    /// Render the selected revision view to deterministic PDF bytes.
    pub fn to_pdf_deterministic_with_options(&self, options: RenderOptions) -> Result<Vec<u8>> {
        let layout = self.layout_for_policy(options, FontPolicy::BundledOnly)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout.layout)?)
    }

    /// Lay out caller fonts over the normal system and bundled fallback policy.
    /// The returned bundle owns the exact face instances used by every renderer.
    pub fn layout_with_additional_fonts_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_docx_layout::WordLayoutResult> {
        let input = self.build_layout_input_with_fonts(font_files, options)?;
        let mut layout = oxoffice_docx_layout::layout_document_with_provenance(&input)?;
        annotate_undrawable_images(&mut layout, &input);
        Ok(layout)
    }

    /// Render caller fonts using the bundled fallback policy and selected revision view.
    pub fn to_pdf_with_fonts_deterministic_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Vec<u8>> {
        let layout =
            self.layout_with_fonts_and_bundled_fallback_and_options(font_files, options)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout.layout)?)
    }

    /// Render caller fonts to the selected archival profile using bundled fallback.
    pub fn to_pdfa_with_fonts_deterministic(
        &self,
        font_files: &[(&str, &[u8])],
        profile: oxoffice_pdf::PdfConformance,
    ) -> Result<Vec<u8>> {
        let layout = self.layout_with_fonts_and_bundled_fallback(font_files)?;
        Ok(oxoffice_pdf::render_to_pdf_with_options(
            &layout.layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?)
    }

    /// Render the document to PDF bytes with user-provided font files.
    ///
    /// User-provided fonts take highest priority in font resolution.
    ///
    /// # Arguments
    /// * `font_files` - Additional font files to use. Each entry is `(family_name, font_bytes)`.
    ///
    /// Font resolution order:
    /// 1. User-provided fonts (this parameter)
    /// 2. Fonts embedded in the DOCX file (word/fonts/)
    /// 3. The bundled fonts, then a system font for characters none of these
    ///    draws, as for [`Self::to_pdf`]
    pub fn to_pdf_with_fonts(&self, font_files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
        self.to_pdf_with_fonts_and_options(font_files, RenderOptions::default())
    }

    /// Render the selected revision view to PDF with user-provided fonts.
    ///
    /// The caller fonts are added to the fonts `to_pdf` resolves from, in the
    /// order [`Self::to_pdf_with_fonts`] lists, under the font switches of
    /// `options`. [`Self::layout_with_fonts`] is the layout limited to the
    /// caller and embedded fonts.
    pub fn to_pdf_with_fonts_and_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Vec<u8>> {
        let layout = self.layout_for_render(font_files, options)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout.layout)?)
    }

    /// Save the document as a PDF file.
    pub fn save_pdf<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_pdf_with_options(path, RenderOptions::default())
    }

    /// Save the selected revision view as a PDF file.
    pub fn save_pdf_with_options<P: AsRef<Path>>(
        &self,
        path: P,
        options: RenderOptions,
    ) -> Result<()> {
        let pdf_bytes = self.to_pdf_with_options(options)?;
        std::fs::write(path, pdf_bytes)?;
        Ok(())
    }

    /// Convert the document to a complete HTML document string.
    ///
    /// The export holds the body only: comments, footnotes, endnotes,
    /// headers and footers are left out (`oxoffice docx convert -t html|md`
    /// appends them after the body). Equations are written as MathML in
    /// HTML and LaTeX in Markdown, internal links point at an anchor on
    /// their bookmark.
    pub fn to_html(&self) -> String {
        self.to_html_with_diagnostics().0
    }

    /// Convert to HTML and return the chart projection and equation
    /// conversion diagnostics separately.
    pub fn to_html_with_diagnostics(&self) -> (String, Vec<String>) {
        let (html, mut diagnostics, equations) = self.html_document_with_equation_diagnostics();
        diagnostics.extend(equations);
        (html, diagnostics)
    }

    /// The complete HTML document, the chart diagnostics, and the equation
    /// diagnostics apart, which MHTML reports.
    pub(crate) fn html_document_with_equation_diagnostics(
        &self,
    ) -> (String, Vec<String>, Vec<String>) {
        let (input, charts, diagnostics) = self.build_html_input();
        let equations = crate::math::ExportEquations::default();
        let html = oxoffice_docx_html::to_html_document_with_equations(
            &input,
            &oxoffice_docx_html::HtmlOptions::default(),
            &charts,
            &equations,
        );
        (html, diagnostics, equations.into_diagnostics())
    }

    /// Convert to an HTML fragment without the document wrapper.
    ///
    /// The export holds the body only: comments, footnotes, endnotes,
    /// headers and footers are left out (`oxoffice docx convert -t html|md`
    /// appends them after the body). Equations are written as MathML in
    /// HTML and LaTeX in Markdown, internal links point at an anchor on
    /// their bookmark.
    pub fn to_html_fragment(&self) -> String {
        self.to_html_fragment_with_diagnostics().0
    }

    /// Convert to an HTML fragment and return the chart and equation
    /// diagnostics separately.
    pub fn to_html_fragment_with_diagnostics(&self) -> (String, Vec<String>) {
        let (input, charts, mut diagnostics) = self.build_html_input();
        let equations = crate::math::ExportEquations::default();
        let html = oxoffice_docx_html::to_html_fragment_with_equations(
            &input,
            &oxoffice_docx_html::HtmlOptions::default(),
            &charts,
            &equations,
        );
        diagnostics.extend(equations.into_diagnostics());
        (html, diagnostics)
    }

    /// Convert to Markdown.
    ///
    /// The export holds the body only: comments, footnotes, endnotes,
    /// headers and footers are left out (`oxoffice docx convert -t html|md`
    /// appends them after the body). Equations are written as MathML in
    /// HTML and LaTeX in Markdown, internal links point at an anchor on
    /// their bookmark.
    pub fn to_markdown(&self) -> String {
        self.to_markdown_with_diagnostics().0
    }

    /// Convert to Markdown and return the chart and equation diagnostics
    /// separately.
    pub fn to_markdown_with_diagnostics(&self) -> (String, Vec<String>) {
        let (input, charts, mut diagnostics) = self.build_html_input();
        let equations = crate::math::ExportEquations::default();
        let markdown = oxoffice_docx_html::to_markdown_with_equations(&input, &charts, &equations);
        diagnostics.extend(equations.into_diagnostics());
        (markdown, diagnostics)
    }

    /// Build an HtmlInput from the document's current state.
    fn build_html_input(
        &self,
    ) -> (
        oxoffice_docx_html::HtmlInput,
        std::collections::HashMap<String, oxoffice_docx_html::ChartTable>,
        Vec<String>,
    ) {
        use oxoffice_opc::relationship::rel_types;
        use std::collections::HashMap;

        let mut charts = HashMap::new();
        let mut diagnostics = Vec::new();
        let mut images: HashMap<String, oxoffice_docx_html::ImageData> = HashMap::new();
        let mut hyperlink_urls: HashMap<String, String> = HashMap::new();

        if let Some(rels) = self.package.get_part_rels(&self.doc_part_name) {
            for rel in &rels.items {
                match rel.rel_type.as_str() {
                    t if t == rel_types::IMAGE => {
                        if !relationship_is_internal(rel) {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(data) = self.package.get_part(&part_name) {
                            let content_type = oxoffice_media::resolve(data, &part_name)
                                .content_type()
                                .to_owned();
                            images.insert(
                                rel.id.clone(),
                                oxoffice_docx_html::ImageData {
                                    data: data.to_vec(),
                                    content_type,
                                },
                            );
                        }
                    }
                    t if t == rel_types::CHART && relationship_is_internal(rel) => {
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        let projection =
                            (|| -> std::result::Result<oxoffice_docx_html::ChartTable, String> {
                                let bytes = self
                                    .package
                                    .get_part(&part_name)
                                    .ok_or_else(|| "chart part was not resolved".to_owned())?;
                                let (_, data) = oxoffice_chart::read_chart_data(bytes)
                                    .map_err(|error| error.to_string())?;
                                let title = oxoffice_chart::chart_plain_title(bytes)
                                    .map_err(|error| error.to_string())?;
                                Ok(oxoffice_docx_html::ChartTable {
                                    title,
                                    categories: data.categories,
                                    series: data.series,
                                })
                            })();
                        match projection {
                            Ok(chart) => {
                                if chart.series.iter().any(|(_, values)| {
                                    values.iter().any(|value| !value.is_finite())
                                }) {
                                    diagnostics.push(format!(
                                        "chart relationship {}: non-finite series value omitted",
                                        rel.id
                                    ));
                                }
                                charts.insert(rel.id.clone(), chart);
                            }
                            Err(error) => diagnostics.push(format!(
                                "chart relationship {}: chart table omitted: {error}",
                                rel.id
                            )),
                        }
                    }
                    t if t == rel_types::HYPERLINK
                        && rel.target_mode.as_ref().is_some_and(|m| m == "External") =>
                    {
                        hyperlink_urls.insert(rel.id.clone(), rel.target.clone());
                    }
                    _ => {}
                }
            }
        }

        (
            oxoffice_docx_html::HtmlInput {
                document: (self.document.clone()).into_inner(),
                styles: CT_Styles::clone(&self.styles),
                numbering: self.numbering.clone(),
                images,
                hyperlink_urls,
            },
            charts,
            diagnostics,
        )
    }

    /// Render a single page of the document to PNG bytes.
    ///
    /// # Arguments
    /// * `page_index` - 0-based page index
    /// * `dpi` - Resolution (72 = 1:1, 150 = standard, 300 = high quality)
    pub fn render_page_to_png(&self, page_index: usize, dpi: f64) -> Result<Option<Vec<u8>>> {
        self.render_page_to_png_with_options(page_index, dpi, RenderOptions::default())
    }

    /// Render one page to PNG with the selected revision view.
    pub fn render_page_to_png_with_options(
        &self,
        page_index: usize,
        dpi: f64,
        options: RenderOptions,
    ) -> Result<Option<Vec<u8>>> {
        let layout = self.layout_with_options(options)?;
        Ok(oxoffice_pdf::render_page_to_png(
            &layout.layout,
            page_index,
            dpi,
        ))
    }

    /// Render a single page to PNG using bundled fonts without system font
    /// discovery.
    ///
    /// # Arguments
    /// * `page_index` - 0-based page index
    /// * `dpi` - Resolution (72 = 1:1, 150 = standard, 300 = high quality)
    pub fn render_page_to_png_deterministic(
        &self,
        page_index: usize,
        dpi: f64,
    ) -> Result<Option<Vec<u8>>> {
        self.render_page_to_png_deterministic_with_options(
            page_index,
            dpi,
            RenderOptions::default(),
        )
    }

    /// Render one page to deterministic PNG with the selected revision view.
    pub fn render_page_to_png_deterministic_with_options(
        &self,
        page_index: usize,
        dpi: f64,
        options: RenderOptions,
    ) -> Result<Option<Vec<u8>>> {
        let layout = self.layout_for_policy(options, FontPolicy::BundledOnly)?;
        Ok(oxoffice_pdf::render_page_to_png(
            &layout.layout,
            page_index,
            dpi,
        ))
    }

    /// Render one zero-based page as a self-contained searchable SVG document.
    ///
    /// An index beyond the laid-out document returns `None`. Layout diagnostics
    /// precede path-specific SVG lowering diagnostics in the returned result.
    pub fn render_page_to_svg(&self, page_index: usize) -> Result<Option<crate::SvgRenderResult>> {
        self.render_page_to_svg_with_options(page_index, RenderOptions::default())
    }

    /// Render one page as SVG with the selected revision view.
    pub fn render_page_to_svg_with_options(
        &self,
        page_index: usize,
        options: RenderOptions,
    ) -> Result<Option<crate::SvgRenderResult>> {
        let layout = self.layout_with_options(options)?;
        Ok(crate::svg::render_page(&layout.layout, page_index))
    }

    /// Render one page as SVG using bundled fonts without system font discovery.
    pub fn render_page_to_svg_deterministic(
        &self,
        page_index: usize,
    ) -> Result<Option<crate::SvgRenderResult>> {
        self.render_page_to_svg_deterministic_with_options(page_index, RenderOptions::default())
    }

    /// Render one revision view as deterministic SVG using bundled fonts.
    pub fn render_page_to_svg_deterministic_with_options(
        &self,
        page_index: usize,
        options: RenderOptions,
    ) -> Result<Option<crate::SvgRenderResult>> {
        let layout = self.layout_for_policy(options, FontPolicy::BundledOnly)?;
        Ok(crate::svg::render_page(&layout.layout, page_index))
    }

    /// Render selected zero-based pages to the requested image format.
    pub fn render_pages(
        &self,
        page_indices: &[usize],
        raster_options: oxoffice_pdf::RasterOptions,
    ) -> Result<oxoffice_pdf::RasterOutput> {
        self.render_pages_with_options(page_indices, raster_options, RenderOptions::default())
    }

    /// Render selected zero-based pages with the selected revision view.
    pub fn render_pages_with_options(
        &self,
        page_indices: &[usize],
        raster_options: oxoffice_pdf::RasterOptions,
        options: RenderOptions,
    ) -> Result<oxoffice_pdf::RasterOutput> {
        let layout = self.layout_with_options(options)?;
        Ok(oxoffice_pdf::render_pages(
            &layout.layout,
            page_indices,
            raster_options,
        )?)
    }

    /// Render selected zero-based pages using bundled fonts without system font discovery.
    pub fn render_pages_deterministic(
        &self,
        page_indices: &[usize],
        raster_options: oxoffice_pdf::RasterOptions,
    ) -> Result<oxoffice_pdf::RasterOutput> {
        self.render_pages_deterministic_with_options(
            page_indices,
            raster_options,
            RenderOptions::default(),
        )
    }

    /// Render selected zero-based pages to deterministic images with the selected revision view.
    pub fn render_pages_deterministic_with_options(
        &self,
        page_indices: &[usize],
        raster_options: oxoffice_pdf::RasterOptions,
        options: RenderOptions,
    ) -> Result<oxoffice_pdf::RasterOutput> {
        let layout = self.layout_for_policy(options, FontPolicy::BundledOnly)?;
        Ok(oxoffice_pdf::render_pages(
            &layout.layout,
            page_indices,
            raster_options,
        )?)
    }

    /// Render all pages of the document to PNG bytes.
    pub fn render_all_pages(&self, dpi: f64) -> Result<Vec<Vec<u8>>> {
        self.render_all_pages_with_options(dpi, RenderOptions::default())
    }

    /// Render every page to PNG with the selected revision view.
    pub fn render_all_pages_with_options(
        &self,
        dpi: f64,
        options: RenderOptions,
    ) -> Result<Vec<Vec<u8>>> {
        let layout = self.layout_with_options(options)?;
        Ok(oxoffice_pdf::render_all_pages(&layout.layout, dpi))
    }

    /// Encode one bundle with the font policy of [`Self::layout_for_render`]
    /// and return its owned diagnostics. Caller fonts keep highest priority.
    pub fn to_pdf_with_diagnostics(
        &self,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = self.layout_with_font_policy(font_policy(options), font_files, options)?;
        let bytes = oxoffice_pdf::render_to_pdf_checked(&layout.layout)?;
        Ok((bytes, layout.layout.diagnostics.clone()))
    }

    /// Paint one PNG page from one bundle without rerendering for diagnostics.
    pub fn render_page_to_png_with_diagnostics(
        &self,
        page_index: usize,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<(Option<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = self.layout_with_font_policy(font_policy(options), font_files, options)?;
        let bytes = oxoffice_pdf::render_page_to_png(&layout.layout, page_index, dpi);
        Ok((bytes, layout.layout.diagnostics.clone()))
    }

    /// Paint every PNG page once and return the same bundle's ordered facts.
    pub fn render_all_pages_with_diagnostics(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<(Vec<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = self.layout_with_font_policy(font_policy(options), font_files, options)?;
        let bytes = oxoffice_pdf::render_all_pages(&layout.layout, dpi);
        Ok((bytes, layout.layout.diagnostics.clone()))
    }

    /// Paint explicit or all pages from one configured bundle, preserving TIFF.
    pub fn render_pages_with_diagnostics(
        &self,
        page_indices: Option<&[usize]>,
        raster_options: oxoffice_pdf::RasterOptions,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<(oxoffice_pdf::RasterOutput, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = self.layout_with_font_policy(font_policy(options), font_files, options)?;
        let all_pages;
        let selected = match page_indices {
            Some(selected) => selected,
            None => {
                all_pages = (0..layout.layout.pages.len()).collect::<Vec<_>>();
                &all_pages
            }
        };
        let bytes = oxoffice_pdf::render_pages(&layout.layout, selected, raster_options)?;
        Ok((bytes, layout.layout.diagnostics.clone()))
    }

    /// Render one zero-based PNG page with caller fonts, under the font
    /// policy of [`Self::layout_for_render`].
    pub fn render_page_to_png_with_fonts_and_options(
        &self,
        page_index: usize,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Option<Vec<u8>>> {
        let layout = self.layout_for_render(font_files, options)?;
        Ok(oxoffice_pdf::render_page_to_png(
            &layout.layout,
            page_index,
            dpi,
        ))
    }

    /// Render one searchable SVG page with caller fonts, under the font
    /// policy of [`Self::layout_for_render`].
    pub fn render_page_to_svg_with_fonts_and_options(
        &self,
        page_index: usize,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Option<crate::SvgRenderResult>> {
        let layout = self.layout_for_render(font_files, options)?;
        Ok(crate::svg::render_page(&layout.layout, page_index))
    }

    /// Render selected pages with caller fonts, under the font policy of
    /// [`Self::layout_for_render`].
    pub fn render_pages_with_fonts_and_options(
        &self,
        page_indices: &[usize],
        raster_options: oxoffice_pdf::RasterOptions,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<oxoffice_pdf::RasterOutput> {
        let layout = self.layout_for_render(font_files, options)?;
        Ok(oxoffice_pdf::render_pages(
            &layout.layout,
            page_indices,
            raster_options,
        )?)
    }

    /// Render every PNG page with caller fonts, under the font policy of
    /// [`Self::layout_for_render`].
    pub fn render_all_pages_with_fonts_and_options(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: RenderOptions,
    ) -> Result<Vec<Vec<u8>>> {
        let layout = self.layout_for_render(font_files, options)?;
        Ok(oxoffice_pdf::render_all_pages(&layout.layout, dpi))
    }

    /// Return a cloned positioned page from the cached default layout.
    ///
    /// The fonts are those of [`Self::layout`]; set the font switches of
    /// [`RenderOptions`] through [`Self::layout_page_with_options`].
    /// `page_index` is zero-based. An index beyond the document returns `None`.
    pub fn layout_page(&self, page_index: usize) -> Result<Option<oxoffice_layout::PageFrame>> {
        self.layout_page_with_options(page_index, RenderOptions::default())
    }

    /// Return one positioned page from the selected revision view.
    pub fn layout_page_with_options(
        &self,
        page_index: usize,
        options: RenderOptions,
    ) -> Result<Option<oxoffice_layout::PageFrame>> {
        let layout = self.layout_with_options(options)?;
        Ok(layout
            .layout
            .pages
            .get(page_index)
            .map(|page| page.as_ref().clone()))
    }

    /// Build a LayoutInput from the document's current state.
    fn layout_story_body(&self, story: &StoryId) -> Result<oxoffice_docx_oxml::document::CT_Body> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let xml = source.xml.as_ref();
        let mut projected = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#.to_vec();
        for item in direct_story_content_items(xml, &owner)? {
            let scope = story_namespace_scope_at(xml, item.full.start)?;
            projected.extend(close_content_fragment_namespaces(&xml[item.full], &scope)?);
        }
        projected.extend(b"</w:body></w:document>");
        Ok(oxoffice_docx_oxml::document::CT_Document::from_xml(&projected)?.body)
    }

    pub(crate) fn build_layout_input(&self) -> Result<oxoffice_docx_layout::LayoutInput> {
        let embedded_fonts = self.extract_embedded_fonts()?;
        let modern_compatibility = self.settings.as_ref().is_some_and(|settings| {
            settings.compatibility_settings().iter().any(|setting| {
                setting.name == "compatibilityMode"
                    && setting.uri == "http://schemas.microsoft.com/office/word"
                    && setting
                        .value
                        .trim()
                        .parse::<u32>()
                        .is_ok_and(|mode| mode >= 15)
            })
        });
        use oxoffice_docx_layout::{ImageData, LayoutInput};
        use oxoffice_opc::relationship::rel_types;
        use std::collections::HashMap;

        let mut story_part_names = HashMap::from([(
            oxoffice_docx_layout::WordStory::Document,
            self.doc_part_name.clone(),
        )]);
        let mut headers: HashMap<String, CT_HdrFtr> = HashMap::new();
        let mut footers: HashMap<String, CT_HdrFtr> = HashMap::new();
        let mut images: HashMap<String, ImageData> = HashMap::new();
        let mut charts = HashMap::new();
        let mut hyperlink_urls: HashMap<String, String> = HashMap::new();
        let mut footnotes = None;
        let mut endnotes = None;
        let active_header_footer_ids = self.header_footer_rel_ids_for_layout();
        let even_headers_enabled = self.even_headers_enabled();

        // Extract embedded fonts from the DOCX package
        let fonts = Vec::new();

        let insert_part_images =
            |images: &mut HashMap<String, ImageData>, part_rel_id: &str, part_name: &str| {
                let Some(part_relationships) = self.package.get_part_rels(part_name) else {
                    return;
                };
                for image_relationship in part_relationships.items.iter().filter(|item| {
                    item.rel_type == rel_types::IMAGE && relationship_is_internal(item)
                }) {
                    let image_part =
                        OpcPackage::resolve_rel_target(part_name, &image_relationship.target);
                    if let Some(data) = self.package.get_part(&image_part) {
                        images.insert(
                            format!("{part_rel_id}\0{}", image_relationship.id),
                            ImageData {
                                data: data.to_vec(),
                                content_type: oxoffice_media::resolve(data, &image_part)
                                    .content_type()
                                    .to_owned(),
                            },
                        );
                    }
                }
            };

        if let Some(rels) = self.package.get_part_rels(&self.doc_part_name) {
            for rel in &rels.items {
                match rel.rel_type.as_str() {
                    t if t == rel_types::HEADER => {
                        if !active_header_footer_ids.contains(&rel.id)
                            || !relationship_is_internal(rel)
                        {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(xml) = self.package.get_part(&part_name)
                            && let Ok(hf) = CT_HdrFtr::from_xml(xml)
                        {
                            story_part_names.insert(
                                oxoffice_docx_layout::WordStory::Header {
                                    relationship_id: rel.id.clone(),
                                },
                                part_name.clone(),
                            );
                            headers.insert(rel.id.clone(), hf);
                        }
                        insert_part_images(&mut images, &rel.id, &part_name);
                    }
                    t if t == rel_types::FOOTER => {
                        if !active_header_footer_ids.contains(&rel.id)
                            || !relationship_is_internal(rel)
                        {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(xml) = self.package.get_part(&part_name)
                            && let Ok(hf) = CT_HdrFtr::from_xml(xml)
                        {
                            story_part_names.insert(
                                oxoffice_docx_layout::WordStory::Footer {
                                    relationship_id: rel.id.clone(),
                                },
                                part_name.clone(),
                            );
                            footers.insert(rel.id.clone(), hf);
                        }
                        insert_part_images(&mut images, &rel.id, &part_name);
                    }
                    t if t == rel_types::IMAGE => {
                        if !relationship_is_internal(rel) {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(data) = self.package.get_part(&part_name) {
                            let content_type = oxoffice_media::resolve(data, &part_name)
                                .content_type()
                                .to_owned();
                            images.insert(
                                rel.id.clone(),
                                ImageData {
                                    data: data.to_vec(),
                                    content_type,
                                },
                            );
                        }
                    }
                    t if t == rel_types::CHART => {
                        let chart = if !relationship_is_internal(rel) {
                            Err(format!("non-internal target {}", rel.target))
                        } else {
                            let part_name =
                                OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                            match self.package.get_part(&part_name) {
                                Some(xml) => {
                                    CT_ChartSpace::from_xml(xml).map(Box::new).map_err(|error| {
                                        format!("malformed target {part_name}: {error}")
                                    })
                                }
                                None => Err(format!("missing target {part_name}")),
                            }
                        };
                        charts.insert(rel.id.clone(), chart);
                    }
                    t if t == rel_types::HYPERLINK => {
                        if rel.target_mode.as_ref().is_some_and(|m| m == "External") {
                            hyperlink_urls.insert(rel.id.clone(), rel.target.clone());
                        }
                    }
                    t if t == rel_types::FOOTNOTES => {
                        if !relationship_is_internal(rel) {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(xml) = self.package.get_part(&part_name) {
                            footnotes =
                                oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml).ok();
                            if let Some(stream) = &footnotes {
                                for note in &stream.footnotes {
                                    story_part_names.insert(
                                        oxoffice_docx_layout::WordStory::Footnote { id: note.id },
                                        part_name.clone(),
                                    );
                                }
                            }
                        }
                    }
                    t if t == rel_types::ENDNOTES => {
                        if !relationship_is_internal(rel) {
                            continue;
                        }
                        let part_name =
                            OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                        if let Some(xml) = self.package.get_part(&part_name) {
                            endnotes =
                                oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml).ok();
                            if let Some(stream) = &endnotes {
                                for note in &stream.footnotes {
                                    story_part_names.insert(
                                        oxoffice_docx_layout::WordStory::Endnote { id: note.id },
                                        part_name.clone(),
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let chart_theme = if self.theme_dirty {
            self.theme.clone()
        } else {
            self.package
                .get_part_rels(&self.doc_part_name)
                .and_then(|relationships| {
                    relationships.items.iter().rev().find(|relationship| {
                        relationship.rel_type == rel_types::THEME
                            && relationship_is_internal(relationship)
                    })
                })
                .map(|relationship| {
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
                })
                .as_deref()
                .and_then(|part_name| self.package.get_part(part_name))
                .and_then(|xml| oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(xml).ok())
                .or_else(|| self.theme.clone())
        }
        .unwrap_or_else(oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default);
        let theme = Some(oxoffice_docx_oxml::theme::Theme::from(&chart_theme));

        let mut document = self.document.clone();
        if !even_headers_enabled {
            for content in &mut document.body.content {
                if let BodyContent::Paragraph(paragraph) = content
                    && let Some(section) = paragraph
                        .properties
                        .as_mut()
                        .and_then(|properties| properties.sect_pr.as_mut())
                {
                    section
                        .header_refs
                        .retain(|reference| reference.hdr_ftr_type != HdrFtrType::Even);
                    section
                        .footer_refs
                        .retain(|reference| reference.hdr_ftr_type != HdrFtrType::Even);
                }
            }
            if let Some(section) = document.body.sect_pr.as_mut() {
                section
                    .header_refs
                    .retain(|reference| reference.hdr_ftr_type != HdrFtrType::Even);
                section
                    .footer_refs
                    .retain(|reference| reference.hdr_ftr_type != HdrFtrType::Even);
            }
        }
        materialize_header_footer_inheritance(
            &mut document,
            even_headers_enabled,
            &headers.keys().cloned().collect(),
            &footers.keys().cloned().collect(),
        );

        if let Some(story) = self.stories().ok().and_then(|stories| {
            stories
                .into_iter()
                .find(|story| story.kind == StoryKind::Body)
        }) && let Ok((source, owner)) = self.story_source_and_owner(&story)
            && bind_text_box_source_owners(
                &mut document.body,
                source.xml.as_ref(),
                source.root_kind,
                owner.full,
            )
            .is_err()
        {
            // Binding is atomic. Unresolved physical owners retain their caches.
        }

        // Optional drawing binding has already chosen its successful/fallback
        // projection. Semantic admission failures must not be swallowed with it.
        prepare_semantic_body_projection(&mut document.body.content)?;
        let mut story_bodies = HashMap::new();
        if let Ok(stories) = self.stories() {
            for (identity, part_name) in &story_part_names {
                let kind = match identity {
                    oxoffice_docx_layout::WordStory::Header { .. } => StoryKind::Header,
                    oxoffice_docx_layout::WordStory::Footer { .. } => StoryKind::Footer,
                    oxoffice_docx_layout::WordStory::Footnote { .. } => StoryKind::Footnote,
                    oxoffice_docx_layout::WordStory::Endnote { .. } => StoryKind::Endnote,
                    _ => continue,
                };
                let story = match identity {
                    oxoffice_docx_layout::WordStory::Footnote { id } => {
                        self.footnote_story(*id).ok().flatten()
                    }
                    oxoffice_docx_layout::WordStory::Endnote { id } => {
                        self.endnote_story(*id).ok().flatten()
                    }
                    _ => stories
                        .iter()
                        .find(|story| story.kind == kind && &story.part_name == part_name)
                        .cloned(),
                };
                if let Some(story) = story
                    && let Ok(mut body) = self.layout_story_body(&story)
                {
                    if let Ok((source, owner)) = self.story_source_and_owner(&story)
                        && bind_text_box_source_owners(
                            &mut body,
                            source.xml.as_ref(),
                            source.root_kind,
                            owner.full,
                        )
                        .is_err()
                    {
                        // The unbound authoritative projection remains renderable.
                    }
                    prepare_semantic_body_projection(&mut body.content)?;
                    story_bodies.insert(identity.clone(), body);
                }
            }
        }

        // Fallback paragraph caches are real layout consumers too, even if an
        // optional authoritative story-body projection could not be acquired.
        for (header, parts) in [(true, &mut headers), (false, &mut footers)] {
            for (id, part) in parts {
                let story = if header {
                    oxoffice_docx_layout::WordStory::Header {
                        relationship_id: id.clone(),
                    }
                } else {
                    oxoffice_docx_layout::WordStory::Footer {
                        relationship_id: id.clone(),
                    }
                };
                if !story_bodies.contains_key(&story) {
                    for paragraph in &mut part.paragraphs {
                        prepare_semantic_paragraph_projection(paragraph)?;
                    }
                }
            }
        }
        for (footnote, notes) in [(true, &mut footnotes), (false, &mut endnotes)] {
            if let Some(notes) = notes {
                for note in &mut notes.footnotes {
                    if note.note_type != oxoffice_docx_oxml::footnotes::NoteType::Normal {
                        continue;
                    }
                    let story = if footnote {
                        oxoffice_docx_layout::WordStory::Footnote { id: note.id }
                    } else {
                        oxoffice_docx_layout::WordStory::Endnote { id: note.id }
                    };
                    if !story_bodies.contains_key(&story) {
                        for paragraph in &mut note.paragraphs {
                            prepare_semantic_paragraph_projection(paragraph)?;
                        }
                    }
                }
            }
        }

        // Rendering may consume a sole effects-only default without changing
        // either authored style part or merging the effects style catalogue.
        let mut layout_styles = CT_Styles::clone(&self.styles);
        if layout_styles.get_default(StyleType::Table).is_none() {
            let effects = self.package.get_part_rels(&self.doc_part_name).and_then(|rels| {
                let mut candidates = rels.items.iter().filter(|rel| {
                    rel.rel_type == "http://schemas.microsoft.com/office/2007/relationships/stylesWithEffects"
                        && relationship_is_internal(rel)
                });
                let rel = candidates.next()?;
                if candidates.next().is_some() {
                    return None;
                }
                let target = OpcPackage::resolve_rel_target(&self.doc_part_name, &rel.target);
                let xml = self.package.get_part(&target)?;
                let parsed = CT_Styles::from_xml(xml).ok()?;
                let mut style_ids = std::collections::HashSet::new();
                if !parsed.styles.iter().all(|style| style_ids.insert(&style.style_id)) {
                    return None;
                }
                // The general projection omits empty style owners. Reject an
                // effects catalogue whose physical owners are not all modeled,
                // rather than accepting an invisible duplicate or default.
                let mut reader = NsReader::from_reader(xml);
                let mut buffer = Vec::new();
                let mut depth = 0usize;
                let mut root_seen = false;
                let mut style_count = 0usize;
                loop {
                    let (namespace, event) = reader.read_resolved_event_into(&mut buffer).ok()?;
                    let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes());
                    let starts_scope = matches!(&event, Event::Start(_));
                    match event {
                        Event::Start(element) | Event::Empty(element) => {
                            if depth == 0 {
                                if root_seen || !word || element.local_name().as_ref() != b"styles" {
                                    return None;
                                }
                                root_seen = true;
                            } else if depth == 1 && word && element.local_name().as_ref() == b"style" {
                                style_count += 1;
                            }
                            // Empty events do not open an ownership level.
                            if starts_scope {
                                depth += 1;
                            }
                        }
                        Event::End(_) => depth = depth.checked_sub(1)?,
                        Event::DocType(_) => return None,
                        Event::Text(text) if depth == 0 && !text.iter().all(u8::is_ascii_whitespace) => return None,
                        Event::Eof => break,
                        _ => {}
                    }
                    buffer.clear();
                }
                (root_seen && depth == 0 && style_count == parsed.styles.len()).then_some(parsed)
            });
            if let Some(effects) = effects {
                let mut defaults = effects
                    .styles
                    .iter()
                    .filter(|style| style.style_type == StyleType::Table && style.is_default);
                if let Some(default) = defaults.next()
                    && !default.style_id.is_empty()
                    && defaults.next().is_none()
                    && default.based_on.is_none()
                    && layout_styles.get_by_id(&default.style_id).is_none()
                    && effects
                        .styles
                        .iter()
                        .filter(|style| style.style_id == default.style_id)
                        .count()
                        == 1
                {
                    layout_styles.styles.push(default.clone());
                }
            }
        }

        let mut input = LayoutInput {
            block_bookmarks: Default::default(),
            form_fields: false,
            form_paragraphs: None,
            sequence_snapshot: None,
            revision_view: oxoffice_docx_layout::RevisionView::Accepted,
            revision_style: oxoffice_docx_layout::RevisionStyle::Plain,
            document: (document).into_inner(),
            automatic_hyphenation: self
                .settings
                .as_ref()
                .is_some_and(CT_Settings::automatic_hyphenation),
            default_tab_stop: self
                .settings
                .as_ref()
                .and_then(CT_Settings::default_tab_stop),
            clamp_tabs_past_margin: modern_compatibility,
            legacy_table_positioning: !modern_compatibility,
            modern_footnote_layout: modern_compatibility,
            footnote_layout_like_word8: self
                .settings
                .as_ref()
                .and_then(|settings| {
                    settings.compatibility_option(
                        oxoffice_docx_oxml::settings::CompatibilityOption::FootnoteLayoutLikeWW8,
                    )
                })
                .unwrap_or(false),
            mirror_margins: self
                .settings
                .as_ref()
                .and_then(CT_Settings::mirror_margins)
                .unwrap_or(false),
            gutter_at_top: self
                .settings
                .as_ref()
                .and_then(CT_Settings::gutter_at_top)
                .unwrap_or(false),
            do_not_use_html_paragraph_auto_spacing: self
                .settings
                .as_ref()
                .and_then(|settings| {
                    settings
                        .compatibility_option(CompatibilityOption::DoNotUseHTMLParagraphAutoSpacing)
                })
                .unwrap_or(false),
            math_properties: self
                .settings
                .as_ref()
                .and_then(CT_Settings::math_properties)
                .cloned(),
            note_defaults: [
                self.settings
                    .as_ref()
                    .and_then(CT_Settings::footnote_properties)
                    .cloned(),
                self.settings
                    .as_ref()
                    .and_then(CT_Settings::endnote_properties)
                    .cloned(),
            ],
            styles: layout_styles,
            numbering: self.numbering.clone(),
            story_part_names,
            story_bodies,
            headers,
            footers,
            images,
            charts,
            chart_theme,
            chart_color_map: oxoffice_drawing::color::ColorMap::default(),
            core_properties: self.core_properties.as_deref().cloned(),
            hyperlink_urls,
            footnotes,
            endnotes,
            theme,
            fonts,
            embedded_fonts,
        };
        input.sequence_snapshot = match self.navigation_exclusions_for_input(&input) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let mut snapshot = oxoffice_docx_layout::WordSequenceSnapshot::default();
                snapshot.set_navigation_failure(&input, error.to_string())?;
                Some(Arc::new(snapshot))
            }
        };
        Ok(input)
    }

    /// Prepare distinct owned font bodies and all aliases before layout allocation.
    fn extract_embedded_fonts(
        &self,
    ) -> Result<Vec<oxoffice_docx_layout::input::EmbeddedFontSource>> {
        if self.embed_true_type_fonts()? == Some(false) {
            return Ok(Vec::new());
        }
        let Some(table) = &self.font_table else {
            if self.font_table_part_name.is_some() {
                return Err(Error::Other("font table is unmodeled".to_owned()));
            }
            return Ok(Vec::new());
        };
        let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
        let mut by_part = HashMap::new();
        let mut total = 0usize;
        let mut alias_count = 0usize;
        let mut alias_bytes = 0usize;
        for font in table.fonts() {
            for reference in &font.embedded_fonts {
                let (part, body) = self.embedded_font_source(reference)?;
                // An unsupported content type stays in the package and renders
                // with the fallback face. Listing still reports that type.
                if self.embedded_font_storage(&part).is_none() {
                    continue;
                }
                if body.len() > 64 * 1024 * 1024
                    || font.name.len() > 4096
                    || reference.font_key.len() > 4096
                {
                    return Err(Error::Other(
                        "embedded font input exceeds its bound".to_owned(),
                    ));
                }
                let identity = part.to_ascii_lowercase();
                let index = if let Some(&index) = by_part.get(&identity) {
                    index
                } else {
                    total = total
                        .checked_add(body.len())
                        .ok_or_else(|| Error::Other("embedded font size overflow".to_owned()))?;
                    if total > 256 * 1024 * 1024 || groups.len() >= 4096 {
                        return Err(Error::Other(
                            "embedded font input exceeds its aggregate bound".to_owned(),
                        ));
                    }
                    let index = groups.len();
                    groups.push((part, reference.font_key.clone(), Vec::new()));
                    by_part.insert(identity, index);
                    index
                };
                let group = &mut groups[index];
                if group.1 != reference.font_key {
                    return Err(Error::Other(
                        "shared font part has conflicting obfuscation keys".to_owned(),
                    ));
                }
                if !group.2.contains(&font.name) {
                    alias_count += 1;
                    alias_bytes = alias_bytes.checked_add(font.name.len()).ok_or_else(|| {
                        Error::Other("embedded font alias size overflow".to_owned())
                    })?;
                    if alias_count > 16384 || alias_bytes > 4 * 1024 * 1024 {
                        return Err(Error::Other(
                            "embedded font aliases exceed their bound".to_owned(),
                        ));
                    }
                    group.2.push(font.name.clone());
                }
            }
        }
        let mut sources = Vec::new();
        for (part, key, aliases) in groups {
            let body = self.package.get_part(&part).expect("preflighted font part");
            // Unsupported producer bytes remain in the original package and use fallback.
            if let Some(data) = self.embedded_font_face(&part, body, &key) {
                sources.push((aliases, Arc::from(data)));
            }
        }
        Ok(sources)
    }

    /// Load font files from a directory and return them as FontFile entries.
    ///
    /// This is useful for CLI tools that accept a `--font-dir` argument.
    /// Supports `.ttf`, `.otf`, and `.ttc` files.
    pub fn load_fonts_from_dir<P: AsRef<Path>>(dir: P) -> Vec<oxoffice_docx_layout::FontFile> {
        let mut fonts = Vec::new();
        let dir = dir.as_ref();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if (ext == "ttf" || ext == "otf" || ext == "ttc")
                    && let Ok(data) = std::fs::read(&path)
                {
                    let family = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    fonts.push(oxoffice_docx_layout::FontFile { family, data });
                }
            }
        }
        fonts
    }
}

fn annotate_undrawable_images(
    layout: &mut oxoffice_docx_layout::WordLayoutResult,
    input: &oxoffice_docx_layout::LayoutInput,
) {
    let mut messages = Vec::new();
    for (id, story) in layout.picture_draw_owners() {
        let key = story
            .as_ref()
            .map_or_else(|| id.clone(), |story| format!("{story}\0{id}"));
        let Some(image) = input.images.get(&key) else {
            continue;
        };
        let detail = match oxoffice_pdf::classify_image_draw(&image.data, &image.content_type) {
            oxoffice_pdf::ImageDrawClass::UnsupportedFormat { kind } => {
                format!("uses unsupported image format {kind}")
            }
            oxoffice_pdf::ImageDrawClass::CouldNotDecode => "could not be decoded".to_owned(),
            oxoffice_pdf::ImageDrawClass::Drawn | oxoffice_pdf::ImageDrawClass::NotThisDecoder => {
                continue;
            }
        };
        let mut message = format!("picture relationship {id} {detail}");
        if let Some(story) = story {
            message.push_str(&format!(" in related story {story}"));
        }
        if !messages.contains(&message) {
            messages.push(message);
        }
    }
    for message in messages {
        if !layout
            .layout
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == message)
        {
            layout
                .layout
                .diagnostics
                .push(oxoffice_layout::Diagnostic { message });
        }
    }
}
