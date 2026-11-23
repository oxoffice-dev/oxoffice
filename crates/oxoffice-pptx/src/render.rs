//! Rendering and export behind the `render` feature: slides, notes and handouts to PDF and PNG, timelines, text layout, autofit and fit reports.

use super::*;

/// One deterministic evaluated slide frame returned by the native facade.
#[cfg(feature = "render")]
#[derive(Clone, Debug)]
pub struct DeterministicTimelineFrame {
    pub page: PageFrame,
    pub state: EvaluatedFrameState,
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
}

/// How the media-aware renderer handles a media picture's poster.
#[cfg(feature = "render")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaFallbackPolicy {
    PosterFrame,
    DeterministicPlaceholder,
    Fail,
}

/// An audience-handout slide arrangement.
#[cfg(feature = "render")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoutLayout {
    One,
    Two,
    Three,
    Four,
    Six,
    Nine,
}

#[cfg(feature = "render")]
impl HandoutLayout {
    const fn slides_per_page(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Six => 6,
            Self::Nine => 9,
        }
    }
}

/// One deterministic page and its synchronized media playback state.
#[cfg(feature = "render")]
#[derive(Clone, Debug)]
pub struct DeterministicMediaTimelineFrame {
    pub frame: DeterministicTimelineFrame,
    pub media: Vec<EvaluatedMediaState>,
}

/// One text-bearing slide shape as the deterministic renderer lays it out.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq)]
pub struct TextFrameLayout {
    pub slide_index: usize,
    pub shape_id: Option<u32>,
    pub name: Option<String>,
    /// The autofit mode in effect after placeholder inheritance.
    pub autofit: AutofitMode,
    pub layout: oxoffice_pptx_render::ShapeTextLayout,
    /// Ordered facts from this exact shape measurement, not another render.
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
}
/// A checked physical text owner. Shape IDs are metadata, not selectors.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFrameLocation {
    pub owner_part: String,
    pub child_path: Vec<usize>,
    pub cell: Option<(usize, usize)>,
    fingerprint: Vec<u8>,
}

/// Immutable facts from one explicit text authoring operation.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq)]
pub struct AutofitUpdate {
    pub location: TextFrameLocation,
    pub owner_kind: String,
    pub slide_index: Option<usize>,
    pub shape_id: Option<u32>,
    pub name: Option<String>,
    pub mode: AutofitMode,
    pub status: String,
    pub font_scale: Option<u32>,
    pub line_spacing_reduction: Option<u32>,
    pub point_size: Option<u16>,
    pub old_bounds: (i64, i64, i64, i64),
    pub new_bounds: (i64, i64, i64, i64),
    pub changed: bool,
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
}

/// Physical-owner ordered updates from one atomic publication.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq)]
pub struct AutofitRefreshResult {
    pub updates: Vec<AutofitUpdate>,
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
    pub changed: bool,
}

/// Read-only measurement in one instantiated slide or notes context.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq)]
pub struct FitFrameReport {
    pub update: AutofitUpdate,
    pub context: String,
    pub overflow: bool,
    pub needed_scale: Option<u32>,
    pub needed_reduction: Option<u32>,
    pub measured_font_scale: Option<f64>,
    pub current_line_spacing_reduction: Option<f64>,
    pub quantized_trial: Option<oxoffice_pptx_render::AutofitMeasurement>,
    pub persisted_trial: Option<oxoffice_pptx_render::AutofitMeasurement>,
}

/// The explicit authoring solver's read-only results.
#[cfg(feature = "render")]
#[derive(Clone, Debug, PartialEq)]
pub struct FitReport {
    pub frames: Vec<FitFrameReport>,
    pub diagnostics: Vec<oxoffice_layout::Diagnostic>,
}

#[cfg(feature = "render")]
#[derive(Debug, PartialEq)]
struct AutofitContext {
    slide_index: usize,
    context: String,
    shape: oxoffice_pptx_layout::ResolvedShape,
    text: oxoffice_pptx_layout::ResolvedTextBody,
    directions: Vec<Vec<oxoffice_layout::TextDirection>>,
    /// For a table cell, the table whose rows grow to fit their text.
    table: Option<Arc<AutofitTable>>,
}

/// A table as one context renders it, shared by the contexts of its cells.
#[cfg(feature = "render")]
#[derive(Debug, PartialEq)]
struct AutofitTable {
    shape: oxoffice_pptx_layout::ResolvedShape,
    /// One entry per cell in row order.
    directions: Vec<Vec<oxoffice_layout::TextDirection>>,
    page: (f64, f64),
}

/// The zero-based child path as plain ordinals joined by `/`.
#[cfg(feature = "render")]
fn plain_path(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

/// Options of a deck render: the fonts it may use.
///
/// Built from [`Default`] and set field by field, so that new options break
/// no caller.
#[cfg(feature = "render")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PptxRenderOptions {
    /// Use the embedded, caller and bundled fonts only, never a system font:
    /// the render is the same on every machine, and characters those fonts
    /// lack are drawn as empty boxes, which a diagnostic names. By default
    /// (`false`) a system font found by glyph coverage draws them, such as
    /// most Chinese, Japanese and Korean text; no family name ever selects a
    /// system font, and a build without the `system-fonts` feature has none.
    /// The `*_deterministic` helpers always render with the bundled fonts only.
    pub bundled_fonts_only: bool,
}

#[cfg(feature = "render")]
impl PptxRenderOptions {
    /// These options with `bundled_fonts_only` set.
    pub const fn with_bundled_fonts_only(mut self, bundled_fonts_only: bool) -> Self {
        self.bundled_fonts_only = bundled_fonts_only;
        self
    }
}

#[cfg(feature = "render")]
#[derive(Debug, PartialEq)]
struct AutofitOwner {
    update: AutofitUpdate,
    empty: bool,
    unsupported: Option<String>,
    contexts: Vec<AutofitContext>,
}

impl Presentation {
    /// Resolves and lays out the current presentation with deterministic fonts.
    #[cfg(feature = "render")]
    pub fn render_deterministic(&self) -> Result<(RenderInput, LayoutResult)> {
        assemble_render_input(&self.staged_package(false)?, &[], false)
    }

    /// Resolves and lays out the presentation with caller fonts over the
    /// embedded and bundled ones, and with a system font found by glyph
    /// coverage for the characters they lack, unless `options` asks for the
    /// bundled fonts only.
    #[cfg(feature = "render")]
    pub fn render_with_options(
        &self,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(RenderInput, LayoutResult)> {
        assemble_render_input(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            !options.bundled_fonts_only,
        )
    }

    /// Resolve and lay out caller fonts over deterministic bundled fallback.
    #[cfg(feature = "render")]
    pub fn render_deterministic_with_fonts(
        &self,
        font_files: &[(&str, &[u8])],
    ) -> Result<(RenderInput, LayoutResult)> {
        assemble_render_input(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            false,
        )
    }

    /// Encodes selected slides from one caller-font layout, retaining owned diagnostics.
    #[cfg(feature = "render")]
    pub fn render_slides_with_diagnostics(
        &self,
        slide_indices: &[usize],
        options: RasterOptions,
        font_files: &[(&str, &[u8])],
        render_options: PptxRenderOptions,
    ) -> Result<(RasterOutput, Vec<oxoffice_layout::Diagnostic>)> {
        validate_export_png_pages(&[], options.dpi)?;
        if slide_indices.is_empty() {
            return Err(render_failure("raster slide selection is empty"));
        }
        let mut seen = HashSet::new();
        for &index in slide_indices {
            self.require_slide_index(index)?;
            if !seen.insert(index) {
                return Err(render_failure(
                    "raster slide selection contains duplicate indices",
                ));
            }
        }
        if let RasterFormat::Jpeg { quality } = options.format
            && !(1..=100).contains(&quality)
        {
            return Err(render_failure("JPEG quality must be in 1..=100"));
        }
        let (width, height) = self
            .slide_size()
            .unwrap_or((Emu(12_192_000), Emu(6_858_000)));
        let scale = options.dpi / 914_400.0;
        let pixels = ((width.0 as f64 * scale).ceil() as u64)
            .checked_mul((height.0 as f64 * scale).ceil() as u64)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_mul(slide_indices.len() as u64));
        if width.0 <= 0
            || height.0 <= 0
            || pixels.is_none_or(|n| n > MAX_PRESENTATION_EXPORT_RASTER_BYTES)
        {
            return Err(render_failure(
                "raster selection exceeds the decoded-output limit",
            ));
        }
        let (_, layout) = self.render_with_options(font_files, render_options)?;
        let pages = slide_indices
            .iter()
            .map(|index| layout.pages[*index].clone())
            .collect::<Vec<_>>();
        validate_export_png_pages(&pages, options.dpi)?;
        let output = oxoffice_pdf::render_pages(&layout, slide_indices, options)
            .map_err(|error| render_failure(error.to_string()))?;
        Ok((output, layout.diagnostics))
    }

    /// Render caller-selected fonts to a checked deterministic PDF.
    #[cfg(feature = "render")]
    pub fn to_pdf_deterministic_with_fonts(&self, font_files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
        let (_, layout) = self.render_deterministic_with_fonts(font_files)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Encode the deck to PDF with caller fonts and `options`, retaining owned diagnostics.
    #[cfg(feature = "render")]
    pub fn to_pdf_with_diagnostics(
        &self,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let (_, layout) = self.render_with_options(font_files, options)?;
        let bytes = oxoffice_pdf::render_to_pdf_checked(&layout)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Paint one slide and retain the same successful bundle's facts.
    #[cfg(feature = "render")]
    pub fn render_slide_to_png_with_diagnostics(
        &self,
        slide_index: usize,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Option<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let (_, layout) = self.render_with_options(font_files, options)?;
        let bytes = render_export_png(&layout, slide_index, dpi)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Paint all slides once with owned ordered diagnostics.
    #[cfg(feature = "render")]
    pub fn render_all_slides_with_diagnostics(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let (_, layout) = self.render_with_options(font_files, options)?;
        let bytes = render_export_pngs(&layout, dpi)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Render caller-selected fonts to the requested archival profile.
    #[cfg(feature = "render")]
    pub fn to_pdfa_deterministic_with_fonts(
        &self,
        profile: oxoffice_pdf::PdfConformance,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<u8>> {
        let (_, layout) = self.render_deterministic_with_fonts(font_files)?;
        Ok(oxoffice_pdf::render_to_pdf_with_options(
            &layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?)
    }

    /// Encode one caller-selected archival bundle and retain its owned diagnostics.
    #[cfg(feature = "render")]
    pub fn to_pdfa_with_diagnostics(
        &self,
        profile: oxoffice_pdf::PdfConformance,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let (_, layout) = self.render_with_options(font_files, options)?;
        let bytes = oxoffice_pdf::render_to_pdf_with_options(
            &layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?;
        Ok((bytes, layout.diagnostics))
    }

    /// Rasterize one slide from the caller-selected bundle.
    #[cfg(feature = "render")]
    pub fn slide_png_deterministic_with_fonts(
        &self,
        slide_index: usize,
        dpi: f64,
        font_files: &[(&str, &[u8])],
    ) -> Result<Option<Vec<u8>>> {
        let (_, layout) = self.render_deterministic_with_fonts(font_files)?;
        render_export_png(&layout, slide_index, dpi)
    }

    /// Rasterize all slides from one caller-selected bundle.
    #[cfg(feature = "render")]
    pub fn slide_pngs_deterministic_with_fonts(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<Vec<u8>>> {
        let (_, layout) = self.render_deterministic_with_fonts(font_files)?;
        render_export_pngs(&layout, dpi)
    }

    /// Render caller-selected speaker notes to checked PDF.
    #[cfg(feature = "render")]
    pub fn to_notes_pdf_deterministic_with_fonts(
        &self,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<u8>> {
        let layout = render_notes_pages(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            false,
        )?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Rasterize caller-selected speaker notes from one prepared manager.
    #[cfg(feature = "render")]
    pub fn notes_page_pngs_deterministic_with_fonts(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<Vec<u8>>> {
        let layout = render_notes_pages(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            false,
        )?;
        render_export_pngs(&layout, dpi)
    }

    /// Render caller-selected handout pages to checked PDF.
    #[cfg(feature = "render")]
    pub fn to_handout_pdf_deterministic_with_fonts(
        &self,
        arrangement: HandoutLayout,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<u8>> {
        let layout = render_handout_pages(
            &self.staged_package(false)?,
            arrangement,
            &caller_font_files(font_files),
            false,
        )?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Rasterize caller-selected handout pages from one prepared manager.
    #[cfg(feature = "render")]
    pub fn handout_page_pngs_deterministic_with_fonts(
        &self,
        arrangement: HandoutLayout,
        dpi: f64,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<Vec<u8>>> {
        let layout = render_handout_pages(
            &self.staged_package(false)?,
            arrangement,
            &caller_font_files(font_files),
            false,
        )?;
        render_export_pngs(&layout, dpi)
    }

    /// Encode the prepared export and return only that bundle's owned diagnostics.
    #[cfg(feature = "render")]
    pub fn to_notes_pdf_with_diagnostics(
        &self,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = render_notes_pages(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            !options.bundled_fonts_only,
        )?;
        let bytes = oxoffice_pdf::render_to_pdf_checked(&layout)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Encode the prepared export and return only that bundle's owned diagnostics.
    #[cfg(feature = "render")]
    pub fn render_all_notes_with_diagnostics(
        &self,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = render_notes_pages(
            &self.staged_package(false)?,
            &caller_font_files(font_files),
            !options.bundled_fonts_only,
        )?;
        let bytes = render_export_pngs(&layout, dpi)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Encode the prepared export and return only that bundle's owned diagnostics.
    #[cfg(feature = "render")]
    pub fn to_handout_pdf_with_diagnostics(
        &self,
        arrangement: HandoutLayout,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<u8>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = render_handout_pages(
            &self.staged_package(false)?,
            arrangement,
            &caller_font_files(font_files),
            !options.bundled_fonts_only,
        )?;
        let bytes = oxoffice_pdf::render_to_pdf_checked(&layout)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Encode the prepared export and return only that bundle's owned diagnostics.
    #[cfg(feature = "render")]
    pub fn render_all_handouts_with_diagnostics(
        &self,
        arrangement: HandoutLayout,
        dpi: f64,
        font_files: &[(&str, &[u8])],
        options: PptxRenderOptions,
    ) -> Result<(Vec<Vec<u8>>, Vec<oxoffice_layout::Diagnostic>)> {
        let layout = render_handout_pages(
            &self.staged_package(false)?,
            arrangement,
            &caller_font_files(font_files),
            !options.bundled_fonts_only,
        )?;
        let bytes = render_export_pngs(&layout, dpi)?;
        Ok((bytes, layout.diagnostics))
    }

    /// Evaluates and renders one slide at an explicit slide-local timestamp.
    ///
    /// `outgoing_slide_index` supplies the zero-based previous slide for a
    /// two-slide transition. Static rendering remains on its independent path.
    #[cfg(feature = "render")]
    pub fn render_timeline_deterministic(
        &self,
        slide_index: usize,
        position: TimelinePosition,
        outgoing_slide_index: Option<usize>,
    ) -> Result<DeterministicTimelineFrame> {
        self.render_timeline_deterministic_inner(
            slide_index,
            position,
            outgoing_slide_index,
            None,
            &[],
        )
        .map(|(frame, _)| frame)
    }

    /// Evaluates one deterministic page and synchronized media playback state.
    #[cfg(feature = "render")]
    pub fn render_media_timeline_deterministic(
        &self,
        slide_index: usize,
        position: TimelinePosition,
        outgoing_slide_index: Option<usize>,
        fallback_policy: MediaFallbackPolicy,
    ) -> Result<DeterministicMediaTimelineFrame> {
        let (frame, media) = self.render_timeline_deterministic_inner(
            slide_index,
            position,
            outgoing_slide_index,
            Some(fallback_policy),
            &[],
        )?;
        Ok(DeterministicMediaTimelineFrame { frame, media })
    }

    /// Render a timeline sample with caller-selected fonts and bundled fallback.
    #[cfg(feature = "render")]
    pub fn render_timeline_deterministic_with_fonts(
        &self,
        slide_index: usize,
        position: TimelinePosition,
        outgoing_slide_index: Option<usize>,
        font_files: &[(&str, &[u8])],
    ) -> Result<DeterministicTimelineFrame> {
        self.render_timeline_deterministic_inner(
            slide_index,
            position,
            outgoing_slide_index,
            None,
            &caller_font_files(font_files),
        )
        .map(|(frame, _)| frame)
    }

    #[cfg(feature = "render")]
    fn render_timeline_deterministic_inner(
        &self,
        slide_index: usize,
        position: TimelinePosition,
        outgoing_slide_index: Option<usize>,
        fallback_policy: Option<MediaFallbackPolicy>,
        font_files: &[oxoffice_layout::FontFile],
    ) -> Result<(DeterministicTimelineFrame, Vec<EvaluatedMediaState>)> {
        let package = self.staged_package(false)?;
        let mut assembly =
            prepare_render_context(&package, fallback_policy.is_some(), font_files, false)?;
        // A detached sample starts with fresh glyph identities, unlike an animation sequence.
        assembly.font_manager = configured_font_manager(
            &package,
            &self.presentation_part,
            &self.presentation,
            font_files,
            false,
        )?;
        render_prepared_timeline_request(
            &mut assembly,
            TimelineRequest {
                slide_index,
                outgoing_slide_index,
                position,
                fallback_policy,
            },
            self.slides.len(),
        )
    }

    /// Recomputes explicitly flagged physical text owners and publishes once.
    /// Ordinary render and save never invoke this operation.
    #[cfg(feature = "render")]
    pub fn refresh_autofit(
        &mut self,
        font_files: &[(&str, &[u8])],
    ) -> Result<AutofitRefreshResult> {
        let mut package = self.staged_package(true)?;
        for (part, xml) in &package.parts {
            if matches!(
                package.content_types.content_type_for(part),
                Some(
                    content_types::SLIDE
                        | content_types::SLIDE_LAYOUT
                        | content_types::SLIDE_MASTER
                        | content_types::NOTES_SLIDE
                        | content_types::NOTES_MASTER
                )
            ) {
                validate_autofit_authoring_namespaces(xml, part)?;
            }
        }
        let (mut owners, _) = autofit_inventory(self, &package, None, false)?;
        let mut fonts = authoring_fonts(self, font_files)?;
        let mut plans = Vec::new();
        for owner in &mut owners {
            if owner.update.mode == AutofitMode::None {
                continue;
            }
            if owner.empty {
                continue;
            }
            if let Some(reason) = &owner.unsupported {
                return Err(autofit_failure(&owner.update.location, reason));
            }
            if owner.contexts.is_empty() {
                return Err(autofit_failure(
                    &owner.update.location,
                    "shared flagged owner has no measured context",
                ));
            }
            match owner.update.mode {
                AutofitMode::Normal => {
                    let pair = select_autofit_pair(owner, &mut fonts)?.ok_or_else(|| {
                        autofit_failure(
                            &owner.update.location,
                            "text does not fit at the admitted floor",
                        )
                    })?;
                    owner.update.font_scale = Some(pair.0);
                    owner.update.line_spacing_reduction = Some(pair.1);
                    plans.push((owner.update.location.clone(), AuthoringChange::Normal(pair)));
                }
                AutofitMode::Shape => {
                    let mut bounds = None;
                    for context in &owner.contexts {
                        let next = shape_autofit_bounds(owner, context, &mut fonts)?;
                        if bounds.is_some_and(|old| old != next) {
                            return Err(autofit_failure(
                                &owner.update.location,
                                "shared contexts require different local bounds",
                            ));
                        }
                        bounds = Some(next);
                    }
                    owner.update.new_bounds = bounds.expect("nonempty contexts");
                    plans.push((
                        owner.update.location.clone(),
                        AuthoringChange::Shape(owner.update.new_bounds),
                    ));
                }
                AutofitMode::None => {}
            }
        }
        for (location, change) in &plans {
            let owner = owners
                .iter_mut()
                .find(|o| o.update.location == *location)
                .expect("planned owner");
            let diagnostics = authoring_font_diagnostics(owner, &mut fonts, Some(change))?;
            owner.update.diagnostics.extend(diagnostics);
        }
        // No part is touched until all owners and contexts admit their answers.
        for (location, change) in &plans {
            let changed = apply_authoring_change(&mut package, location, change)?;
            let owner = owners
                .iter_mut()
                .find(|owner| owner.update.location == *location)
                .expect("planned owner");
            owner.update.changed = changed;
            if changed {
                owner.update.status = "updated".to_owned();
            }
        }
        if plans
            .iter()
            .any(|(_, change)| matches!(change, AuthoringChange::Shape(_)))
        {
            // Reopen the complete staged source, including every other planned
            // owner. Cached pre-change slide records cannot validate new bounds.
            let mut bytes = Cursor::new(Vec::new());
            package.write_to(&mut bytes)?;
            let candidate = Self::from_bytes(bytes.get_ref())?;
            let (proposed, _) = autofit_inventory(&candidate, &package, None, false)?;
            for (location, change) in &plans {
                if !matches!(change, AuthoringChange::Shape(_)) {
                    continue;
                }
                let original = owners
                    .iter()
                    .find(|owner| owner.update.location == *location)
                    .expect("planned physical owner");
                let current = proposed
                    .iter()
                    .find(|owner| {
                        owner.update.location.owner_part == location.owner_part
                            && owner.update.location.child_path == location.child_path
                            && owner.update.location.cell == location.cell
                    })
                    .ok_or_else(|| {
                        autofit_failure(location, "proposed physical owner is missing")
                    })?;
                if current.unsupported.is_some()
                    || current
                        .contexts
                        .iter()
                        .map(|context| &context.context)
                        .collect::<Vec<_>>()
                        != original
                            .contexts
                            .iter()
                            .map(|context| &context.context)
                            .collect::<Vec<_>>()
                {
                    return Err(autofit_failure(location, "proposed owner contexts changed"));
                }
                for context in &current.contexts {
                    if !oxoffice_pptx_render::authoring_text_fits_at_anchor(
                        &context.shape,
                        &context.text,
                        &mut fonts,
                        context.slide_index + 1,
                        &context.directions,
                    )
                    .map_err(|error| autofit_failure(location, error))?
                    {
                        return Err(autofit_failure(
                            location,
                            "proposed shape bounds do not contain the physical text",
                        ));
                    }
                }
            }
        }
        let changed = owners.iter().any(|owner| owner.update.changed);
        if changed {
            self.publish_authoring_package(package)?;
        }
        let mut seen_font_facts = HashSet::new();
        let diagnostics = owners
            .iter()
            .flat_map(|owner| owner.update.diagnostics.iter().cloned())
            .filter(|diagnostic| keep_authoring_font_fact(diagnostic, &mut seen_font_facts))
            .collect();
        Ok(AutofitRefreshResult {
            updates: owners.into_iter().map(|owner| owner.update).collect(),
            diagnostics,
            changed,
        })
    }

    /// Returns all instantiated authoring contexts without changing the package.
    #[cfg(feature = "render")]
    pub fn fit_report(&self, font_files: &[(&str, &[u8])]) -> Result<FitReport> {
        let package = self.staged_package(false)?;
        let (owners, duplicates) = autofit_inventory(self, &package, None, true)?;
        let mut fonts = authoring_fonts(self, font_files)?;
        let mut frames = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_font_facts = HashSet::new();
        // Retain each shared owner with its result so pointer identity cannot be reused.
        let mut grown_tables: HashMap<(*const AutofitTable, usize), (Arc<AutofitTable>, bool)> =
            HashMap::new();
        for mut owner in owners {
            let pair = if owner.empty || owner.unsupported.is_some() {
                None
            } else {
                select_autofit_pair(&owner, &mut fonts)?
            };
            if !owner.empty && owner.unsupported.is_none() {
                owner.update.diagnostics = authoring_font_diagnostics(&owner, &mut fonts, None)?;
                diagnostics.extend(
                    owner
                        .update
                        .diagnostics
                        .iter()
                        .filter(|diagnostic| {
                            keep_authoring_font_fact(diagnostic, &mut seen_font_facts)
                        })
                        .cloned(),
                );
            }
            // A repeated shape id is named once on the physical owner that
            // carries it, after the font facts and before its contexts.
            for (part, path, diagnostic) in &duplicates {
                if owner.update.location.owner_part == *part
                    && owner.update.location.child_path == *path
                    && !owner.update.diagnostics.contains(diagnostic)
                {
                    owner.update.diagnostics.push(diagnostic.clone());
                }
            }
            for context in &owner.contexts {
                if owner.empty {
                    frames.push(FitFrameReport {
                        update: owner.update.clone(),
                        context: context.context.clone(),
                        overflow: false,
                        needed_scale: None,
                        needed_reduction: None,
                        measured_font_scale: None,
                        current_line_spacing_reduction: None,
                        quantized_trial: None,
                        persisted_trial: None,
                    });
                    continue;
                }
                if let Some(reason) = &owner.unsupported {
                    let mut update = owner.update.clone();
                    update.status = "unsupported".to_owned();
                    let diagnostic = oxoffice_layout::Diagnostic {
                        message: reason.clone(),
                    };
                    if !update.diagnostics.contains(&diagnostic) {
                        update.diagnostics.push(diagnostic);
                    }
                    frames.push(FitFrameReport {
                        update,
                        context: context.context.clone(),
                        overflow: true,
                        needed_scale: None,
                        needed_reduction: None,
                        measured_font_scale: None,
                        current_line_spacing_reduction: None,
                        quantized_trial: None,
                        persisted_trial: None,
                    });
                    continue;
                }
                let measured = oxoffice_pptx_render::measure_authoring_text(
                    &context.shape,
                    &context.text,
                    &mut fonts,
                    context.slide_index + 1,
                    &context.directions,
                )
                .map_err(|e| autofit_failure(&owner.update.location, e))?;
                let mut update = owner.update.clone();
                if !owner.empty && pair.is_none() {
                    update.status = "unfit".to_owned();
                }
                // A table row grows to fit its cells, so a cell taller than its
                // stored row overflows only when the grown table leaves the page.
                let overflow = match &context.table {
                    Some(table) if !measured.fits && measured.width_fits => {
                        let page_number = context.slide_index + 1;
                        let key = (Arc::as_ptr(table), page_number);
                        let fits = if let Some((_, fits)) = grown_tables.get(&key) {
                            *fits
                        } else {
                            let fits = grown_table_fits_page(table, &mut fonts, page_number)
                                .map_err(|e| autofit_failure(&owner.update.location, e))?;
                            grown_tables.insert(key, (Arc::clone(table), fits));
                            fits
                        };
                        !fits
                    }
                    _ => !measured.fits,
                };
                let trials = pair
                    .map(|pair| {
                        oxoffice_pptx_render::measure_autofit_candidate(
                            &context.shape,
                            &context.text,
                            &mut fonts,
                            context.slide_index + 1,
                            &context.directions,
                            pair,
                        )
                    })
                    .transpose()
                    .map_err(|e| autofit_failure(&owner.update.location, e))?;
                frames.push(FitFrameReport {
                    update,
                    context: context.context.clone(),
                    quantized_trial: trials.as_ref().map(|(q, _)| q.clone()),
                    persisted_trial: trials.map(|(_, s)| s),
                    overflow,
                    needed_scale: pair.map(|p| p.0),
                    needed_reduction: pair.map(|p| p.1),
                    measured_font_scale: Some(measured.font_scale),
                    current_line_spacing_reduction: Some(match context.text.autofit {
                        ResolvedAutofit::Normal {
                            line_spacing_reduction,
                            ..
                        } => line_spacing_reduction.unwrap_or(0.0),
                        _ => 0.0,
                    }),
                });
            }
            if owner.contexts.is_empty() {
                let mut update = owner.update;
                update.status = if owner.empty { "empty" } else { "unsupported" }.to_owned();
                if !owner.empty
                    && let Some(reason) = &owner.unsupported
                {
                    let diagnostic = oxoffice_layout::Diagnostic {
                        message: reason.clone(),
                    };
                    if !update.diagnostics.contains(&diagnostic) {
                        update.diagnostics.push(diagnostic);
                    }
                }
                frames.push(FitFrameReport {
                    update,
                    context: "uninstantiated".to_owned(),
                    overflow: !owner.empty,
                    needed_scale: None,
                    needed_reduction: None,
                    measured_font_scale: None,
                    current_line_spacing_reduction: None,
                    quantized_trial: None,
                    persisted_trial: None,
                });
            }
        }
        for (_, _, diagnostic) in duplicates {
            if !diagnostics.contains(&diagnostic) {
                diagnostics.push(diagnostic);
            }
        }
        frames.sort_by_key(|frame| {
            let context_index = self
                .slides
                .iter()
                .position(|record| {
                    frame.context == record.part_name
                        || frame.context == format!("{}:notes", record.part_name)
                        || record
                            .notes
                            .as_ref()
                            .is_some_and(|notes| frame.context == notes.part_name)
                })
                .unwrap_or(usize::MAX);
            let notes = frame.update.owner_kind.starts_with("notes-");
            (
                context_index,
                notes,
                frame.update.location.owner_part.clone(),
                frame.update.location.child_path.clone(),
                frame.update.location.cell,
            )
        });
        Ok(FitReport {
            frames,
            diagnostics,
        })
    }

    /// Fits one checked physical owner at the largest admitted whole point size.
    #[cfg(feature = "render")]
    pub fn fit_text_at(
        &mut self,
        location: &TextFrameLocation,
        font_family: Option<&str>,
        max_size: u16,
        font_files: &[(&str, &[u8])],
    ) -> Result<AutofitUpdate> {
        if max_size == 0 || max_size > 4000 {
            return Err(autofit_failure(
                location,
                "max_size must be an integer from 1 through 4000",
            ));
        }
        if font_family.is_some_and(|family| family.is_empty()) {
            return Err(autofit_failure(location, "font family must not be empty"));
        }
        let mut package = self.staged_package(true)?;
        validate_autofit_authoring_namespaces(
            required_part(&package, &location.owner_part)?,
            &location.owner_part,
        )?;
        // The owners of a slide part take their contexts from that slide
        // alone, so only that slide is measured.
        let slide_part = self
            .slides
            .iter()
            .any(|slide| slide.part_name == location.owner_part)
            .then_some(location.owner_part.as_str());
        let (owners, _) = autofit_inventory(self, &package, slide_part, false)?;
        let mut owner = owners
            .into_iter()
            .find(|owner| owner.update.location == *location)
            .ok_or_else(|| autofit_failure(location, "stale or foreign physical owner"))?;
        // Debug builds, which run the binding tests, find the same owner in
        // the inventory of every slide when it can be taken.
        #[cfg(debug_assertions)]
        if slide_part.is_some()
            && let Ok((owners, _)) = autofit_inventory(self, &package, None, false)
        {
            let whole = owners
                .into_iter()
                .find(|owner| owner.update.location == *location);
            assert_eq!(
                whole.as_ref(),
                Some(&owner),
                "fit_text measured another owner"
            );
        }
        if let Some(reason) = &owner.unsupported {
            return Err(autofit_failure(location, reason));
        }
        if owner.empty {
            return Ok(owner.update);
        }
        if owner.contexts.is_empty() {
            return Err(autofit_failure(
                location,
                "physical owner has no measured context",
            ));
        }
        let mut fonts = authoring_fonts(self, font_files)?;
        let mut chosen = None;
        for size in (1..=max_size).rev() {
            let mut fits = true;
            for context in &owner.contexts {
                let mut text = context.text.clone();
                restyle_authoring_text(&mut text, font_family, size);
                let trial = oxoffice_pptx_render::measure_authoring_text(
                    &context.shape,
                    &text,
                    &mut fonts,
                    context.slide_index + 1,
                    &context.directions,
                )
                .map_err(|e| autofit_failure(location, e))?;
                if !trial.fits {
                    fits = false;
                    break;
                }
            }
            if fits {
                chosen = Some(size);
                break;
            }
        }
        let chosen =
            chosen.ok_or_else(|| autofit_failure(location, "text does not fit at one point"))?;
        let diagnostics = authoring_font_diagnostics(
            &owner,
            &mut fonts,
            Some(&AuthoringChange::Fit {
                size: chosen,
                family: font_family.map(str::to_owned),
            }),
        )?;
        owner.update.diagnostics.extend(diagnostics);
        owner.update.point_size = Some(chosen);
        owner.update.mode = AutofitMode::None;
        owner.update.changed = apply_authoring_change(
            &mut package,
            location,
            &AuthoringChange::Fit {
                size: chosen,
                family: font_family.map(str::to_owned),
            },
        )?;
        if owner.update.changed {
            owner.update.status = "updated".to_owned();
            self.publish_authoring_package(package)?;
        }
        Ok(owner.update)
    }

    /// Resolves an existing native or Python content path to a checked owner.
    #[cfg(feature = "render")]
    pub fn text_frame_location(
        &self,
        slide_index: usize,
        child_path: &[usize],
        cell: Option<(usize, usize)>,
    ) -> Result<TextFrameLocation> {
        let slide = self
            .slides
            .get(slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: slide_index,
                slide_count: self.slides.len(),
            })?;
        let child = owned_child(
            &slide.slide.common_slide_data.shape_tree.children,
            child_path,
        )
        .ok_or_else(|| render_failure("text frame path is stale"))?;
        if owned_text(child, cell).is_none() {
            return Err(render_failure("content path does not own a text body"));
        }
        Ok(TextFrameLocation {
            owner_part: slide.part_name.clone(),
            child_path: child_path.to_vec(),
            cell,
            fingerprint: shape_ref(child).xml()?,
        })
    }

    #[cfg(feature = "render")]
    fn publish_authoring_package(&mut self, package: OpcPackage) -> Result<()> {
        // The reopened candidate has already validated all changed model
        // roots. Publication happens once, after the complete package reopens.
        *self = self.checked_commit(|staged| {
            let mut package = package.clone();
            let invalidated = staged.package_signatures_invalidated
                || package
                    .package_rels
                    .get_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN)
                    .is_some();
            embedded::persist_invalidated_package_signature(&mut package, invalidated)?;
            let mut candidate = staged.reopen(package)?;
            candidate.embedded_invalidated_signatures =
                staged.embedded_invalidated_signatures.clone();
            candidate.package_signatures_invalidated = invalidated;
            Ok(candidate)
        })?;
        Ok(())
    }

    /// Renders the current presentation to a complete deterministic PDF.
    #[cfg(feature = "render")]
    pub fn to_pdf_deterministic(&self) -> Result<Vec<u8>> {
        let (_, layout) = self.render_deterministic()?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Renders one deterministic slide PNG at `dpi` by zero-based index.
    #[cfg(feature = "render")]
    pub fn slide_png_deterministic(&self, slide_index: usize, dpi: f64) -> Result<Option<Vec<u8>>> {
        let (_, layout) = self.render_deterministic()?;
        render_export_png(&layout, slide_index, dpi)
    }

    /// Renders one deterministic PNG per slide at `dpi`.
    #[cfg(feature = "render")]
    pub fn slide_pngs_deterministic(&self, dpi: f64) -> Result<Vec<Vec<u8>>> {
        let (_, layout) = self.render_deterministic()?;
        render_export_pngs(&layout, dpi)
    }

    /// Lays out the text of every text-bearing slide shape with deterministic fonts.
    ///
    /// Frames come in slide order, then draw order, from the same resolved
    /// slides and line breaker as [`Self::to_pdf_deterministic`] and
    /// [`Self::slide_png_deterministic`]. Only shapes in each slide's own shape
    /// tree whose text draws a visible character are reported, so layout and
    /// master shapes, SmartArt and table cells are left out. `width_factor`
    /// scales every usable width before line breaking, so `1.0` reports the
    /// rendered layout and `0.95` asks whether the text fits a narrower frame.
    #[cfg(feature = "render")]
    pub fn text_layout_deterministic(&self, width_factor: f64) -> Result<Vec<TextFrameLayout>> {
        self.text_layout_deterministic_with_fonts(width_factor, &[])
    }

    /// Measure text with the same caller configuration as PDF and previews.
    #[cfg(feature = "render")]
    pub fn text_layout_deterministic_with_fonts(
        &self,
        width_factor: f64,
        font_files: &[(&str, &[u8])],
    ) -> Result<Vec<TextFrameLayout>> {
        if !width_factor.is_finite() || width_factor <= 0.0 {
            return Err(render_failure(format!(
                "text layout width factor must be finite and positive, found {width_factor}"
            )));
        }
        let package = self.staged_package(false)?;
        let mut assembly =
            prepare_render_context(&package, false, &caller_font_files(font_files), false)?;
        let mut frames = Vec::new();
        for (slide_index, ((prepared, slide), directions)) in assembly
            .slides
            .iter()
            .zip(&assembly.input.slides)
            .zip(&assembly.text_directions)
            .enumerate()
        {
            let context = ResolveCtx::new(
                &prepared.theme,
                render_effective_color_map(&prepared.master, &prepared.layout, &prepared.slide),
                &prepared.master,
                &prepared.layout,
                &prepared.slide,
                &assembly.default_text_style,
            );
            let sources = context
                .flatten()
                .into_iter()
                .filter(|item| render_source_shape_has_bounds(&context, item))
                .collect::<Vec<_>>();
            if sources.len() != slide.shapes.len() {
                return Err(render_failure(format!(
                    "{}: text layout source {}, resolved {}",
                    prepared.slide_part,
                    sources.len(),
                    slide.shapes.len()
                )));
            }
            for (shape_index, ((item, shape), smartart_clip)) in sources
                .into_iter()
                .zip(&slide.shapes)
                .zip(&prepared.smartart_clips)
                .enumerate()
            {
                let (
                    FlattenedItem::Shape {
                        source: FlattenedSource::Slide,
                        child,
                        ..
                    },
                    ResolvedContent::Text(text),
                    None,
                ) = (item, &shape.content, smartart_clip)
                else {
                    continue;
                };
                assembly.font_manager.begin_layout();
                let layout = oxoffice_pptx_render::layout_shape_text(
                    shape,
                    text,
                    &mut assembly.font_manager,
                    slide_index + 1,
                    directions.get(shape_index).map_or(&[], Vec::as_slice),
                    width_factor,
                )
                .map_err(|error| {
                    render_failure(format!(
                        "slide {}, shape id {}: {error}",
                        slide_index + 1,
                        child
                            .non_visual_id()
                            .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
                    ))
                })?;
                if layout.lines.iter().all(|line| line.text.trim().is_empty()) {
                    continue;
                }
                frames.push(TextFrameLayout {
                    slide_index,
                    shape_id: child.non_visual_id(),
                    name: child.non_visual_name(),
                    diagnostics: assembly.font_manager.current_font_diagnostics().to_vec(),
                    autofit: match text.autofit {
                        ResolvedAutofit::None => AutofitMode::None,
                        ResolvedAutofit::Normal { .. } => AutofitMode::Normal,
                        ResolvedAutofit::Shape => AutofitMode::Shape,
                    },
                    layout,
                });
            }
        }
        Ok(frames)
    }

    /// Render the presentation to the selected archival PDF profile.
    #[cfg(feature = "render")]
    pub fn to_pdfa_deterministic(&self, profile: oxoffice_pdf::PdfConformance) -> Result<Vec<u8>> {
        let (_, layout) = self.render_deterministic()?;
        Ok(oxoffice_pdf::render_to_pdf_with_options(
            &layout,
            oxoffice_pdf::PdfOptions::new(profile),
        )?)
    }

    /// Renders one deterministic speaker-notes page per source slide.
    #[cfg(feature = "render")]
    pub fn to_notes_pdf_deterministic(&self) -> Result<Vec<u8>> {
        let package = self.staged_package(false)?;
        let layout = render_notes_pages(&package, &[], false)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Renders one deterministic PNG per speaker-notes page at `dpi`.
    #[cfg(feature = "render")]
    pub fn notes_page_pngs_deterministic(&self, dpi: f64) -> Result<Vec<Vec<u8>>> {
        let package = self.staged_package(false)?;
        let layout = render_notes_pages(&package, &[], false)?;
        render_export_pngs(&layout, dpi)
    }

    /// Renders deterministic audience handouts in the selected arrangement.
    #[cfg(feature = "render")]
    pub fn to_handout_pdf_deterministic(&self, layout: HandoutLayout) -> Result<Vec<u8>> {
        let package = self.staged_package(false)?;
        let layout = render_handout_pages(&package, layout, &[], false)?;
        Ok(oxoffice_pdf::render_to_pdf_checked(&layout)?)
    }

    /// Renders one deterministic PNG per audience-handout page at `dpi`.
    #[cfg(feature = "render")]
    pub fn handout_page_pngs_deterministic(
        &self,
        layout: HandoutLayout,
        dpi: f64,
    ) -> Result<Vec<Vec<u8>>> {
        let package = self.staged_package(false)?;
        let layout = render_handout_pages(&package, layout, &[], false)?;
        render_export_pngs(&layout, dpi)
    }
}

#[cfg(feature = "render")]
pub(super) fn render_prepared_timeline_request(
    assembly: &mut PreparedRenderAssembly,
    request: TimelineRequest,
    slide_count: usize,
) -> Result<(DeterministicTimelineFrame, Vec<EvaluatedMediaState>)> {
    let incoming_source =
        assembly
            .slides
            .get(request.slide_index)
            .ok_or(Error::UnknownSlideIndex {
                index: request.slide_index,
                slide_count,
            })?;
    let font_manager = &mut assembly.font_manager;
    font_manager.begin_layout();
    let mut incoming_context = ResolveCtx::new(
        &incoming_source.theme,
        render_effective_color_map(
            &incoming_source.master,
            &incoming_source.layout,
            &incoming_source.slide,
        ),
        &incoming_source.master,
        &incoming_source.layout,
        &incoming_source.slide,
        &assembly.default_text_style,
    )
    .with_media_failures(&incoming_source.media_failures);
    if let Some(styles) = assembly.table_styles.as_ref() {
        incoming_context = incoming_context.with_table_styles(styles);
    }
    if request.fallback_policy.is_some() {
        incoming_context = incoming_context.with_media_poster_diagnostic_identity();
    }
    let (mut incoming, incoming_directions) = incoming_context
        .resolve_slide_with_chart_resources_and_text_directions_at(
            assembly.size,
            &incoming_source.media,
            &incoming_source.hyperlinks,
            &incoming_source.charts,
            font_manager,
            request.position,
        )
        .map_err(|error| render_failure(format!("{}: {error}", incoming_source.slide_part)))?;
    let mut incoming_media = Vec::new();
    let mut incoming_media_diagnostics = Vec::new();
    if let Some(policy) = request.fallback_policy {
        apply_media_fallback_policy(&mut incoming, &incoming_source.slide, policy, font_manager)?;
        let (states, diagnostics) = evaluate_slide_media(&incoming_source.slide, request.position);
        let consumed_shape_ids = states
            .iter()
            .map(|state| state.shape_id)
            .collect::<HashSet<_>>();
        incoming_media = states;
        incoming_media_diagnostics = incoming_source.media_diagnostics.clone();
        incoming_media_diagnostics.extend(diagnostics);
        suppress_consumed_media_timing_diagnostics(
            &mut incoming.state.diagnostics,
            incoming_source.slide.timing.as_ref(),
            &consumed_shape_ids,
        );
    }
    let (outgoing, outgoing_directions) = match request.outgoing_slide_index {
        None => (None, None),
        Some(index) if index == request.slide_index => {
            (Some(incoming.clone()), Some(incoming_directions.clone()))
        }
        Some(index) => {
            let source = assembly
                .slides
                .get(index)
                .ok_or(Error::UnknownSlideIndex { index, slide_count })?;
            let mut context = ResolveCtx::new(
                &source.theme,
                render_effective_color_map(&source.master, &source.layout, &source.slide),
                &source.master,
                &source.layout,
                &source.slide,
                &assembly.default_text_style,
            )
            .with_media_failures(&source.media_failures);
            if let Some(styles) = assembly.table_styles.as_ref() {
                context = context.with_table_styles(styles);
            }
            if request.fallback_policy.is_some() {
                context = context.with_media_poster_diagnostic_identity();
            }
            let (mut timeline, directions) = context
                .resolve_slide_with_chart_resources_and_text_directions_at(
                    assembly.size,
                    &source.media,
                    &source.hyperlinks,
                    &source.charts,
                    font_manager,
                    TimelinePosition {
                        elapsed_ms: u64::MAX,
                        click_count: u32::MAX,
                    },
                )
                .map_err(|error| render_failure(format!("{}: {error}", source.slide_part)))?;
            if let Some(policy) = request.fallback_policy {
                apply_media_fallback_policy(&mut timeline, &source.slide, policy, font_manager)?;
            }
            (Some(timeline), Some(directions))
        }
    };
    let mut layout_diagnostics = Vec::new();
    let incoming_frame = layout_timeline_slide_with_displayed_number(
        &assembly.input,
        request.slide_index,
        &incoming,
        Some(&incoming_directions),
        font_manager,
        assembly.first_slide_number,
    )
    .map_err(|error| render_failure(error.to_string()))?;
    let mut incoming_diagnostics = incoming_frame.diagnostics.into_iter();
    let incoming_owner_diagnostics = incoming_diagnostics
        .by_ref()
        .take(
            incoming.slide.diagnostics.len()
                + usize::from(incoming.slide.unresolved_member_count > 0),
        )
        .collect::<Vec<_>>();
    layout_diagnostics.extend(incoming_diagnostics);
    let mut outgoing_owner_diagnostics = Vec::new();
    let mut incoming_page = incoming_frame.page;
    apply_smartart_clips(
        &mut incoming_page,
        &incoming.slide,
        &incoming_source.smartart_clips,
    )?;
    let outgoing_page = match (request.outgoing_slide_index, outgoing.as_ref()) {
        (Some(index), Some(outgoing)) => {
            let frame = layout_timeline_slide_with_displayed_number(
                &assembly.input,
                index,
                outgoing,
                outgoing_directions.as_ref(),
                font_manager,
                assembly.first_slide_number,
            )
            .map_err(|error| render_failure(error.to_string()))?;
            let source = assembly
                .slides
                .get(index)
                .ok_or(Error::UnknownSlideIndex { index, slide_count })?;
            let mut frame_diagnostics = frame.diagnostics.into_iter();
            outgoing_owner_diagnostics.extend(frame_diagnostics.by_ref().take(
                outgoing.slide.diagnostics.len()
                    + usize::from(outgoing.slide.unresolved_member_count > 0),
            ));
            layout_diagnostics.extend(frame_diagnostics);
            let mut page = frame.page;
            apply_smartart_clips(&mut page, &outgoing.slide, &source.smartart_clips)?;
            Some(page)
        }
        _ => None,
    };
    let outgoing_page = outgoing_page.as_ref();
    let transition = incoming.state.transition.as_ref();
    let composed = if transition.is_some_and(|transition| {
        matches!(
            transition.effect,
            oxoffice_pptx_oxml::timing::TransitionEffect::Morph
        )
    }) {
        match (outgoing_page, outgoing.as_ref(), transition) {
            (Some(page), Some(outgoing), Some(transition)) => {
                compose_morph_transition(incoming_page, &incoming, page, outgoing, transition)
            }
            _ => compose_transition(incoming_page, outgoing_page, transition),
        }
    } else {
        compose_transition(incoming_page, outgoing_page, transition)
    };
    let mut diagnostics = incoming_owner_diagnostics;
    diagnostics.extend(incoming.state.diagnostics.clone());
    if let Some(outgoing) = outgoing.as_ref() {
        diagnostics.extend(outgoing_owner_diagnostics);
        diagnostics.extend(outgoing.state.diagnostics.clone());
    }
    diagnostics.extend(incoming_media_diagnostics);
    diagnostics.extend(layout_diagnostics);
    diagnostics.extend(composed.diagnostics);
    diagnostics.extend(font_manager.current_font_diagnostics().iter().cloned());
    Ok((
        DeterministicTimelineFrame {
            page: composed.page,
            state: incoming.state.clone(),
            diagnostics,
        },
        incoming_media,
    ))
}

#[cfg(feature = "render")]
fn configured_font_manager(
    package: &OpcPackage,
    presentation_part: &str,
    presentation: &CT_Presentation,
    callers: &[oxoffice_layout::FontFile],
    system_fallback: bool,
) -> Result<FontManager> {
    let mut sources: Vec<(Vec<String>, std::sync::Arc<[u8]>)> = Vec::new();
    if presentation
        .embed_true_type_fonts()
        .map_err(font_model_error)?
        == Some(true)
    {
        let mut parts = HashMap::new();
        for entry in presentation_embedded_fonts(package, presentation_part, presentation)? {
            for face in entry.faces {
                if let (Some(part), Some(data)) = (face.part_name, face.data) {
                    let index = *parts.entry(part.to_ascii_lowercase()).or_insert_with(|| {
                        sources.push((Vec::new(), data));
                        sources.len() - 1
                    });
                    if !sources[index].0.contains(&entry.typeface) {
                        sources[index].0.push(entry.typeface.clone());
                    }
                }
            }
        }
    }
    // A system font, found by glyph coverage, only draws what the embedded,
    // caller and bundled fonts lack; no family name selects one.
    let mut fonts = if system_fallback {
        FontManager::new_with_system_fallback()
    } else {
        FontManager::new_deterministic()
    }
    .map_err(|error| render_failure(format!("deck fonts: {error}")))?;
    fonts
        .load_embedded_fonts(&sources)
        .map_err(|error| render_failure(format!("embedded fonts: {error}")))?;
    fonts.load_additional_fonts(callers);
    Ok(fonts)
}

#[cfg(feature = "render")]
pub(super) fn caller_font_files(font_files: &[(&str, &[u8])]) -> Vec<oxoffice_layout::FontFile> {
    font_files
        .iter()
        .map(|(family, data)| oxoffice_layout::FontFile {
            family: (*family).to_owned(),
            data: data.to_vec(),
        })
        .collect()
}

#[cfg(feature = "render")]
fn assemble_render_input(
    package: &OpcPackage,
    font_files: &[oxoffice_layout::FontFile],
    system_fallback: bool,
) -> Result<(RenderInput, LayoutResult)> {
    let assembly = prepare_render_context(package, false, font_files, system_fallback)?;
    Ok((assembly.input, assembly.layout))
}

#[cfg(feature = "render")]
const MAX_PRESENTATION_EXPORT_DPI: f64 = 600.0;
#[cfg(feature = "render")]
const MAX_PRESENTATION_EXPORT_RASTER_BYTES: u64 = 256 * 1024 * 1024;

#[cfg(feature = "render")]
fn render_export_pngs(layout: &LayoutResult, dpi: f64) -> Result<Vec<Vec<u8>>> {
    validate_export_png_pages(&layout.pages, dpi)?;
    (0..layout.pages.len())
        .map(|index| {
            oxoffice_pdf::render_page_to_png(layout, index, dpi).ok_or_else(|| {
                render_failure(format!("could not rasterize export page {}", index + 1))
            })
        })
        .collect()
}

#[cfg(feature = "render")]
fn render_export_png(layout: &LayoutResult, index: usize, dpi: f64) -> Result<Option<Vec<u8>>> {
    let Some(page) = layout.pages.get(index) else {
        validate_export_png_pages(&[], dpi)?;
        return Ok(None);
    };
    validate_export_png_pages(std::slice::from_ref(page), dpi)?;
    oxoffice_pdf::render_page_to_png(layout, index, dpi)
        .map(Some)
        .ok_or_else(|| render_failure(format!("could not rasterize export page {}", index + 1)))
}

#[cfg(feature = "render")]
fn validate_export_png_pages(pages: &[std::sync::Arc<PageFrame>], dpi: f64) -> Result<()> {
    if !dpi.is_finite() || dpi <= 0.0 || dpi > MAX_PRESENTATION_EXPORT_DPI {
        return Err(render_failure(format!(
            "raster DPI must be finite and in 0..={MAX_PRESENTATION_EXPORT_DPI}, found {dpi}"
        )));
    }
    let scale = dpi / 72.0;
    let estimated = pages.iter().try_fold(0u64, |total, page| {
        let width = (page.width * scale).ceil();
        let height = (page.height * scale).ceil();
        if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
            return None;
        }
        let page_bytes = (width as u64).checked_mul(height as u64)?.checked_mul(4)?;
        total.checked_add(page_bytes)
    });
    if estimated.is_none_or(|bytes| bytes > MAX_PRESENTATION_EXPORT_RASTER_BYTES) {
        return Err(render_failure(format!(
            "raster export exceeds the {} byte decoded-output limit",
            MAX_PRESENTATION_EXPORT_RASTER_BYTES
        )));
    }
    Ok(())
}

/// The export's text drawn as empty boxes or with a system font, named once
/// over all its pages (a slide's text is drawn again in its preview on its
/// notes page), in place of what the deck's own layout said.
#[cfg(feature = "render")]
fn with_export_missing_glyphs(mut result: LayoutResult, fonts: &FontManager) -> LayoutResult {
    let coverage = oxoffice_layout::glyph_coverage_diagnostics(
        &result.pages,
        fonts,
        oxoffice_pptx_render::MISSING_GLYPH_WAY_OUT,
    );
    result.diagnostics.retain(|diagnostic| {
        !diagnostic.message.contains("is drawn as empty boxes")
            && !diagnostic.message.contains("is drawn with the system font")
    });
    result.diagnostics.extend(coverage);
    result
}

#[cfg(feature = "render")]
fn append_export_font_facts(
    diagnostics: &mut Vec<oxoffice_layout::Diagnostic>,
    seen: &mut Vec<oxoffice_layout::Diagnostic>,
    facts: &[oxoffice_layout::Diagnostic],
) {
    for fact in facts {
        if !seen.contains(fact) {
            diagnostics.push(fact.clone());
            seen.push(fact.clone());
        }
    }
}

#[cfg(feature = "render")]
fn render_notes_pages(
    package: &OpcPackage,
    font_files: &[oxoffice_layout::FontFile],
    system_fallback: bool,
) -> Result<LayoutResult> {
    let mut assembly = prepare_render_context(package, false, font_files, system_fallback)?;
    if assembly.slides.is_empty() {
        return Ok(LayoutResult::new(
            Vec::new(),
            assembly.font_manager.all_font_data(),
            assembly.input.metadata.clone(),
            Vec::new(),
        ));
    }
    let presentation_part = package
        .main_document_part()
        .ok_or(Error::MissingMainDocument)?;
    let presentation = CT_Presentation::from_xml(required_part(package, &presentation_part)?)
        .map_err(|error| Error::MalformedPart {
            part_name: presentation_part.clone(),
            message: error.to_string(),
        })?;
    let notes_size = export_page_size(&presentation);
    let present = package
        .get_part_rels(&presentation_part)
        .is_some_and(|rels| !rels.get_all_by_type(rel_types::NOTES_MASTER).is_empty());
    let (notes_master_part, notes_master, theme) = if present {
        let notes_master_part = render_exact_related_part(
            package,
            &presentation_part,
            rel_types::NOTES_MASTER,
            content_types::NOTES_MASTER,
        )?;
        let notes_master = CT_NotesMaster::from_xml(required_part(package, &notes_master_part)?)
            .map_err(|error| Error::MalformedPart {
                part_name: notes_master_part.clone(),
                message: error.to_string(),
            })?;
        let theme_part = render_exact_related_part(
            package,
            &notes_master_part,
            rel_types::THEME,
            content_types::THEME,
        )?;
        let theme = CT_OfficeStyleSheet::from_xml(required_part(package, &theme_part)?).map_err(
            |error| Error::MalformedPart {
                part_name: theme_part,
                message: error.to_string(),
            },
        )?;
        (notes_master_part, notes_master, theme)
    } else {
        // Native-authenticated Office 16:9 notes geometry, kept entirely in
        // this read-only export surface. Imported owner graphs are not repaired.
        let mut tree = CT_ShapeTree::new();
        for (id, kind, idx, x, y, cx, cy) in [
            (1, PhType::Header, None, 0, 0, 2971800, 458788),
            (2, PhType::DateTime, Some(1), 3884613, 0, 2971800, 458788),
            (
                3,
                PhType::SlideImage,
                Some(2),
                685800,
                1143000,
                5486400,
                3086100,
            ),
            (4, PhType::Body, Some(3), 685800, 4400550, 5486400, 3600450),
            (5, PhType::Footer, Some(4), 0, 8685213, 2971800, 458787),
            (
                6,
                PhType::SlideNumber,
                Some(5),
                3884613,
                8685213,
                2971800,
                458787,
            ),
        ] {
            let placeholder = CT_Placeholder::new(Some(kind), idx);
            let mut shape = CT_Shape::new_placeholder(id, placeholder)
                .map_err(|e| render_failure(e.to_string()))?;
            shape.shape_properties.transform =
                Some(positioned_transform(Emu(x), Emu(y), Emu(cx), Emu(cy)));
            tree.children.push(ShapeTreeChild::Shape(shape));
        }
        let shell = format!(
            r#"<p:notesMaster xmlns:p="{P_NS}" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="0" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" bg1="lt1" bg2="lt2" folHlink="folHlink" hlink="hlink" tx1="dk1" tx2="dk2"/></p:notesMaster>"#
        );
        let mut master = CT_NotesMaster::from_xml(shell.as_bytes())
            .map_err(|e| render_failure(e.to_string()))?;
        master.common_slide_data.shape_tree = tree;
        (
            "/ppt/rdocx-neutral-notes-export.xml".to_owned(),
            master,
            assembly.slides[0].theme.clone(),
        )
    };
    let default_text_style = notes_master
        .notes_style
        .clone()
        .or(presentation.default_text_style.clone())
        .unwrap_or_default();
    let shell_layout = assembly.slides[0].layout.clone();
    let shell_master = assembly.slides[0].master.clone();
    let mut export_package = package.clone();
    let mut pages = Vec::with_capacity(assembly.slides.len());
    let mut diagnostics = assembly.layout.diagnostics.clone();
    let mut seen_font_facts = assembly.font_manager.current_font_diagnostics().to_vec();
    for (slide_index, prepared) in assembly.slides.iter().enumerate() {
        let notes = resolve_notes(package, &prepared.slide_part)?;
        if let Some(notes) = &notes {
            validate_notes_export_graph(
                package,
                &prepared.slide_part,
                &notes.part_name,
                &notes_master_part,
                present,
            )?;
        }
        let (page_source, page_master, page_notes) = remap_notes_export_scope(
            &mut export_package,
            &notes_master_part,
            &notes_master,
            notes.as_ref(),
        )?;
        let common = compose_notes_common(
            &page_master,
            page_notes.as_ref(),
            slide_index + 1,
            &mut diagnostics,
        )?;
        let preview = assembly.layout.pages[slide_index].as_ref();
        let page = render_export_surface(
            &export_package,
            &page_source,
            common,
            page_master.color_map.clone(),
            &default_text_style,
            &theme,
            notes_size,
            slide_index + 1,
            Some(preview),
            &shell_layout,
            &shell_master,
            &mut assembly.input.media,
            &mut assembly.font_manager,
            assembly.table_styles.as_ref(),
        )?;
        diagnostics.extend(page.1);
        append_export_font_facts(&mut diagnostics, &mut seen_font_facts, &page.2);
        pages.push(std::sync::Arc::new(page.0));
    }
    let mut result = LayoutResult::new(
        pages,
        assembly.font_manager.all_font_data(),
        assembly.input.metadata.clone(),
        Vec::new(),
    );
    result.diagnostics = diagnostics;
    Ok(with_export_missing_glyphs(result, &assembly.font_manager))
}

#[cfg(feature = "render")]
fn remap_notes_export_scope(
    package: &mut OpcPackage,
    master_part: &str,
    master: &CT_NotesMaster,
    notes: Option<&NotesRecord>,
) -> Result<(String, CT_NotesMaster, Option<CT_NotesSlide>)> {
    const SCOPE_PART: &str = "/ppt/rdocx-notes-export.xml";

    let mut relationships = Vec::new();
    let master_map =
        copy_export_relationship_scope(package, master_part, "rdocxMaster", &mut relationships)?;
    let master_xml = master.to_xml().map_err(|error| Error::MalformedPart {
        part_name: master_part.to_owned(),
        message: error.to_string(),
    })?;
    let master_xml =
        rewrite_exact_rel_ids(&master_xml, &master_map).map_err(|error| Error::MalformedPart {
            part_name: master_part.to_owned(),
            message: error.to_string(),
        })?;
    let remapped_master =
        CT_NotesMaster::from_xml(&master_xml).map_err(|error| Error::MalformedPart {
            part_name: master_part.to_owned(),
            message: error.to_string(),
        })?;

    let remapped_notes = notes
        .map(|record| {
            let notes_map = copy_export_relationship_scope(
                package,
                &record.part_name,
                "rdocxNotes",
                &mut relationships,
            )?;
            let notes_xml = record
                .notes
                .to_xml()
                .map_err(|error| Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: error.to_string(),
                })?;
            let notes_xml = rewrite_exact_rel_ids(&notes_xml, &notes_map).map_err(|error| {
                Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: error.to_string(),
                }
            })?;
            CT_NotesSlide::from_xml(&notes_xml).map_err(|error| Error::MalformedPart {
                part_name: record.part_name.clone(),
                message: error.to_string(),
            })
        })
        .transpose()?;

    package.get_or_create_part_rels(SCOPE_PART).items = relationships;
    Ok((SCOPE_PART.to_owned(), remapped_master, remapped_notes))
}

#[cfg(feature = "render")]
fn copy_export_relationship_scope(
    package: &OpcPackage,
    source_part: &str,
    prefix: &str,
    output: &mut Vec<Relationship>,
) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let Some(source) = package.get_part_rels(source_part) else {
        return Ok(map);
    };
    for (index, relationship) in source.items.iter().enumerate() {
        let id = format!("{prefix}{}", index + 1);
        if map.insert(relationship.id.clone(), id.clone()).is_some() {
            return Err(render_failure(format!(
                "{source_part}: duplicate relationship id {}",
                relationship.id
            )));
        }
        let external = relationship
            .target_mode
            .as_deref()
            .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
        output.push(Relationship {
            id,
            rel_type: relationship.rel_type.clone(),
            target: if external {
                relationship.target.clone()
            } else {
                OpcPackage::resolve_rel_target(source_part, &relationship.target)
            },
            target_mode: relationship.target_mode.clone(),
        });
    }
    Ok(map)
}

#[cfg(feature = "render")]
fn render_handout_pages(
    package: &OpcPackage,
    handout_layout: HandoutLayout,
    font_files: &[oxoffice_layout::FontFile],
    system_fallback: bool,
) -> Result<LayoutResult> {
    let mut assembly = prepare_render_context(package, false, font_files, system_fallback)?;
    if assembly.slides.is_empty() {
        return Ok(LayoutResult::new(
            Vec::new(),
            assembly.font_manager.all_font_data(),
            assembly.input.metadata.clone(),
            Vec::new(),
        ));
    }
    let presentation_part = package
        .main_document_part()
        .ok_or(Error::MissingMainDocument)?;
    let presentation = CT_Presentation::from_xml(required_part(package, &presentation_part)?)
        .map_err(|error| Error::MalformedPart {
            part_name: presentation_part.clone(),
            message: error.to_string(),
        })?;
    let page_size = export_page_size(&presentation);
    let present = package
        .get_part_rels(&presentation_part)
        .is_some_and(|rels| !rels.get_all_by_type(rel_types::HANDOUT_MASTER).is_empty());
    let (handout_master_part, handout_master, theme) = if present {
        let handout_master_part = render_exact_related_part(
            package,
            &presentation_part,
            rel_types::HANDOUT_MASTER,
            content_types::HANDOUT_MASTER,
        )?;
        let handout_master =
            CT_HandoutMaster::from_xml(required_part(package, &handout_master_part)?).map_err(
                |error| Error::MalformedPart {
                    part_name: handout_master_part.clone(),
                    message: error.to_string(),
                },
            )?;
        let theme_part = render_exact_related_part(
            package,
            &handout_master_part,
            rel_types::THEME,
            content_types::THEME,
        )?;
        let theme = CT_OfficeStyleSheet::from_xml(required_part(package, &theme_part)?).map_err(
            |error| Error::MalformedPart {
                part_name: theme_part,
                message: error.to_string(),
            },
        )?;
        (handout_master_part, handout_master, theme)
    } else {
        let source = br#"<p:handoutMaster xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" bg1="lt1" bg2="lt2" folHlink="folHlink" hlink="hlink" tx1="dk1" tx2="dk2"/></p:handoutMaster>"#;
        let master =
            CT_HandoutMaster::from_xml(source).map_err(|e| render_failure(e.to_string()))?;
        (
            "/ppt/rdocx-neutral-handout-export.xml".to_owned(),
            master,
            assembly.slides[0].theme.clone(),
        )
    };
    let default_text_style = presentation.default_text_style.clone().unwrap_or_default();
    let shell_layout = assembly.slides[0].layout.clone();
    let shell_master = assembly.slides[0].master.clone();
    let capacity = handout_layout.slides_per_page();
    let page_count = assembly.layout.pages.len().div_ceil(capacity);
    let mut pages = Vec::with_capacity(page_count);
    let mut diagnostics = assembly.layout.diagnostics.clone();
    let mut seen_font_facts = assembly.font_manager.current_font_diagnostics().to_vec();
    for page_index in 0..page_count {
        let common = compose_handout_common(&handout_master, page_index + 1)?;
        let (mut base, page_diagnostics, page_font_facts) = render_export_surface(
            package,
            &handout_master_part,
            common,
            handout_master.color_map.clone(),
            &default_text_style,
            &theme,
            page_size,
            page_index + 1,
            None,
            &shell_layout,
            &shell_master,
            &mut assembly.input.media,
            &mut assembly.font_manager,
            assembly.table_styles.as_ref(),
        )?;
        diagnostics.extend(page_diagnostics);
        append_export_font_facts(&mut diagnostics, &mut seen_font_facts, &page_font_facts);
        let start = page_index * capacity;
        let end = (start + capacity).min(assembly.layout.pages.len());
        let mut elements = std::mem::take(&mut base.elements);
        let mut thumbnails = handout_thumbnail_elements(
            &assembly.layout.pages[start..end],
            start,
            handout_layout,
            page_size,
            &mut assembly.font_manager,
        )?;
        append_export_font_facts(
            &mut diagnostics,
            &mut seen_font_facts,
            assembly.font_manager.current_font_diagnostics(),
        );
        elements.append(&mut thumbnails);
        base.elements = elements;
        pages.push(std::sync::Arc::new(base));
    }
    let mut result = LayoutResult::new(
        pages,
        assembly.font_manager.all_font_data(),
        assembly.input.metadata.clone(),
        Vec::new(),
    );
    result.diagnostics = diagnostics;
    Ok(with_export_missing_glyphs(result, &assembly.font_manager))
}

#[cfg(feature = "render")]
fn export_page_size(presentation: &CT_Presentation) -> (f64, f64) {
    (
        presentation.notes_size.cx.0 as f64 / 12_700.0,
        presentation.notes_size.cy.0 as f64 / 12_700.0,
    )
}

#[cfg(feature = "render")]
fn render_exact_related_part(
    package: &OpcPackage,
    source_part: &str,
    relationship_type: &str,
    content_type: &str,
) -> Result<String> {
    let matches = package
        .get_part_rels(source_part)
        .map(|relationships| relationships.get_all_by_type(relationship_type))
        .unwrap_or_default();
    if matches.len() != 1 {
        return Err(render_failure(format!(
            "{source_part}: found {} relationships of type {relationship_type}, expected exactly one",
            matches.len()
        )));
    }
    let relationship = matches[0];
    reject_external(source_part, relationship)?;
    let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
    required_part(package, &target)?;
    let actual = package.content_types.content_type_for(&target);
    if actual != Some(content_type) {
        return Err(render_failure(format!(
            "{source_part}: relationship {} targets {target} with content type {:?}, expected {content_type}",
            relationship.id, actual
        )));
    }
    Ok(target)
}

#[cfg(feature = "render")]
fn validate_notes_export_graph(
    package: &OpcPackage,
    slide_part: &str,
    notes_part: &str,
    notes_master_part: &str,
    require_master: bool,
) -> Result<()> {
    if package.content_types.content_type_for(notes_part) != Some(content_types::NOTES_SLIDE) {
        return Err(render_failure(format!(
            "{notes_part}: expected notes-slide content type"
        )));
    }
    if require_master
        || package
            .get_part_rels(notes_part)
            .is_some_and(|rels| !rels.get_all_by_type(rel_types::NOTES_MASTER).is_empty())
    {
        let master = render_exact_related_part(
            package,
            notes_part,
            rel_types::NOTES_MASTER,
            content_types::NOTES_MASTER,
        )?;
        if master != notes_master_part {
            return Err(render_failure(format!(
                "{notes_part}: notes master {master} differs from presentation master {notes_master_part}"
            )));
        }
    }
    let slide_relationships = package
        .get_part_rels(notes_part)
        .map(|relationships| relationships.get_all_by_type(rel_types::SLIDE))
        .unwrap_or_default();
    if slide_relationships.len() > 1 {
        return Err(render_failure(format!(
            "{notes_part}: found {} relationships of type {}, expected at most one",
            slide_relationships.len(),
            rel_types::SLIDE
        )));
    }
    if let Some(relationship) = slide_relationships.first() {
        reject_external(notes_part, relationship)?;
        let slide = OpcPackage::resolve_rel_target(notes_part, &relationship.target);
        required_part(package, &slide)?;
        let actual = package.content_types.content_type_for(&slide);
        if actual != Some(content_types::SLIDE) {
            return Err(render_failure(format!(
                "{notes_part}: relationship {} targets {slide} with content type {:?}, expected {}",
                relationship.id,
                actual,
                content_types::SLIDE
            )));
        }
        if slide != slide_part {
            return Err(render_failure(format!(
                "{notes_part}: source slide {slide} differs from owner {slide_part}"
            )));
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
fn compose_notes_common(
    master: &CT_NotesMaster,
    notes: Option<&CT_NotesSlide>,
    slide_number: usize,
    diagnostics: &mut Vec<oxoffice_layout::Diagnostic>,
) -> Result<CT_CommonSlideData> {
    let mut common = master.common_slide_data.clone();
    if let Some(notes) = notes {
        if let Some(background) = &notes.common_slide_data.background {
            common.background = Some(background.clone());
        }
        if notes
            .show_master_shapes()
            .map_err(|error| render_failure(error.to_string()))?
            == Some(false)
        {
            retain_notes_master_placeholders(&mut common.shape_tree.children);
        }
        let mut overlays = Vec::new();
        collect_export_placeholder_shapes(
            &notes.common_slide_data.shape_tree.children,
            &mut overlays,
        )?;
        let mut consumed = HashSet::new();
        apply_export_placeholder_overlays(
            &mut common.shape_tree.children,
            &overlays,
            &mut consumed,
        )?;
        for (index, (unmatched, _)) in overlays.iter().enumerate() {
            if !consumed.contains(&index) {
                diagnostics.push(oxoffice_layout::Diagnostic {
                    message: format!(
                        "notes slide placeholder {unmatched:?} has no matching notes-master placeholder and was skipped"
                    ),
                });
            }
        }
        for child in &notes.common_slide_data.shape_tree.children {
            if child_export_placeholder(child).is_none() {
                common.shape_tree.children.push(child.clone());
            }
        }
    }
    filter_export_master_shapes(
        &mut common.shape_tree.children,
        master.header_footer.as_ref(),
        notes.is_some(),
        slide_number,
    );
    let slide_images = count_export_placeholders(&common.shape_tree.children, |kind| {
        kind == &PhType::SlideImage
    });
    if slide_images != 1 {
        return Err(render_failure(format!(
            "notes master has {slide_images} visible slide-image placeholders, expected exactly one"
        )));
    }
    Ok(common)
}

#[cfg(feature = "render")]
fn retain_notes_master_placeholders(children: &mut Vec<ShapeTreeChild>) {
    for child in children.iter_mut() {
        if let ShapeTreeChild::GroupShape(group) = child {
            retain_notes_master_placeholders(&mut group.children);
        }
    }
    children.retain(|child| match child {
        ShapeTreeChild::Shape(shape) => shape.placeholder.is_some(),
        ShapeTreeChild::Picture(picture) => picture.placeholder.is_some(),
        ShapeTreeChild::GroupShape(group) => !group.children.is_empty(),
        _ => false,
    });
}

#[cfg(feature = "render")]
fn compose_handout_common(
    master: &CT_HandoutMaster,
    page_number: usize,
) -> Result<CT_CommonSlideData> {
    let mut common = master.common_slide_data.clone();
    filter_export_master_shapes(
        &mut common.shape_tree.children,
        master.header_footer.as_ref(),
        false,
        page_number,
    );
    Ok(common)
}

#[cfg(feature = "render")]
fn collect_export_placeholder_shapes(
    children: &[ShapeTreeChild],
    output: &mut Vec<(PlaceholderKey, CT_Shape)>,
) -> Result<()> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) => {
                if let Some(placeholder) = shape.placeholder.as_ref() {
                    let key = placeholder.key();
                    if output.iter().any(|(existing, _)| existing.matches(&key)) {
                        return Err(render_failure(format!(
                            "notes slide has ambiguous placeholder {key:?}"
                        )));
                    }
                    output.push((key, shape.clone()));
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                collect_export_placeholder_shapes(&group.children, output)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
fn apply_export_placeholder_overlays(
    children: &mut Vec<ShapeTreeChild>,
    overlays: &[(PlaceholderKey, CT_Shape)],
    consumed: &mut HashSet<usize>,
) -> Result<()> {
    let mut index = 0;
    while index < children.len() {
        let mut absent = false;
        match &mut children[index] {
            ShapeTreeChild::Shape(shape) => {
                if let Some(placeholder) = shape.placeholder.as_ref() {
                    let key = placeholder.key();
                    let mut matches = overlays
                        .iter()
                        .enumerate()
                        .filter(|(_, (overlay_key, _))| key.matches(overlay_key));
                    if let Some((overlay_index, (_, overlay))) = matches.next() {
                        if matches.next().is_some() {
                            return Err(render_failure(format!(
                                "notes master placeholder {key:?} matches multiple notes-slide placeholders"
                            )));
                        }
                        if !consumed.insert(overlay_index) {
                            return Err(render_failure(format!(
                                "notes master has duplicate placeholder {key:?}"
                            )));
                        }
                        let properties = overlay
                            .shape_properties
                            .with_inherited_defaults(&shape.shape_properties)
                            .map_err(|error| render_failure(error.to_string()))?;
                        if overlay.style().is_some() {
                            *shape = overlay.clone();
                        } else {
                            shape.text_body = overlay.text_body.clone();
                        }
                        *shape.shape_properties = properties;
                    } else {
                        absent = true;
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                apply_export_placeholder_overlays(&mut group.children, overlays, consumed)?;
            }
            _ => {}
        }
        if absent {
            children.remove(index);
        } else {
            index += 1;
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
fn child_export_placeholder(child: &ShapeTreeChild) -> Option<PhType> {
    match child {
        ShapeTreeChild::Shape(shape) => shape
            .placeholder
            .as_ref()
            .map(CT_Placeholder::effective_type),
        ShapeTreeChild::Picture(picture) => picture
            .placeholder
            .as_ref()
            .map(CT_Placeholder::effective_type),
        _ => None,
    }
}

#[cfg(feature = "render")]
fn filter_export_master_shapes(
    children: &mut Vec<ShapeTreeChild>,
    header_footer: Option<&CT_HeaderFooter>,
    keep_body: bool,
    page_number: usize,
) {
    children.retain_mut(|child| {
        if let ShapeTreeChild::GroupShape(group) = child {
            filter_export_master_shapes(&mut group.children, header_footer, keep_body, page_number);
            return true;
        }
        let Some(kind) = child_export_placeholder(child) else {
            return true;
        };
        let enabled = match kind {
            PhType::DateTime => header_footer.is_none_or(CT_HeaderFooter::date_time_enabled),
            PhType::Header => header_footer.is_none_or(CT_HeaderFooter::header_enabled),
            PhType::Footer => header_footer.is_none_or(CT_HeaderFooter::footer_enabled),
            PhType::SlideNumber => header_footer.is_none_or(CT_HeaderFooter::slide_number_enabled),
            PhType::Body => keep_body,
            _ => true,
        };
        if enabled
            && kind == PhType::SlideNumber
            && let ShapeTreeChild::Shape(shape) = child
        {
            if let Some(placeholder) = shape.placeholder.as_mut() {
                placeholder.ph_type = Some(PhType::Other("exportPageNumber".to_owned()));
            }
            if let Some(body) = shape.text_body.as_mut() {
                if keep_body {
                    // Resolve actual local number fields on the render clone.
                    // Empty placeholders and ordinary local text stay unchanged.
                    for index in 0..body.paragraphs().len() {
                        let paragraph =
                            body.paragraph_mut(index).expect("existing notes paragraph");
                        for run in &mut paragraph.runs {
                            if let oxoffice_drawing::text::TextRun::Field(field) = run
                                && field
                                    .field_type
                                    .as_deref()
                                    .is_some_and(|kind| kind.eq_ignore_ascii_case("slidenum"))
                            {
                                field.field_type = Some("exportPageNumber".into());
                                field.text.get_or_insert_with(Default::default).value =
                                    page_number.to_string();
                            }
                        }
                    }
                } else {
                    body.set_text(&page_number.to_string());
                }
            }
        }
        if enabled
            && kind == PhType::Body
            && let ShapeTreeChild::Shape(shape) = child
            && shape
                .text_body
                .as_ref()
                .is_none_or(|body| body.plain_text().trim().is_empty())
        {
            return false;
        }
        enabled
    });
}

#[cfg(feature = "render")]
fn count_export_placeholders(
    children: &[ShapeTreeChild],
    predicate: impl Copy + Fn(&PhType) -> bool,
) -> usize {
    children
        .iter()
        .map(|child| match child {
            ShapeTreeChild::GroupShape(group) => {
                count_export_placeholders(&group.children, predicate)
            }
            _ => child_export_placeholder(child)
                .as_ref()
                .is_some_and(predicate) as usize,
        })
        .sum()
}

#[cfg(feature = "render")]
#[allow(clippy::too_many_arguments)]
fn render_export_surface(
    package: &OpcPackage,
    source_part: &str,
    common: CT_CommonSlideData,
    color_map: ColorMap,
    default_text_style: &CT_TextListStyle,
    theme: &CT_OfficeStyleSheet,
    size: (f64, f64),
    page_number: usize,
    preview: Option<&PageFrame>,
    shell_layout: &CT_SlideLayout,
    shell_master: &CT_SlideMaster,
    deck_media: &mut HashMap<MediaId, MediaData>,
    font_manager: &mut FontManager,
    table_styles: Option<&CT_TableStyleList>,
) -> Result<(
    PageFrame,
    Vec<oxoffice_layout::Diagnostic>,
    Vec<oxoffice_layout::Diagnostic>,
)> {
    let mut slide = CT_Slide::new(CT_ShapeTree::new());
    slide.common_slide_data = common;
    let mut layout = shell_layout.clone();
    layout.common_slide_data.background = None;
    layout.common_slide_data.shape_tree = CT_ShapeTree::new();
    layout.header_footer = None;
    let mut master = shell_master.clone();
    master.common_slide_data.background = None;
    master.common_slide_data.shape_tree = CT_ShapeTree::new();
    master.color_map = color_map.clone();
    master.text_styles = None;
    master.header_footer = None;
    let empty: &[ShapeTreeChild] = &[];
    let (media, media_failures, hyperlinks, charts, diagrams) = render_scoped_resources(
        package,
        [source_part, source_part, source_part],
        [&slide.common_slide_data.shape_tree.children, empty, empty],
        deck_media,
    )?;
    let expansion = diagram::expand_tree(&mut slide.common_slide_data.shape_tree, &diagrams.slide);
    let mut context = ResolveCtx::new(
        theme,
        color_map,
        &master,
        &layout,
        &slide,
        default_text_style,
    )
    .with_media_failures(&media_failures);
    if let Some(styles) = table_styles {
        context = context.with_table_styles(styles);
    }
    let mut preview_index = None;
    let mut source_index = 0usize;
    for item in context.flatten() {
        if !render_source_shape_has_bounds(&context, &item) {
            continue;
        }
        if let FlattenedItem::Shape { child, .. } = &item
            && child_export_placeholder(child) == Some(PhType::SlideImage)
            && preview_index.replace(source_index).is_some()
        {
            return Err(render_failure(
                "notes surface has multiple bounded slide-image placeholders",
            ));
        }
        source_index += 1;
    }
    if preview.is_some() != preview_index.is_some() {
        return Err(render_failure(
            "notes surface and slide-image placeholder do not correspond",
        ));
    }
    let (mut resolved, directions) = context
        .resolve_slide_with_chart_resources_and_text_directions(
            size,
            &media,
            &hyperlinks,
            &charts,
            font_manager,
        )
        .map_err(|error| render_failure(format!("{source_part}: {error}")))?;
    resolved.diagnostics.extend(expansion.diagnostics);
    if let (Some(preview), Some(index)) = (preview, preview_index) {
        let shape = resolved.shapes.get_mut(index).ok_or_else(|| {
            render_failure("notes slide-image placeholder was dropped during resolution")
        })?;
        let target = Rect {
            x: 0.0,
            y: 0.0,
            width: shape.bounds.width,
            height: shape.bounds.height,
        };
        shape.content = ResolvedContent::Group(page_thumbnail_group(preview, target).0);
    }
    let input = RenderInput {
        slides: vec![resolved],
        media: deck_media.clone(),
        fonts: Vec::new(),
        metadata: None,
    };
    let layout_result = layout_presentation_with_font_manager_and_text_directions_mut(
        &input,
        font_manager,
        &[directions],
    )
    .map_err(|error| render_failure(error.to_string()))?;
    let mut page = layout_result
        .pages
        .first()
        .map(|page| page.as_ref().clone())
        .ok_or_else(|| render_failure("export surface produced no page"))?;
    page.page_number = page_number;
    // The renderer appends exactly these owned facts after ordinary slide warnings.
    // Keep the contributions separate so only font facts are deduplicated by exporters.
    let font_facts = font_manager.current_font_diagnostics().to_vec();
    let mut diagnostics = layout_result.diagnostics;
    diagnostics.truncate(diagnostics.len() - font_facts.len());
    Ok((page, diagnostics, font_facts))
}

#[cfg(feature = "render")]
fn page_thumbnail_group(page: &PageFrame, target: Rect) -> (GroupElement, Rect) {
    let scale = (target.width / page.width).min(target.height / page.height);
    let fitted = Rect {
        x: target.x + (target.width - page.width * scale) / 2.0,
        y: target.y + (target.height - page.height * scale) / 2.0,
        width: page.width * scale,
        height: page.height * scale,
    };
    let mut children = vec![PositionedElement::FilledRect {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: page.width,
            height: page.height,
        },
        color: Color::WHITE,
    }];
    if let Some(background) = &page.background {
        children.push(PositionedElement::Path(PathElement {
            path: LayoutPath::rect(Rect {
                x: 0.0,
                y: 0.0,
                width: page.width,
                height: page.height,
            }),
            fill: Some(background.clone()),
            stroke: None,
        }));
    }
    children.extend(page.elements.clone());
    (
        GroupElement {
            transform: Transform {
                a: scale,
                b: 0.0,
                c: 0.0,
                d: scale,
                e: fitted.x,
                f: fitted.y,
            },
            clip: Some(LayoutPath::rect(Rect {
                x: 0.0,
                y: 0.0,
                width: page.width,
                height: page.height,
            })),
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
        fitted,
    )
}

#[cfg(feature = "render")]
fn handout_thumbnail_elements(
    pages: &[std::sync::Arc<PageFrame>],
    source_start: usize,
    layout: HandoutLayout,
    page_size: (f64, f64),
    font_manager: &mut FontManager,
) -> Result<Vec<PositionedElement>> {
    let targets = handout_targets(layout, page_size);
    let mut elements = Vec::new();
    for (offset, page) in pages.iter().enumerate() {
        let target = targets[offset];
        let (thumbnail, fitted) = page_thumbnail_group(page, target);
        elements.push(PositionedElement::Group(thumbnail));
        push_thumbnail_border(&mut elements, fitted);
        elements.push(handout_slide_number(
            source_start + offset + 1,
            fitted,
            font_manager,
        )?);
        if layout == HandoutLayout::Three {
            push_three_up_note_rules(&mut elements, target, page_size.0);
        }
    }
    Ok(elements)
}

#[cfg(feature = "render")]
fn handout_targets(layout: HandoutLayout, page_size: (f64, f64)) -> Vec<Rect> {
    let margin_x = 36.0;
    let margin_y = 54.0;
    let gap = 18.0;
    let label_height = 14.0;
    let usable_width = (page_size.0 - 2.0 * margin_x).max(1.0);
    let usable_height = (page_size.1 - 2.0 * margin_y).max(1.0);
    let (columns, rows, width_fraction) = match layout {
        HandoutLayout::One => (1, 1, 1.0),
        HandoutLayout::Two => (1, 2, 1.0),
        HandoutLayout::Three => (1, 3, 0.46),
        HandoutLayout::Four => (2, 2, 1.0),
        HandoutLayout::Six => (2, 3, 1.0),
        HandoutLayout::Nine => (3, 3, 1.0),
    };
    let cell_width = (usable_width - gap * (columns - 1) as f64) / columns as f64;
    let cell_height = (usable_height - gap * (rows - 1) as f64) / rows as f64;
    (0..layout.slides_per_page())
        .map(|index| {
            let row = index / columns;
            let column = index % columns;
            Rect {
                x: margin_x + column as f64 * (cell_width + gap),
                y: margin_y + row as f64 * (cell_height + gap),
                width: cell_width * width_fraction,
                height: (cell_height - label_height).max(1.0),
            }
        })
        .collect()
}

#[cfg(feature = "render")]
fn push_thumbnail_border(elements: &mut Vec<PositionedElement>, rect: Rect) {
    for (start, end) in [
        (
            Point {
                x: rect.x,
                y: rect.y,
            },
            Point {
                x: rect.x + rect.width,
                y: rect.y,
            },
        ),
        (
            Point {
                x: rect.x + rect.width,
                y: rect.y,
            },
            Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            },
        ),
        (
            Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            },
            Point {
                x: rect.x,
                y: rect.y + rect.height,
            },
        ),
        (
            Point {
                x: rect.x,
                y: rect.y + rect.height,
            },
            Point {
                x: rect.x,
                y: rect.y,
            },
        ),
    ] {
        elements.push(PositionedElement::Line {
            start,
            end,
            width: 0.75,
            color: Color::BLACK,
            dash_pattern: None,
        });
    }
}

#[cfg(feature = "render")]
fn handout_slide_number(
    number: usize,
    thumbnail: Rect,
    font_manager: &mut FontManager,
) -> Result<PositionedElement> {
    let text = number.to_string();
    let font_id = font_manager
        .resolve_font_for_text(Some("Carlito"), false, false, &text)
        .map_err(|error| render_failure(format!("handout slide number font: {error}")))?;
    let shaped = font_manager
        .shape_text(font_id, &text, 9.0)
        .map_err(|error| render_failure(format!("handout slide number shaping: {error}")))?;
    Ok(PositionedElement::Text(GlyphRun {
        origin: Point {
            x: thumbnail.x,
            y: thumbnail.y + thumbnail.height + 11.0,
        },
        font_id,
        font_size: 9.0,
        glyph_ids: shaped.glyph_ids,
        advances: shaped.advances,
        text,
        source: None,
        color: Color::BLACK,
        bold: false,
        italic: false,
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
        tab_aligned: None,
    }))
}

#[cfg(feature = "render")]
fn push_three_up_note_rules(
    elements: &mut Vec<PositionedElement>,
    thumbnail: Rect,
    page_width: f64,
) {
    let start_x = thumbnail.x + thumbnail.width + 18.0;
    let end_x = page_width - 36.0;
    if end_x <= start_x {
        return;
    }
    for line in 1..=5 {
        let y = thumbnail.y + thumbnail.height * line as f64 / 6.0;
        elements.push(PositionedElement::Line {
            start: Point { x: start_x, y },
            end: Point { x: end_x, y },
            width: 0.5,
            color: Color::BLACK,
            dash_pattern: None,
        });
    }
}

#[cfg(feature = "render")]
enum AuthoringChange {
    Normal((u32, u32)),
    Shape((i64, i64, i64, i64)),
    Fit { size: u16, family: Option<String> },
}

#[cfg(feature = "render")]
fn authoring_fonts(presentation: &Presentation, files: &[(&str, &[u8])]) -> Result<FontManager> {
    let mut fonts = configured_font_manager(
        &presentation.package,
        &presentation.presentation_part,
        &presentation.presentation,
        &caller_font_files(files),
        false,
    )?;
    fonts.begin_layout();
    Ok(fonts)
}

#[cfg(feature = "render")]
fn keep_authoring_font_fact(
    diagnostic: &oxoffice_layout::Diagnostic,
    seen: &mut HashSet<&'static str>,
) -> bool {
    let request = ["Aptos", "Aptos Display"].into_iter().find(|request| {
        diagnostic
            .message
            .starts_with(&format!("Font '{request}' is approximate: "))
    });
    request.is_none_or(|request| seen.insert(request))
}

#[cfg(feature = "render")]
fn authoring_font_diagnostics(
    owner: &AutofitOwner,
    fonts: &mut FontManager,
    change: Option<&AuthoringChange>,
) -> Result<Vec<oxoffice_layout::Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();
    for context in &owner.contexts {
        let mut text = context.text.clone();
        match change {
            Some(AuthoringChange::Normal((scale, reduction))) => {
                text.autofit = ResolvedAutofit::Normal {
                    font_scale: Some(f64::from(*scale) / 100000.0),
                    line_spacing_reduction: Some(f64::from(*reduction) / 100000.0),
                }
            }
            Some(AuthoringChange::Fit { size, family }) => {
                restyle_authoring_text(&mut text, family.as_deref(), *size)
            }
            Some(AuthoringChange::Shape(_)) => text.autofit = ResolvedAutofit::None,
            None => {}
        }
        // Sizing trials use the same loaded faces, but their first-use facts
        // must not suppress this owner's successful final contributions.
        fonts.begin_layout();
        let diagnostic_start = fonts.current_font_diagnostics().len();
        let scope = fonts.begin_font_contribution_scope().ok_or_else(|| {
            autofit_failure(
                &owner.update.location,
                "font contribution capture unavailable",
            )
        })?;
        let trial = oxoffice_pptx_render::measure_authoring_text(
            &context.shape,
            &text,
            fonts,
            context.slide_index + 1,
            &context.directions,
        );
        let events = fonts
            .finish_font_contribution_scope(scope)
            .map_err(|e| autofit_failure(&owner.update.location, e))?
            .ok_or_else(|| {
                autofit_failure(
                    &owner.update.location,
                    "font contribution capture exceeded its bound",
                )
            })?;
        trial.map_err(|e| autofit_failure(&owner.update.location, e))?;
        // Nested shaping-cache scopes discard their approximation messages.
        // Regenerate only the captured successful physical contributions in
        // another bounded scope, retaining its facts before it rolls back.
        let fact_scope = fonts.begin_font_contribution_scope().ok_or_else(|| {
            autofit_failure(
                &owner.update.location,
                "font diagnostic capture unavailable",
            )
        })?;
        let replay = fonts.replay_font_trace_events(&events);
        let approximation_facts = fonts.current_font_diagnostics()[diagnostic_start..].to_vec();
        let finished = fonts.finish_font_contribution_scope(fact_scope);
        replay.map_err(|e| autofit_failure(&owner.update.location, e))?;
        finished
            .map_err(|e| autofit_failure(&owner.update.location, e))?
            .ok_or_else(|| {
                autofit_failure(
                    &owner.update.location,
                    "font diagnostic capture exceeded its bound",
                )
            })?;
        for fact in &approximation_facts {
            if !diagnostics.contains(fact) {
                diagnostics.push(fact.clone());
            }
        }
        for event in events {
            let oxoffice_layout::FontTraceEvent::Contribution(contribution) = event else {
                continue;
            };
            let Some(requested) = contribution.requested_family else {
                continue;
            };
            let actual = fonts
                .font_data(contribution.font_id)
                .map_err(|e| autofit_failure(&owner.update.location, e))?
                .family;
            let approximation_request = if requested.eq_ignore_ascii_case("Aptos") {
                "Aptos"
            } else if requested.eq_ignore_ascii_case("Aptos Display") {
                "Aptos Display"
            } else {
                &requested
            };
            if approximation_facts.iter().any(|fact| {
                fact.message
                    .starts_with(&format!("Font '{approximation_request}' is approximate: "))
            }) {
                continue;
            }
            if !requested.eq_ignore_ascii_case(&actual)
                && seen.insert((requested.clone(), actual.clone()))
            {
                diagnostics.push(oxoffice_layout::Diagnostic {
                    message: format!(
                        "{} {:?}: requested font {requested:?} measured with {actual:?}",
                        owner.update.location.owner_part, owner.update.location.child_path
                    ),
                });
            }
        }
    }
    Ok(diagnostics)
}

#[cfg(feature = "render")]
fn select_autofit_pair(
    owner: &AutofitOwner,
    fonts: &mut FontManager,
) -> Result<Option<(u32, u32)>> {
    for pair in oxoffice_pptx_render::AUTOFIT_CANDIDATES {
        let mut fits = true;
        for context in &owner.contexts {
            let (q, s) = oxoffice_pptx_render::measure_autofit_candidate(
                &context.shape,
                &context.text,
                fonts,
                context.slide_index + 1,
                &context.directions,
                pair,
            )
            .map_err(|e| autofit_failure(&owner.update.location, e))?;
            if !q.fits || !s.fits {
                fits = false;
                break;
            }
        }
        if fits {
            return Ok(Some(pair));
        }
    }
    Ok(None)
}

#[cfg(feature = "render")]
fn restyle_authoring_text(
    text: &mut oxoffice_pptx_layout::ResolvedTextBody,
    family: Option<&str>,
    size: u16,
) {
    text.autofit = ResolvedAutofit::None;
    text.wrap = true;
    let apply = |style: &mut oxoffice_pptx_layout::ResolvedRunStyle| {
        style.font_size = Some(f64::from(size));
        if let Some(family) = family {
            style.latin_typeface = Some(family.to_owned());
            style.east_asian_typeface = Some(family.to_owned());
            style.complex_script_typeface = Some(family.to_owned());
        }
    };
    for paragraph in &mut text.paragraphs {
        apply(&mut paragraph.end_style);
        for run in &mut paragraph.runs {
            if let oxoffice_pptx_layout::ResolvedTextRun::Text { style, .. }
            | oxoffice_pptx_layout::ResolvedTextRun::Field { style, .. } = run
            {
                apply(style);
            }
        }
    }
}

#[cfg(feature = "render")]
fn shape_autofit_bounds(
    owner: &AutofitOwner,
    context: &AutofitContext,
    fonts: &mut FontManager,
) -> Result<(i64, i64, i64, i64)> {
    let mut text = context.text.clone();
    text.autofit = ResolvedAutofit::None;
    let trial = oxoffice_pptx_render::measure_authoring_text(
        &context.shape,
        &text,
        fonts,
        context.slide_index + 1,
        &context.directions,
    )
    .map_err(|e| autofit_failure(&owner.update.location, e))?;
    if !trial.width_fits {
        return Err(autofit_failure(
            &owner.update.location,
            "shape text exceeds its fixed usable width",
        ));
    }
    let vertical = text.vertical != oxoffice_pptx_layout::TextDirection::Horizontal;
    let scaled_dimension = if vertical {
        context.shape.bounds.width
    } else {
        context.shape.bounds.height
    };
    let old_dimension = if vertical {
        owner.update.old_bounds.2
    } else {
        owner.update.old_bounds.3
    };
    if old_dimension <= 0 || scaled_dimension <= 0.0 {
        return Err(autofit_failure(
            &owner.update.location,
            "missing direct shape extent",
        ));
    }
    let scale = scaled_dimension / (old_dimension as f64 / 12700.0);
    let insets = if vertical {
        text.insets.left + text.insets.right
    } else {
        text.insets.top + text.insets.bottom
    };
    let needed = (trial.physical_height + insets) / scale;
    let needed_emu = checked_authoring_emu(needed, &owner.update.location)?;
    if needed_emu <= 0 {
        return Err(autofit_failure(
            &owner.update.location,
            "nonpositive resized extent",
        ));
    }
    let (x, y, w, h) = owner.update.old_bounds;
    let (nw, nh) = if vertical {
        (needed_emu, h)
    } else {
        (w, needed_emu)
    };
    let fraction = match text.anchor {
        oxoffice_pptx_layout::TextAnchor::Top => 0.0,
        oxoffice_pptx_layout::TextAnchor::Center => 0.5,
        oxoffice_pptx_layout::TextAnchor::Bottom => 1.0,
        oxoffice_pptx_layout::TextAnchor::Justified
        | oxoffice_pptx_layout::TextAnchor::Distributed => {
            return Err(autofit_failure(
                &owner.update.location,
                "distributed or justified shape anchor has no single preserved anchor",
            ));
        }
    };
    let reading_sign = if matches!(
        text.vertical,
        oxoffice_pptx_layout::TextDirection::Vertical
            | oxoffice_pptx_layout::TextDirection::EastAsianVertical
            | oxoffice_pptx_layout::TextDirection::WordArtVertical
    ) {
        -1.0
    } else {
        1.0
    };
    let scale_x = context.shape.bounds.width / (w as f64 / 12700.0);
    let scale_y = context.shape.bounds.height / (h as f64 / 12700.0);
    let mut resized = context.shape.clone();
    resized.bounds.width = nw as f64 / 12700.0 * scale_x;
    resized.bounds.height = nh as f64 / 12700.0 * scale_y;
    let anchor = |shape: &oxoffice_pptx_layout::ResolvedShape| {
        let local = if vertical {
            Point {
                x: (0.5 + (fraction - 0.5) * reading_sign) * shape.bounds.width,
                y: shape.bounds.height / 2.0,
            }
        } else {
            Point {
                x: shape.bounds.width / 2.0,
                y: fraction * shape.bounds.height,
            }
        };
        oxoffice_pptx_render::shape_transform(shape).apply(local)
    };
    let old_anchor = anchor(&context.shape);
    let resized_anchor = anchor(&resized);
    // Offsets are stored before the parent scale and affine group transform.
    // Invert that actual mapping rather than treating painted displacements
    // as unscaled coordinates or reconstructing a separate transform order.
    let group = context.shape.group_transform;
    let a = group.a * scale_x;
    let b = group.b * scale_x;
    let c = group.c * scale_y;
    let d = group.d * scale_y;
    let determinant = a * d - b * c;
    if ![
        a,
        b,
        c,
        d,
        determinant,
        old_anchor.x,
        old_anchor.y,
        resized_anchor.x,
        resized_anchor.y,
    ]
    .into_iter()
    .all(f64::is_finite)
        || determinant.abs() <= f64::EPSILON
    {
        return Err(autofit_failure(
            &owner.update.location,
            "noninvertible painted anchor mapping",
        ));
    }
    let dx = old_anchor.x - resized_anchor.x;
    let dy = old_anchor.y - resized_anchor.y;
    let nx = x as f64 + (d * dx - c * dy) / determinant * 12700.0;
    let ny = y as f64 + (-b * dx + a * dy) / determinant * 12700.0;
    let quantize = |value: f64| -> Result<i64> {
        if !value.is_finite() || !(-27_273_042_329_600.0..=27_273_042_316_900.0).contains(&value) {
            Err(autofit_failure(
                &owner.update.location,
                "unrepresentable anchored coordinate",
            ))
        } else {
            Ok(value.trunc() as i64)
        }
    };
    Ok((quantize(nx)?, quantize(ny)?, nw, nh))
}

#[cfg(feature = "render")]
fn checked_authoring_emu(points: f64, location: &TextFrameLocation) -> Result<i64> {
    let value = points * 12700.0;
    if !value.is_finite() || !(0.0..=27_273_042_316_900.0).contains(&value) {
        return Err(autofit_failure(location, "unrepresentable measured extent"));
    }
    Ok(value.trunc() as i64)
}

#[cfg(feature = "render")]
fn validate_autofit_authoring_namespaces(xml: &[u8], part: &str) -> Result<()> {
    // Ordinary import, exact source save and rendering remain permissive.
    // Only authoring refuses explicit foreign/rebound modeled lookalikes,
    // before local-name parsing could treat their payload as an autofit flag.
    let mut reader = NsReader::from_reader(xml);
    let mut depth = 0usize;
    let mut body_depth = None;
    let mut text_body_depth = None;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(|error| {
            render_failure(format!(
                "autofit authoring namespace preflight in {part}: {error}"
            ))
        })?;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                let name = start.name();
                let local = xml_local_name(name.as_ref());
                let text_body = local == b"txBody"
                    && (xml_is(
                        &namespace,
                        "http://schemas.openxmlformats.org/drawingml/2006/main",
                    ) || xml_is(
                        &namespace,
                        "http://schemas.openxmlformats.org/presentationml/2006/main",
                    ));
                let body = text_body_depth == Some(depth) && local == b"bodyPr";
                let choice = body_depth == Some(depth)
                    && matches!(local, b"normAutofit" | b"noAutofit" | b"spAutoFit");
                if (body || choice)
                    && !xml_is(
                        &namespace,
                        "http://schemas.openxmlformats.org/drawingml/2006/main",
                    )
                {
                    return Err(render_failure(format!(
                        "autofit authoring refuses foreign or unbound owner/choice namespace in {part}: {}",
                        String::from_utf8_lossy(name.as_ref())
                    )));
                }
                if text_body && !empty && text_body_depth.is_none() {
                    text_body_depth = Some(depth + 1);
                }
                if body && !empty {
                    body_depth = Some(depth + 1);
                }
                if !empty {
                    depth += 1;
                }
            }
            XmlEvent::End(_) => {
                if body_depth == Some(depth) {
                    body_depth = None;
                }
                if text_body_depth == Some(depth) {
                    text_body_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
fn apply_authoring_change(
    package: &mut OpcPackage,
    location: &TextFrameLocation,
    change: &AuthoringChange,
) -> Result<bool> {
    let original = required_part(package, &location.owner_part)?.to_vec();
    let content_type = package
        .content_types
        .content_type_for(&location.owner_part)
        .unwrap_or_default();
    macro_rules! update_root {
        ($root:ty) => {{
            let mut root =
                <$root>::from_xml(&original).map_err(|e| autofit_failure(location, e))?;
            let child = owned_child_mut(
                &mut root.common_slide_data.shape_tree.children,
                &location.child_path,
            )
            .ok_or_else(|| autofit_failure(location, "owner path disappeared"))?;
            // Each plan is tied to its own original source child. Multiple cell
            // plans may share a frame, so check them once in inventory/preflight.
            let original_child = child.clone();
            apply_owned_change(child, location, change)?;
            // A planned pair can already be stored. Keep its source part exact
            // instead of publishing unrelated canonical serialization changes.
            if *child == original_child {
                return Ok(false);
            }
            root.to_xml().map_err(|e| autofit_failure(location, e))?
        }};
    }
    let updated = match content_type {
        content_types::SLIDE => update_root!(CT_Slide),
        content_types::SLIDE_LAYOUT => update_root!(CT_SlideLayout),
        content_types::SLIDE_MASTER => update_root!(CT_SlideMaster),
        content_types::NOTES_SLIDE => update_root!(CT_NotesSlide),
        content_types::NOTES_MASTER => update_root!(CT_NotesMaster),
        _ => return Err(autofit_failure(location, "unsupported physical part kind")),
    };
    let changed = updated != original;
    if changed {
        package.set_part(&location.owner_part, updated);
    }
    Ok(changed)
}

#[cfg(feature = "render")]
fn apply_owned_change(
    child: &mut ShapeTreeChild,
    location: &TextFrameLocation,
    change: &AuthoringChange,
) -> Result<()> {
    if let AuthoringChange::Shape((x, y, w, h)) = change {
        let ShapeTreeChild::Shape(shape) = child else {
            return Err(autofit_failure(location, "resize owner is not a shape"));
        };
        let transform = shape
            .shape_properties
            .transform
            .get_or_insert_with(CT_Transform2D::default);
        transform.offset = Some(CT_Point2D {
            x: Emu(*x),
            y: Emu(*y),
        });
        transform.extent = Some(CT_PositiveSize2D {
            cx: Emu(*w),
            cy: Emu(*h),
        });
        return Ok(());
    }
    let body = match (child, location.cell) {
        (ShapeTreeChild::Shape(shape), None) => shape.text_body.as_mut(),
        (ShapeTreeChild::GraphicFrame(frame), Some((r, c))) => frame
            .graphic_data
            .table_mut()
            .and_then(|t| t.rows.get_mut(r))
            .and_then(|r| r.cells.get_mut(c))
            .and_then(|c| c.text_body.as_mut()),
        _ => None,
    }
    .ok_or_else(|| autofit_failure(location, "text owner disappeared"))?;
    match change {
        AuthoringChange::Normal((scale, reduction)) => {
            body.body_properties.autofit = Some(TextAutofit::Normal(NormalAutofit {
                font_scale: Some(scale.to_string()),
                line_spacing_reduction: Some(reduction.to_string()),
            }))
        }
        AuthoringChange::Fit { size, family } => {
            body.body_properties.wrap = Some(TextWrap::Square);
            body.body_properties.autofit = Some(TextAutofit::NoAutofit);
            let apply = |properties: &mut CT_TextCharacterProperties| {
                properties.font_size = Some(i32::from(*size) * 100);
                if let Some(family) = family {
                    for slot in [
                        &mut properties.latin,
                        &mut properties.east_asian,
                        &mut properties.complex_script,
                    ] {
                        if let Some(font) = slot {
                            font.typeface = family.clone();
                        } else {
                            *slot = Some(TextFont::new(family).expect("validated family"));
                        }
                    }
                }
            };
            for index in 0..body.paragraph_count() {
                let paragraph = body.paragraph_mut(index).expect("bounded paragraph");
                apply(
                    paragraph
                        .properties_mut()
                        .default_run_properties
                        .get_or_insert_with(CT_TextCharacterProperties::default),
                );
                apply(
                    paragraph
                        .end_properties
                        .get_or_insert_with(CT_TextCharacterProperties::default),
                );
                for run in &mut paragraph.runs {
                    match run {
                        TextRun::Run(run) => apply(
                            run.properties
                                .get_or_insert_with(CT_TextCharacterProperties::default),
                        ),
                        TextRun::Field(field) => apply(
                            field
                                .run_properties
                                .get_or_insert_with(CT_TextCharacterProperties::default),
                        ),
                        TextRun::Break(br) => apply(
                            br.properties
                                .get_or_insert_with(CT_TextCharacterProperties::default),
                        ),
                    }
                }
            }
        }
        AuthoringChange::Shape(_) => unreachable!(),
    }
    Ok(())
}

/// Whether `table`, its rows grown to fit their text, stays inside its page.
#[cfg(feature = "render")]
fn grown_table_fits_page(
    table: &AutofitTable,
    fonts: &mut FontManager,
    page_number: usize,
) -> std::result::Result<bool, oxoffice_pptx_render::RenderInputError> {
    let shape = &table.shape;
    let ResolvedContent::Table(content) = &shape.content else {
        return Ok(false);
    };
    let height = oxoffice_pptx_render::authoring_table_height(
        content,
        fonts,
        page_number,
        &table.directions,
    )?;
    let width = content.column_widths.iter().sum::<f64>();
    let rect = oxoffice_layout::Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    let bounds = oxoffice_pptx_render::shape_transform(shape).transform_rect_bbox(rect);
    Ok(bounds.x >= -0.01
        && bounds.y >= -0.01
        && bounds.x + bounds.width <= table.page.0 + 0.01
        && bounds.y + bounds.height <= table.page.1 + 0.01)
}

#[cfg(feature = "render")]
fn autofit_failure(location: &TextFrameLocation, detail: impl std::fmt::Display) -> Error {
    render_failure(format!(
        "autofit {} {:?} cell {:?}: {detail}",
        location.owner_part, location.child_path, location.cell
    ))
}

#[cfg(feature = "render")]
fn owned_child<'a>(children: &'a [ShapeTreeChild], path: &[usize]) -> Option<&'a ShapeTreeChild> {
    let (&index, rest) = path.split_first()?;
    let child = children.get(index)?;
    if rest.is_empty() {
        return Some(child);
    }
    let ShapeTreeChild::GroupShape(group) = child else {
        return None;
    };
    owned_child(&group.children, rest)
}

#[cfg(feature = "render")]
fn owned_child_mut<'a>(
    children: &'a mut [ShapeTreeChild],
    path: &[usize],
) -> Option<&'a mut ShapeTreeChild> {
    let (&index, rest) = path.split_first()?;
    let child = children.get_mut(index)?;
    if rest.is_empty() {
        return Some(child);
    }
    let ShapeTreeChild::GroupShape(group) = child else {
        return None;
    };
    owned_child_mut(&mut group.children, rest)
}

#[cfg(feature = "render")]
fn owned_text(child: &ShapeTreeChild, cell: Option<(usize, usize)>) -> Option<&CT_TextBody> {
    match (child, cell) {
        (ShapeTreeChild::Shape(shape), None) => shape.text_body.as_ref(),
        (ShapeTreeChild::GraphicFrame(frame), Some((row, column))) => {
            let GraphicDataPayload::Table(table) = frame.graphic_data.payload() else {
                return None;
            };
            table
                .rows
                .get(row)?
                .cells
                .get(column)
                .and_then(|cell| cell.text_body.as_ref())
        }
        _ => None,
    }
}

#[cfg(feature = "render")]
#[allow(clippy::too_many_arguments)] // Physical leaf operands plus the shared read-only scan state.
fn append_autofit_context(
    owners: &mut Vec<AutofitOwner>,
    ctx: &ResolveCtx<'_>,
    source: FlattenedSource,
    owner_identity: (&str, &str),
    consuming_context: (usize, &str),
    size: (f64, f64),
    read_only: bool,
    duplicates: &mut Vec<(String, Vec<usize>, oxoffice_layout::Diagnostic)>,
    hyperlinks: &ScopedHyperlinkTargets,
) -> Result<()> {
    let (part, kind) = owner_identity;
    let (slide_index, context_name) = consuming_context;
    let mut ids = HashSet::new();
    for (path, item) in ctx.owned_items(source) {
        let FlattenedItem::Shape {
            child,
            group_issues,
            ..
        } = &item
        else {
            continue;
        };
        if let Some(id) = child.non_visual_id()
            && !ids.insert(id)
        {
            if !read_only {
                return Err(render_failure(format!(
                    "autofit {part}: duplicate shape ID {id}"
                )));
            }
            // A read-only report measures a shape that repeats an ID and notes
            // it once for that physical leaf. A picture is skipped below and
            // still reaches the report diagnostics.
            duplicates.push((
                part.to_owned(),
                path.clone(),
                oxoffice_layout::Diagnostic {
                    message: format!(
                        "{part} {}: shape ID {id} is shared with an earlier shape",
                        plain_path(&path)
                    ),
                },
            ));
        }
        let cells: Vec<Option<(usize, usize)>> = match child {
            ShapeTreeChild::Shape(shape) if shape.text_body.is_some() => vec![None],
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(table) => table
                    .rows
                    .iter()
                    .enumerate()
                    .flat_map(|(r, row)| {
                        row.cells.iter().enumerate().map(move |(c, _)| Some((r, c)))
                    })
                    .collect(),
                GraphicDataPayload::SmartArt(_) => {
                    // Diagram text does not belong to this graphic frame's XML.
                    // It must remain an explicit unsupported owner in the report.
                    vec![None]
                }
                _ => Vec::new(),
            },
            ShapeTreeChild::AlternateContent(_) => vec![None],
            _ => Vec::new(),
        };
        if cells.is_empty() {
            continue;
        }
        let fingerprint = shape_ref(child).xml()?;
        let resolved = ctx.resolve_owned_text_shape(item, size, hyperlinks);
        let table = match &resolved {
            Ok((slide, directions)) => slide
                .shapes
                .first()
                .filter(|shape| matches!(shape.content, ResolvedContent::Table(_)))
                .map(|shape| {
                    Arc::new(AutofitTable {
                        shape: shape.clone(),
                        directions: directions.first().cloned().unwrap_or_default(),
                        page: size,
                    })
                }),
            Err(_) => None,
        };
        for cell in cells {
            let location = TextFrameLocation {
                owner_part: part.to_owned(),
                child_path: path.clone(),
                cell,
                fingerprint: fingerprint.clone(),
            };
            let body = owned_text(child, cell);
            let mut empty = body.is_some_and(|body| body.plain_text().is_empty());
            let mut unsupported = if *group_issues != 0 {
                Some("noninvertible or sheared group transform".to_owned())
            } else {
                None
            };
            let projection = match &resolved {
                Ok((slide, directions)) => {
                    slide
                        .shapes
                        .first()
                        .and_then(|shape| match (&shape.content, cell) {
                            (ResolvedContent::Text(text), None) => Some(Ok((
                                shape.clone(),
                                text.clone(),
                                directions.first().cloned().unwrap_or_default(),
                            ))),
                            (ResolvedContent::Table(table), Some((row, column))) => {
                                let ordinal = table
                                    .rows
                                    .iter()
                                    .take(row)
                                    .map(|r| r.cells.len())
                                    .sum::<usize>()
                                    + column;
                                Some(
                                    oxoffice_pptx_render::authoring_table_cell(
                                        shape, table, row, column,
                                    )
                                    .map(|(shape, text)| {
                                        (
                                            shape,
                                            text,
                                            vec![
                                                directions
                                                    .first()
                                                    .and_then(|d| d.get(ordinal))
                                                    .cloned()
                                                    .unwrap_or_default(),
                                            ],
                                        )
                                    })
                                    .map_err(|e| e.to_string()),
                                )
                            }
                            _ => None,
                        })
                }
                Err(error) => Some(Err(error.to_string())),
            };
            let mut contexts = Vec::new();
            let mut mode = body
                .and_then(|b| b.body_properties.autofit.as_ref())
                .map_or(AutofitMode::None, |a| match a {
                    TextAutofit::Normal(_) => AutofitMode::Normal,
                    TextAutofit::ShapeAutofit => AutofitMode::Shape,
                    _ => AutofitMode::None,
                });
            if body.is_none() {
                mode = opaque_autofit_modes(&fingerprint)?
                    .into_iter()
                    .find(|m| *m != AutofitMode::None)
                    .unwrap_or(AutofitMode::None);
            }
            if let Some(Ok((shape, text, directions))) = projection {
                empty = !text
                    .paragraphs
                    .iter()
                    .flat_map(|p| &p.runs)
                    .any(|run| match run {
                        oxoffice_pptx_layout::ResolvedTextRun::Text { text, .. }
                        | oxoffice_pptx_layout::ResolvedTextRun::Field { text, .. } => {
                            !text.is_empty()
                        }
                        oxoffice_pptx_layout::ResolvedTextRun::Break => true,
                    });
                if cell.is_none() {
                    mode = match text.autofit {
                        ResolvedAutofit::Normal { .. } => AutofitMode::Normal,
                        ResolvedAutofit::Shape => AutofitMode::Shape,
                        ResolvedAutofit::None => AutofitMode::None,
                    };
                }
                contexts.push(AutofitContext {
                    slide_index,
                    context: context_name.to_owned(),
                    shape,
                    text,
                    directions,
                    table: cell.and_then(|_| table.clone()),
                });
            } else {
                unsupported = Some(match projection {
                    Some(Err(detail)) => detail,
                    _ => "opaque or unresolved text owner".to_owned(),
                });
            }
            if cell.is_some() && mode == AutofitMode::Shape {
                unsupported =
                    Some("table-cell shape autofit has no independently owned extent".to_owned());
            }
            let effective_xfrm = match child {
                ShapeTreeChild::Shape(shape) => ctx.effective_xfrm(shape),
                _ => shape_transform(child).cloned(),
            };
            let xfrm = effective_xfrm.as_ref();
            let bounds = xfrm
                .and_then(|x| Some((x.offset?.x.0, x.offset?.y.0, x.extent?.cx.0, x.extent?.cy.0)))
                .unwrap_or((0, 0, 0, 0));
            if let Some(existing) = owners
                .iter_mut()
                .find(|owner| owner.update.location == location)
            {
                if existing.update.mode != mode {
                    return Err(autofit_failure(
                        &location,
                        "inconsistent inherited autofit mode across contexts",
                    ));
                }
                existing.contexts.extend(contexts);
                if unsupported.is_some() {
                    existing.unsupported = unsupported;
                }
            } else {
                owners.push(AutofitOwner {
                    update: AutofitUpdate {
                        location,
                        owner_kind: kind.to_owned(),
                        slide_index: (source == FlattenedSource::Slide
                            && matches!(kind, "slide" | "notes-slide"))
                        .then_some(slide_index),
                        shape_id: child.non_visual_id(),
                        name: child.non_visual_name(),
                        mode,
                        status: if empty { "empty" } else { "unchanged" }.to_owned(),
                        font_scale: None,
                        line_spacing_reduction: None,
                        point_size: None,
                        old_bounds: bounds,
                        new_bounds: bounds,
                        changed: false,
                        diagnostics: Vec::new(),
                    },
                    empty,
                    unsupported,
                    contexts,
                });
            }
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
/// The text owners of the deck and their measured contexts, or with
/// `slide_part` only those of that slide, measured on it.
#[allow(clippy::type_complexity)] // Owners plus the duplicate-leaf diagnostics they carry.
fn autofit_inventory(
    presentation: &Presentation,
    package: &OpcPackage,
    slide_part: Option<&str>,
    read_only: bool,
) -> Result<(
    Vec<AutofitOwner>,
    Vec<(String, Vec<usize>, oxoffice_layout::Diagnostic)>,
)> {
    let default_style = presentation
        .presentation
        .default_text_style
        .clone()
        .unwrap_or_default();
    let size = presentation.slide_size().map_or((720.0, 540.0), |(w, h)| {
        (w.0 as f64 / 12700.0, h.0 as f64 / 12700.0)
    });
    let mut owners = Vec::new();
    let mut duplicates = Vec::new();
    let table_styles = render_table_styles(package, &presentation.presentation_part)?;
    for (index, record) in presentation.slides.iter().enumerate() {
        if slide_part.is_some_and(|part| part != record.part_name) {
            continue;
        }
        let layout_part = render_related_part(package, &record.part_name, rel_types::SLIDE_LAYOUT)?;
        let master_part = render_related_part(package, &layout_part, rel_types::SLIDE_MASTER)?;
        let theme_part = render_related_part(package, &master_part, rel_types::THEME)?;
        let layout = CT_SlideLayout::from_xml(required_part(package, &layout_part)?)
            .map_err(|e| render_failure(format!("{layout_part}: {e}")))?;
        let master = CT_SlideMaster::from_xml(required_part(package, &master_part)?)
            .map_err(|e| render_failure(format!("{master_part}: {e}")))?;
        let theme = CT_OfficeStyleSheet::from_xml(required_part(package, &theme_part)?)
            .map_err(|e| render_failure(format!("{theme_part}: {e}")))?;
        let mut ctx = ResolveCtx::new(
            &theme,
            render_effective_color_map(&master, &layout, &record.slide),
            &master,
            &layout,
            &record.slide,
            &default_style,
        );
        if let Some(styles) = table_styles.as_ref() {
            ctx = ctx.with_table_styles(styles);
        }
        let links = ScopedHyperlinkTargets::default();
        for (source, part, kind) in [
            (FlattenedSource::Master, master_part.as_str(), "master"),
            (FlattenedSource::Layout, layout_part.as_str(), "layout"),
            (FlattenedSource::Slide, record.part_name.as_str(), "slide"),
        ] {
            append_autofit_context(
                &mut owners,
                &ctx,
                source,
                (part, kind),
                (index, &record.part_name),
                size,
                read_only,
                &mut duplicates,
                &links,
            )?;
        }
    }
    // Notes use their own theme, notes page dimensions and notesStyle. The
    // ordinary export overlays complete owned text bodies, not slide textStyle.
    if slide_part.is_none()
        && let Some(notes_master_part) = &presentation.notes_master_part
    {
        let notes_master = CT_NotesMaster::from_xml(required_part(package, notes_master_part)?)
            .map_err(|e| render_failure(format!("{notes_master_part}: {e}")))?;
        let theme_part = render_related_part(package, notes_master_part, rel_types::THEME)?;
        let theme = CT_OfficeStyleSheet::from_xml(required_part(package, &theme_part)?)
            .map_err(|e| render_failure(format!("{theme_part}: {e}")))?;
        let style = notes_master
            .notes_style
            .clone()
            .or(presentation.presentation.default_text_style.clone())
            .unwrap_or_default();
        if let Some(first) = presentation.slides.first() {
            let layout_part =
                render_related_part(package, &first.part_name, rel_types::SLIDE_LAYOUT)?;
            let master_part = render_related_part(package, &layout_part, rel_types::SLIDE_MASTER)?;
            let mut shell_layout = CT_SlideLayout::from_xml(required_part(package, &layout_part)?)
                .map_err(|e| render_failure(e.to_string()))?;
            shell_layout.common_slide_data.shape_tree = CT_ShapeTree::new();
            let mut shell_master = CT_SlideMaster::from_xml(required_part(package, &master_part)?)
                .map_err(|e| render_failure(e.to_string()))?;
            // Notes export resolves already composed complete text bodies with
            // empty inheritance trees. Do not merge notes-master bodyPr into
            // an overlaid physical notes-slide text body during measurement.
            shell_master.common_slide_data.shape_tree = CT_ShapeTree::new();
            shell_master.color_map = notes_master.color_map.clone();
            shell_master.text_styles = None;
            for (index, record) in presentation.slides.iter().enumerate() {
                let mut slide = CT_Slide::new(CT_ShapeTree::new());
                if let Some(notes) = &record.notes {
                    slide.common_slide_data = notes.notes.common_slide_data.clone();
                }
                // Geometry inheritance follows the existing notes overlay's
                // matched placeholder, but text remains the physical body.
                let reasons = materialize_notes_measurement_bounds(
                    &mut slide.common_slide_data.shape_tree.children,
                    &notes_master.common_slide_data.shape_tree.children,
                    read_only,
                )?;
                let ctx = ResolveCtx::new(
                    &theme,
                    notes_master.color_map.clone(),
                    &shell_master,
                    &shell_layout,
                    &slide,
                    &style,
                );
                let links = ScopedHyperlinkTargets::default();
                let mut master_slide = CT_Slide::new(CT_ShapeTree::new());
                master_slide.common_slide_data = notes_master.common_slide_data.clone();
                let master_ctx = ResolveCtx::new(
                    &theme,
                    notes_master.color_map.clone(),
                    &shell_master,
                    &shell_layout,
                    &master_slide,
                    &style,
                );
                append_autofit_context(
                    &mut owners,
                    &master_ctx,
                    FlattenedSource::Slide,
                    (notes_master_part, "notes-master"),
                    (index, &format!("{}:notes", record.part_name)),
                    export_page_size(&presentation.presentation),
                    read_only,
                    &mut duplicates,
                    &links,
                )?;
                if let Some(notes) = &record.notes {
                    append_autofit_context(
                        &mut owners,
                        &ctx,
                        FlattenedSource::Slide,
                        (&notes.part_name, "notes-slide"),
                        (index, &notes.part_name),
                        export_page_size(&presentation.presentation),
                        read_only,
                        &mut duplicates,
                        &links,
                    )?;
                    // A placeholder that the read-only type fallback could not
                    // resolve keeps its geometry unset and reports the reason.
                    for (path, reason) in reasons {
                        if let Some(owner) = owners.iter_mut().find(|owner| {
                            owner.update.location.owner_part == notes.part_name
                                && owner.update.location.child_path == path
                                && owner.update.location.cell.is_none()
                        }) {
                            owner.unsupported = Some(reason.clone());
                            if !owner.empty {
                                owner
                                    .update
                                    .diagnostics
                                    .push(oxoffice_layout::Diagnostic { message: reason });
                            }
                        }
                    }
                    // Measurement clones materialized inherited geometry. Bind
                    // mutation selectors to the original physical child bytes.
                    for owner in owners
                        .iter_mut()
                        .filter(|o| o.update.location.owner_part == notes.part_name)
                    {
                        if let Some(child) = owned_child(
                            &notes.notes.common_slide_data.shape_tree.children,
                            &owner.update.location.child_path,
                        ) {
                            owner.update.location.fingerprint = shape_ref(child).xml()?;
                        }
                    }
                }
            }
        }
    }
    // Scan unused shared parts too. Without an instantiated context no answer
    // is earned, so a flagged owner is retained as an explicit refusal.
    let mut part_names = package.parts.keys().collect::<Vec<_>>();
    part_names.sort();
    for part in part_names {
        let common = match package.content_types.content_type_for(part) {
            Some(content_types::SLIDE_MASTER) => Some(
                CT_SlideMaster::from_xml(required_part(package, part)?)
                    .map_err(|e| render_failure(e.to_string()))?
                    .common_slide_data,
            ),
            Some(content_types::SLIDE_LAYOUT) => Some(
                CT_SlideLayout::from_xml(required_part(package, part)?)
                    .map_err(|e| render_failure(e.to_string()))?
                    .common_slide_data,
            ),
            Some(content_types::NOTES_MASTER) => Some(
                CT_NotesMaster::from_xml(required_part(package, part)?)
                    .map_err(|e| render_failure(e.to_string()))?
                    .common_slide_data,
            ),
            _ => None,
        };
        if let Some(common) = common {
            append_uninstantiated_owners(&mut owners, part, &common.shape_tree.children, &[])?;
        } else if package
            .content_types
            .content_type_for(part)
            .is_some_and(|t| {
                t.ends_with("+xml")
                    && !matches!(t, content_types::SLIDE | content_types::NOTES_SLIDE)
            })
        {
            // Text in opaque diagram/chart owners has no writable facade body.
            // Retain each qualified body as unmeasured, not as a visible frame.
            let bytes = required_part(package, part)?;
            for (ordinal, mode) in opaque_autofit_modes(bytes)?.into_iter().enumerate() {
                if mode == AutofitMode::None {
                    continue;
                }
                let location = TextFrameLocation {
                    owner_part: part.to_owned(),
                    child_path: vec![ordinal],
                    cell: None,
                    fingerprint: bytes.to_vec(),
                };
                owners.push(AutofitOwner {
                    update: AutofitUpdate {
                        location,
                        owner_kind: "opaque-text".to_owned(),
                        slide_index: None,
                        shape_id: None,
                        name: Some(format!("opaque qualified body {ordinal}")),
                        mode,
                        status: "unsupported".to_owned(),
                        font_scale: None,
                        line_spacing_reduction: None,
                        point_size: None,
                        old_bounds: (0, 0, 0, 0),
                        new_bounds: (0, 0, 0, 0),
                        changed: false,
                        diagnostics: Vec::new(),
                    },
                    empty: false,
                    unsupported: Some(
                        "opaque text owner has no supported authoring projection".to_owned(),
                    ),
                    contexts: Vec::new(),
                });
            }
        }
    }
    owners.sort_by(|left, right| {
        (
            &left.update.location.owner_part,
            &left.update.location.child_path,
            left.update.location.cell,
        )
            .cmp(&(
                &right.update.location.owner_part,
                &right.update.location.child_path,
                right.update.location.cell,
            ))
    });
    Ok((owners, duplicates))
}

#[cfg(feature = "render")]
fn opaque_autofit_modes(xml: &[u8]) -> Result<Vec<AutofitMode>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ancestors = Vec::<(bool, bool)>::new();
    let mut body = None;
    let mut modes = Vec::new();
    loop {
        let depth = ancestors.len();
        let (is_a, known_namespace, text_body_namespace, event) = {
            let (ns, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|e| render_failure(format!("autofit owner XML: {e}")))?;
            let is_a = xml_is(&ns, A_NS);
            let is_p = xml_is(&ns, P_NS);
            let is_chart = xml_is(
                &ns,
                "http://schemas.openxmlformats.org/drawingml/2006/chart",
            );
            let is_drawing = xml_is(
                &ns,
                "http://schemas.microsoft.com/office/drawing/2008/diagram",
            );
            let known = is_a
                || is_p
                || is_chart
                || is_drawing
                || xml_is(
                    &ns,
                    "http://schemas.openxmlformats.org/drawingml/2006/diagram",
                )
                || xml_is(
                    &ns,
                    "http://schemas.openxmlformats.org/markup-compatibility/2006",
                );
            (is_a, known, (is_a || is_p || is_drawing, is_chart), event)
        };
        match event {
            XmlEvent::Start(start) => {
                let name = start.name();
                let local = xml_local_name(name.as_ref());
                let owned_parent = ancestors.last().is_some_and(|(_, text_body)| *text_body)
                    && ancestors.iter().all(|(known, _)| *known);
                if is_a && local == b"bodyPr" && owned_parent {
                    modes.push(AutofitMode::None);
                    body = Some((depth, modes.len() - 1));
                } else if let Some((parent, index)) = body
                    && depth == parent + 1
                    && is_a
                {
                    match xml_local_name(start.name().as_ref()) {
                        b"normAutofit" => modes[index] = AutofitMode::Normal,
                        b"spAutoFit" => modes[index] = AutofitMode::Shape,
                        _ => {}
                    }
                }
                let text_body = (text_body_namespace.0 && local == b"txBody")
                    || (text_body_namespace.1 && matches!(local, b"txPr" | b"rich"));
                ancestors.push((known_namespace, text_body));
            }
            XmlEvent::Empty(start) => {
                let name = start.name();
                let local = xml_local_name(name.as_ref());
                let owned_parent = ancestors.last().is_some_and(|(_, text_body)| *text_body)
                    && ancestors.iter().all(|(known, _)| *known);
                if is_a && local == b"bodyPr" && owned_parent {
                    modes.push(AutofitMode::None);
                } else if let Some((parent, index)) = body
                    && depth == parent + 1
                    && is_a
                {
                    match xml_local_name(start.name().as_ref()) {
                        b"normAutofit" => modes[index] = AutofitMode::Normal,
                        b"spAutoFit" => modes[index] = AutofitMode::Shape,
                        _ => {}
                    }
                }
            }
            XmlEvent::End(_) => {
                ancestors
                    .pop()
                    .ok_or_else(|| render_failure("unbalanced autofit owner XML"))?;
                if body.is_some_and(|(parent, _)| parent == ancestors.len()) {
                    body = None;
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(modes)
}

#[cfg(feature = "render")]
fn materialize_notes_measurement_bounds(
    children: &mut [ShapeTreeChild],
    master: &[ShapeTreeChild],
    read_only: bool,
) -> Result<Vec<(Vec<usize>, String)>> {
    let mut placeholders = Vec::new();
    collect_export_placeholder_shapes(master, &mut placeholders)?;
    let mut reasons = Vec::new();
    materialize_notes_bounds(
        children,
        &placeholders,
        &mut Vec::new(),
        read_only,
        &mut reasons,
    )?;
    Ok(reasons)
}

#[cfg(feature = "render")]
fn materialize_notes_bounds(
    children: &mut [ShapeTreeChild],
    placeholders: &[(PlaceholderKey, CT_Shape)],
    path: &mut Vec<usize>,
    read_only: bool,
    reasons: &mut Vec<(Vec<usize>, String)>,
) -> Result<()> {
    for (index, child) in children.iter_mut().enumerate() {
        path.push(index);
        match child {
            ShapeTreeChild::GroupShape(group) => materialize_notes_bounds(
                &mut group.children,
                placeholders,
                path,
                read_only,
                reasons,
            )?,
            ShapeTreeChild::Shape(shape) if shape.shape_properties.transform.is_none() => {
                if let Some(ph) = &shape.placeholder {
                    let key = ph.key();
                    let key_matches = placeholders
                        .iter()
                        .filter(|(existing, _)| existing.matches(&key))
                        .count();
                    let mut parent = None;
                    if key_matches == 1 {
                        parent = placeholders
                            .iter()
                            .find(|(existing, _)| existing.matches(&key))
                            .map(|(_, parent)| parent);
                    } else if read_only {
                        let by_type = PlaceholderKey {
                            ph_type: key.ph_type.clone(),
                            idx: None,
                        };
                        let typed = placeholders
                            .iter()
                            .filter(|(existing, _)| by_type.matches(existing))
                            .count();
                        if key_matches == 0 && typed == 1 {
                            parent = placeholders
                                .iter()
                                .find(|(existing, _)| by_type.matches(existing))
                                .map(|(_, parent)| parent);
                        } else {
                            let subject = key.idx.map_or_else(
                                || "has no idx".to_owned(),
                                |idx| format!("idx {idx} matches no notes-master placeholder"),
                            );
                            let count = if typed == 0 {
                                "no notes-master placeholder has its type".to_owned()
                            } else {
                                format!("{typed} notes-master placeholders have its type")
                            };
                            reasons.push((
                                path.clone(),
                                format!("notes placeholder {subject}, and {count}"),
                            ));
                        }
                    } else if key_matches > 1 {
                        return Err(render_failure("ambiguous notes placeholder geometry"));
                    }
                    if let Some(parent) = parent {
                        shape.shape_properties.transform =
                            parent.shape_properties.transform.clone();
                    }
                }
            }
            _ => {}
        }
        path.pop();
    }
    Ok(())
}

#[cfg(feature = "render")]
fn append_uninstantiated_owners(
    owners: &mut Vec<AutofitOwner>,
    part: &str,
    children: &[ShapeTreeChild],
    parent: &[usize],
) -> Result<()> {
    for (index, child) in children.iter().enumerate() {
        let mut path = parent.to_vec();
        path.push(index);
        if let ShapeTreeChild::GroupShape(group) = child {
            append_uninstantiated_owners(owners, part, &group.children, &path)?;
            continue;
        }
        let cells: Vec<Option<(usize, usize)>> = match child {
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(table) => table
                    .rows
                    .iter()
                    .enumerate()
                    .flat_map(|(row, value)| {
                        value
                            .cells
                            .iter()
                            .enumerate()
                            .map(move |(column, _)| Some((row, column)))
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => vec![None],
        };
        for cell in cells {
            if owners.iter().any(|owner| {
                owner.update.location.owner_part == part
                    && owner.update.location.child_path == path
                    && owner.update.location.cell == cell
            }) {
                continue;
            }
            let body = owned_text(child, cell);
            let fingerprint = shape_ref(child).xml()?;
            let mode = if let Some(body) = body {
                match body.body_properties.autofit {
                    Some(TextAutofit::Normal(_)) => AutofitMode::Normal,
                    Some(TextAutofit::ShapeAutofit) => AutofitMode::Shape,
                    _ => AutofitMode::None,
                }
            } else {
                // Retain the physical opaque child as unsupported, never as a
                // writable fallback. Use the same qualified scan as active owners.
                opaque_autofit_modes(&fingerprint)?
                    .into_iter()
                    .find(|mode| *mode != AutofitMode::None)
                    .unwrap_or(AutofitMode::None)
            };
            let empty = body.is_some_and(|body| body.plain_text().is_empty());
            if mode == AutofitMode::None {
                continue;
            }
            let location = TextFrameLocation {
                owner_part: part.to_owned(),
                child_path: path.clone(),
                cell,
                fingerprint,
            };
            owners.push(AutofitOwner {
                update: AutofitUpdate {
                    location,
                    owner_kind: "uninstantiated-shared".to_owned(),
                    slide_index: None,
                    shape_id: child.non_visual_id(),
                    name: child.non_visual_name(),
                    mode,
                    status: if empty { "empty" } else { "unsupported" }.to_owned(),
                    font_scale: None,
                    line_spacing_reduction: None,
                    point_size: None,
                    old_bounds: (0, 0, 0, 0),
                    new_bounds: (0, 0, 0, 0),
                    changed: false,
                    diagnostics: Vec::new(),
                },
                empty,
                unsupported: Some("shared flagged owner has no instantiated context".to_owned()),
                contexts: Vec::new(),
            });
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
pub(super) struct PreparedSlideAssembly {
    slide_part: String,
    slide: CT_Slide,
    layout: CT_SlideLayout,
    master: CT_SlideMaster,
    theme: CT_OfficeStyleSheet,
    media: ScopedMediaIds,
    media_failures: ScopedMediaFailures,
    hyperlinks: ScopedHyperlinkTargets,
    charts: ScopedChartResources,
    media_diagnostics: Vec<oxoffice_layout::Diagnostic>,
    smartart_clips: Vec<Option<Rect>>,
}

#[cfg(feature = "render")]
/// The deck's main language: the language of the presentation's default
/// text, its default paragraph properties first, then its first level.
fn deck_language(style: &CT_TextListStyle) -> Option<String> {
    [&style.default_paragraph_properties, &style.level1]
        .into_iter()
        .flatten()
        .filter_map(|properties| properties.default_run_properties.as_ref()?.language())
        .map(str::trim)
        .find(|language| oxoffice_layout::is_language_tag(language))
        .map(str::to_owned)
}

pub(super) fn prepare_render_context(
    package: &OpcPackage,
    collect_media_diagnostics: bool,
    font_files: &[oxoffice_layout::FontFile],
    system_fallback: bool,
) -> Result<PreparedRenderAssembly> {
    let presentation_part = package
        .main_document_part()
        .ok_or(Error::MissingMainDocument)?;
    let presentation = CT_Presentation::from_xml(required_part(package, &presentation_part)?)
        .map_err(|error| Error::MalformedPart {
            part_name: presentation_part.clone(),
            message: error.to_string(),
        })?;
    let default_text_style = presentation.default_text_style.clone().unwrap_or_default();
    let size = presentation
        .slide_size
        .as_ref()
        .map_or((720.0, 540.0), |size| {
            (size.cx.0 as f64 / 12_700.0, size.cy.0 as f64 / 12_700.0)
        });
    let table_styles = render_table_styles(package, &presentation_part)?;
    let presentation_relationships = package
        .get_part_rels(&presentation_part)
        .ok_or_else(|| render_failure("presentation relationships are missing"))?;

    let mut media = HashMap::new();
    let mut font_manager = configured_font_manager(
        package,
        &presentation_part,
        &presentation,
        font_files,
        system_fallback,
    )?;
    let mut resolved_slides = Vec::with_capacity(presentation.slide_ids.len());
    let mut text_directions = Vec::with_capacity(presentation.slide_ids.len());
    let mut prepared_slides = Vec::with_capacity(presentation.slide_ids.len());
    for (slide_index, slide_id) in presentation.slide_ids.iter().enumerate() {
        let slide_relationship = presentation_relationships
            .get_by_id(&slide_id.relationship_id)
            .ok_or_else(|| Error::MissingRelationship {
                source_part: presentation_part.clone(),
                relationship_id: slide_id.relationship_id.clone(),
            })?;
        require_relationship_type(&presentation_part, slide_relationship, rel_types::SLIDE)?;
        reject_external(&presentation_part, slide_relationship)?;
        let slide_part =
            OpcPackage::resolve_rel_target(&presentation_part, &slide_relationship.target);
        let layout_part = render_related_part(package, &slide_part, rel_types::SLIDE_LAYOUT)?;
        let master_part = render_related_part(package, &layout_part, rel_types::SLIDE_MASTER)?;
        let theme_part = render_related_part(package, &master_part, rel_types::THEME)?;
        let mut slide =
            CT_Slide::from_xml(required_part(package, &slide_part)?).map_err(|error| {
                Error::MalformedPart {
                    part_name: slide_part.clone(),
                    message: error.to_string(),
                }
            })?;
        let mut layout =
            CT_SlideLayout::from_xml(required_part(package, &layout_part)?).map_err(|error| {
                Error::MalformedPart {
                    part_name: layout_part.clone(),
                    message: error.to_string(),
                }
            })?;
        let mut master =
            CT_SlideMaster::from_xml(required_part(package, &master_part)?).map_err(|error| {
                Error::MalformedPart {
                    part_name: master_part.clone(),
                    message: error.to_string(),
                }
            })?;
        let theme = CT_OfficeStyleSheet::from_xml(required_part(package, &theme_part)?).map_err(
            |error| Error::MalformedPart {
                part_name: theme_part.clone(),
                message: error.to_string(),
            },
        )?;
        let (slide_media, slide_media_failures, slide_hyperlinks, slide_charts, slide_diagrams) =
            render_scoped_resources(
                package,
                [&slide_part, &layout_part, &master_part],
                [
                    &slide.common_slide_data.shape_tree.children,
                    &layout.common_slide_data.shape_tree.children,
                    &master.common_slide_data.shape_tree.children,
                ],
                &mut media,
            )?;
        let mut slide_expansion = diagram::expand_tree(
            &mut slide.common_slide_data.shape_tree,
            &slide_diagrams.slide,
        );
        let mut layout_expansion = diagram::expand_tree(
            &mut layout.common_slide_data.shape_tree,
            &slide_diagrams.layout,
        );
        let mut master_expansion = diagram::expand_tree(
            &mut master.common_slide_data.shape_tree,
            &slide_diagrams.master,
        );
        let mut smartart_diagnostics = std::mem::take(&mut slide_expansion.diagnostics);
        smartart_diagnostics.append(&mut layout_expansion.diagnostics);
        smartart_diagnostics.append(&mut master_expansion.diagnostics);
        let color_map = render_effective_color_map(&master, &layout, &slide);
        let mut context = ResolveCtx::new(
            &theme,
            color_map,
            &master,
            &layout,
            &slide,
            &default_text_style,
        )
        .with_media_failures(&slide_media_failures);
        if let Some(styles) = table_styles.as_ref() {
            context = context.with_table_styles(styles);
        }
        let smartart_source_clips = context
            .flatten()
            .into_iter()
            .filter(|item| render_source_shape_has_bounds(&context, item))
            .map(|item| {
                render_source_smartart_clip(
                    &item,
                    &slide_expansion.clips,
                    &layout_expansion.clips,
                    &master_expansion.clips,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let source_count = smartart_source_clips.len();
        let (mut resolved, slide_text_directions) = context
            .resolve_slide_with_chart_resources_and_text_directions(
                size,
                &slide_media,
                &slide_hyperlinks,
                &slide_charts,
                &mut font_manager,
            )
            .map_err(|error| render_failure(format!("{slide_part}: {error}")))?;
        resolved.diagnostics.append(&mut smartart_diagnostics);

        let prepared_media_diagnostics = if collect_media_diagnostics {
            let media_infos = media_infos_for_slide(package, slide_index, &slide_part, &slide)?;
            package_media_diagnostics(&media_infos)
        } else {
            Vec::new()
        };
        if source_count != resolved.shapes.len() {
            return Err(render_failure(format!(
                "{slide_part}: dropped bounded source shape, source {source_count}, resolved {}",
                resolved.shapes.len()
            )));
        }
        for (clip, shape) in smartart_source_clips.iter().zip(&resolved.shapes) {
            if let Some(clip) = clip {
                smartart_local_clip(*clip, shape.bounds)?;
            }
        }
        resolved_slides.push(resolved);
        text_directions.push(slide_text_directions);
        prepared_slides.push(PreparedSlideAssembly {
            slide_part,
            slide,
            layout,
            master,
            theme,
            media: slide_media,
            media_failures: slide_media_failures,
            hyperlinks: slide_hyperlinks,
            charts: slide_charts,
            media_diagnostics: prepared_media_diagnostics,
            smartart_clips: smartart_source_clips,
        });
    }

    let input = RenderInput {
        slides: resolved_slides,
        media,
        fonts: font_files.to_vec(),
        metadata: deck_language(&default_text_style).map(|language| {
            oxoffice_layout::DocumentMetadata {
                language: Some(language),
                ..Default::default()
            }
        }),
    };
    let mut layout = layout_presentation_with_displayed_numbers(
        &input,
        &mut font_manager,
        Some(&text_directions),
        presentation
            .first_slide_number()
            .map_err(|error| render_failure(error.to_string()))?
            .unwrap_or(1) as usize,
    )
    .map_err(|error| render_failure(error.to_string()))?;
    if layout.pages.len() != input.slides.len() {
        return Err(render_failure(format!(
            "rendered {} pages for {} slides",
            layout.pages.len(),
            input.slides.len()
        )));
    }
    for ((page, slide), prepared) in layout
        .pages
        .iter_mut()
        .zip(&input.slides)
        .zip(&prepared_slides)
    {
        apply_smartart_clips(
            std::sync::Arc::make_mut(page),
            slide,
            &prepared.smartart_clips,
        )?;
    }
    Ok(PreparedRenderAssembly {
        first_slide_number: presentation
            .first_slide_number()
            .map_err(|error| render_failure(error.to_string()))?
            .unwrap_or(1) as usize,
        input,
        layout,
        font_manager,
        slides: prepared_slides,
        text_directions,
        size,
        default_text_style,
        table_styles,
    })
}

#[cfg(feature = "render")]
fn evaluate_slide_media(
    slide: &CT_Slide,
    position: TimelinePosition,
) -> (Vec<EvaluatedMediaState>, Vec<oxoffice_layout::Diagnostic>) {
    let mut specs = Vec::new();
    collect_slide_media_specs(&slide.common_slide_data.shape_tree.children, &mut specs);
    let mut states = Vec::with_capacity(specs.len());
    let mut diagnostics = Vec::new();
    for (shape_id, _, trim_start_ms, trim_end_ms) in specs {
        let (state, mut media_diagnostics) = evaluate_media_playback(
            slide.timing.as_ref(),
            shape_id,
            trim_start_ms,
            trim_end_ms,
            position,
        );
        states.push(state);
        diagnostics.append(&mut media_diagnostics);
    }
    (states, diagnostics)
}

#[cfg(feature = "render")]
fn package_media_diagnostics(media_infos: &[MediaInfo]) -> Vec<oxoffice_layout::Diagnostic> {
    media_infos
        .iter()
        .flat_map(|media| {
            media
                .diagnostics
                .iter()
                .map(move |diagnostic| oxoffice_layout::Diagnostic {
                    message: match diagnostic {
                        MediaDiagnostic::UnsupportedContentType { content_type } => format!(
                            "media shape {} has unsupported content type `{content_type}`",
                            media.shape_id
                        ),
                        MediaDiagnostic::MissingPlaybackTiming => {
                            format!("media shape {} has no playback timing", media.shape_id)
                        }
                        MediaDiagnostic::UnsupportedPlaybackCommand { command } => format!(
                            "media shape {} has unsupported playback command `{command}`",
                            media.shape_id
                        ),
                        MediaDiagnostic::MissingPoster => {
                            format!("media shape {} has no poster relationship", media.shape_id)
                        }
                    },
                })
        })
        .collect()
}

#[cfg(feature = "render")]
fn suppress_consumed_media_timing_diagnostics(
    diagnostics: &mut Vec<oxoffice_layout::Diagnostic>,
    timing: Option<&CT_Timing>,
    consumed_shape_ids: &HashSet<u32>,
) {
    const RETAINED_MEDIA_TIMING: &str = "media timing is retained for synchronized playback";

    let mut suppressions = Vec::new();
    if let Some(timing) = timing {
        collect_media_timing_suppressions(timing.nodes(), consumed_shape_ids, &mut suppressions);
    }
    let mut marker_index = 0;
    diagnostics.retain(|diagnostic| {
        if diagnostic.message != RETAINED_MEDIA_TIMING {
            return true;
        }
        let suppress = suppressions.get(marker_index).copied().unwrap_or(false);
        marker_index += 1;
        !suppress
    });
}

#[cfg(feature = "render")]
fn collect_media_timing_suppressions(
    nodes: &[TimingNode],
    consumed_shape_ids: &HashSet<u32>,
    suppressions: &mut Vec<bool>,
) {
    for node in nodes {
        match node {
            TimingNode::Audio(media) | TimingNode::Video(media) => suppressions.push(
                matches!(media.common.target, TimingTarget::Shape(shape_id) if consumed_shape_ids.contains(&shape_id)),
            ),
            TimingNode::MediaCommand(command) => suppressions.push(
                matches!(command.target, TimingTarget::Shape(shape_id) if consumed_shape_ids.contains(&shape_id)),
            ),
            TimingNode::Parallel(container) => collect_media_timing_suppressions(
                &container.common.children,
                consumed_shape_ids,
                suppressions,
            ),
            TimingNode::Sequence(sequence) => collect_media_timing_suppressions(
                &sequence.common.children,
                consumed_shape_ids,
                suppressions,
            ),
            TimingNode::Set(_)
            | TimingNode::Animate(_)
            | TimingNode::Effect(_)
            | TimingNode::Motion(_)
            | TimingNode::Unsupported(_) => {}
        }
    }
}

#[cfg(feature = "render")]
fn collect_slide_media_specs(
    children: &[ShapeTreeChild],
    specs: &mut Vec<(u32, MediaKind, Option<u64>, Option<u64>)>,
) {
    for child in children {
        match child {
            ShapeTreeChild::Picture(picture) => {
                if let Some(media) = picture.media.as_ref()
                    && let Some(shape_id) = child.non_visual_id()
                {
                    let (trim_start_ms, trim_end_ms) = picture.media_trim_bounds();
                    specs.push((shape_id, media.kind, trim_start_ms, trim_end_ms));
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                collect_slide_media_specs(&group.children, specs);
            }
            _ => {}
        }
    }
}

#[cfg(feature = "render")]
fn apply_media_fallback_policy(
    timeline: &mut ResolvedTimelineSlide,
    slide: &CT_Slide,
    policy: MediaFallbackPolicy,
    fonts: &mut FontManager,
) -> Result<()> {
    let mut specs = Vec::new();
    collect_slide_media_specs(&slide.common_slide_data.shape_tree.children, &mut specs);
    let kinds = specs
        .into_iter()
        .map(|(shape_id, kind, _, _)| (shape_id, kind))
        .collect::<HashMap<_, _>>();
    for (shape, identity) in timeline.slide.shapes.iter_mut().zip(&timeline.identities) {
        if identity.source != FlattenedSource::Slide {
            continue;
        }
        let Some(shape_id) = identity.shape_id else {
            continue;
        };
        let Some(kind) = kinds.get(&shape_id).copied() else {
            continue;
        };
        let poster_resolved = matches!(shape.content, ResolvedContent::Image(_));
        if poster_resolved && policy != MediaFallbackPolicy::DeterministicPlaceholder {
            continue;
        }
        if policy == MediaFallbackPolicy::Fail {
            return Err(render_failure(format!(
                "media shape {shape_id} has no renderer-compatible poster"
            )));
        }
        let label = match kind {
            MediaKind::Audio => "Audio",
            MediaKind::Video => "Video",
        };
        shape.content = ResolvedContent::Group(render_media_placeholder(
            label,
            shape.bounds.width,
            shape.bounds.height,
            fonts,
        )?);
        shape.unsupported = Some("media poster");
        let fallback_message =
            format!("media shape {shape_id} rendered as deterministic {label} placeholder");
        let poster_diagnostic_prefix =
            format!("unresolved slide media poster for shape {shape_id}:");
        if let Some(diagnostic) = timeline
            .slide
            .diagnostics
            .iter_mut()
            .find(|diagnostic| diagnostic.message.starts_with(&poster_diagnostic_prefix))
        {
            diagnostic.message = fallback_message;
        } else {
            timeline
                .slide
                .diagnostics
                .push(oxoffice_layout::Diagnostic {
                    message: fallback_message,
                });
        }
    }
    restore_media_poster_diagnostic_messages(&mut timeline.slide.diagnostics);
    Ok(())
}

#[cfg(feature = "render")]
fn restore_media_poster_diagnostic_messages(diagnostics: &mut [oxoffice_layout::Diagnostic]) {
    for diagnostic in diagnostics {
        let Some(message) = legacy_media_poster_diagnostic_message(&diagnostic.message) else {
            continue;
        };
        diagnostic.message = message.to_owned();
    }
}

#[cfg(feature = "render")]
fn legacy_media_poster_diagnostic_message(message: &str) -> Option<&str> {
    ["slide", "layout", "master"]
        .into_iter()
        .find_map(|source| {
            let identity =
                message.strip_prefix(&format!("unresolved {source} media poster for shape "))?;
            let (shape_id, legacy) = identity.split_once(": ")?;
            (!shape_id.is_empty() && shape_id.chars().all(|character| character.is_ascii_digit()))
                .then_some(legacy)
        })
}

#[cfg(feature = "render")]
fn render_media_placeholder(
    label: &str,
    width: f64,
    height: f64,
    fonts: &mut FontManager,
) -> Result<GroupElement> {
    let font_id = fonts
        .resolve_font_for_text(Some("Carlito"), false, false, label)
        .map_err(|error| render_failure(format!("media fallback font: {error}")))?;
    let shaped = fonts
        .shape_text(font_id, label, 9.0)
        .map_err(|error| render_failure(format!("media fallback shaping: {error}")))?;
    Ok(GroupElement {
        transform: Transform::IDENTITY,
        clip: Some(LayoutPath::rect(Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        })),
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![PositionedElement::Text(GlyphRun {
            origin: Point {
                x: 4.0,
                y: (height / 2.0).max(9.0),
            },
            font_id,
            font_size: 9.0,
            glyph_ids: shaped.glyph_ids,
            advances: shaped.advances,
            text: label.to_owned(),
            source: None,
            color: Color::BLACK,
            bold: false,
            italic: false,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        })],
    })
}

#[cfg(feature = "render")]
pub(super) fn render_failure(message: impl Into<String>) -> Error {
    Error::Render {
        message: message.into(),
    }
}

#[cfg(feature = "render")]
fn render_table_styles(
    package: &OpcPackage,
    presentation_part: &str,
) -> Result<Option<CT_TableStyleList>> {
    let Some(relationship) = package
        .get_part_rels(presentation_part)
        .and_then(|relationships| relationships.get_by_type(rel_types::TABLE_STYLES))
    else {
        return Ok(None);
    };
    let target = OpcPackage::resolve_rel_target(presentation_part, &relationship.target);
    CT_TableStyleList::from_xml(required_part(package, &target)?)
        .map(Some)
        .map_err(|error| Error::MalformedPart {
            part_name: target,
            message: error.to_string(),
        })
}

#[cfg(feature = "render")]
fn render_scoped_resources(
    package: &OpcPackage,
    parts: [&str; 3],
    shape_trees: [&[ShapeTreeChild]; 3],
    deck_media: &mut HashMap<MediaId, MediaData>,
) -> Result<(
    ScopedMediaIds,
    ScopedMediaFailures,
    ScopedHyperlinkTargets,
    ScopedChartResources,
    ScopedDiagramResources,
)> {
    let slide = render_part_resources(package, parts[0], shape_trees[0], deck_media)?;
    let layout = render_part_resources(package, parts[1], shape_trees[1], deck_media)?;
    let master = render_part_resources(package, parts[2], shape_trees[2], deck_media)?;
    Ok((
        ScopedMediaIds {
            slide: slide.media_ids,
            layout: layout.media_ids,
            master: master.media_ids,
            media_content_types: deck_media
                .iter()
                .map(|(media_id, media)| {
                    // These relationship IDs have already passed render_compatible_media.
                    // Preserve the package declaration while reporting the effective raster type.
                    let content_type = match ImageFormat::sniff(&media.bytes) {
                        Some(ImageFormat::Png) => "image/png",
                        Some(ImageFormat::Jpeg) => "image/jpeg",
                        _ => &media.content_type,
                    };
                    (*media_id, content_type.to_owned())
                })
                .collect(),
        },
        ScopedMediaFailures {
            slide: slide.media_failures,
            layout: layout.media_failures,
            master: master.media_failures,
        },
        ScopedHyperlinkTargets {
            slide: slide.hyperlinks,
            layout: layout.hyperlinks,
            master: master.hyperlinks,
        },
        ScopedChartResources {
            slide: slide.charts,
            layout: layout.charts,
            master: master.charts,
        },
        ScopedDiagramResources {
            slide: slide.diagrams,
            layout: layout.diagrams,
            master: master.diagrams,
        },
    ))
}

#[cfg(feature = "render")]
struct RenderPartResources {
    media_ids: HashMap<String, MediaId>,
    media_failures: HashMap<String, String>,
    hyperlinks: HashMap<String, String>,
    charts: HashMap<String, ChartResource>,
    diagrams: HashMap<String, DiagramResources>,
}

#[cfg(feature = "render")]
fn render_part_resources(
    package: &OpcPackage,
    source_part: &str,
    shape_tree: &[ShapeTreeChild],
    deck_media: &mut HashMap<MediaId, MediaData>,
) -> Result<RenderPartResources> {
    let mut media_ids = HashMap::new();
    let mut media_failures = HashMap::new();
    let mut hyperlinks = HashMap::new();
    let mut charts = HashMap::new();
    let diagrams = render_part_diagram_resources(package, source_part, shape_tree);
    let Some(relationships) = package.get_part_rels(source_part) else {
        return Ok(RenderPartResources {
            media_ids,
            media_failures,
            hyperlinks,
            charts,
            diagrams,
        });
    };
    for relationship in &relationships.items {
        let external = relationship
            .target_mode
            .as_deref()
            .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
        if relationship.rel_type == rel_types::HYPERLINK && external {
            hyperlinks.insert(relationship.id.clone(), relationship.target.clone());
        }
        if relationship.rel_type == rel_types::CHART {
            let resource = if external {
                ChartResource::External(relationship.target.clone())
            } else {
                let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                match package.get_part(&target) {
                    Some(bytes) => CT_ChartSpace::from_xml(bytes)
                        .map(|chart| ChartResource::Parsed(Box::new(chart)))
                        .unwrap_or_else(|error| ChartResource::Invalid(error.to_string())),
                    None => ChartResource::MissingTarget(target),
                }
            };
            charts.insert(relationship.id.clone(), resource);
        }
        if relationship.rel_type != rel_types::IMAGE || external {
            continue;
        }
        let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
        let Some(bytes) = package.get_part(&target) else {
            continue;
        };
        let content_type = package
            .content_types
            .content_type_for(&target)
            .unwrap_or("application/octet-stream")
            .to_owned();
        match render_compatible_media(bytes, &content_type) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(message) => {
                media_failures.insert(relationship.id.clone(), message.to_owned());
                continue;
            }
        }
        let media_id = MediaId::from_bytes(bytes);
        deck_media.entry(media_id).or_insert_with(|| MediaData {
            bytes: bytes.to_vec(),
            content_type,
        });
        media_ids.insert(relationship.id.clone(), media_id);
    }
    Ok(RenderPartResources {
        media_ids,
        media_failures,
        hyperlinks,
        charts,
        diagrams,
    })
}

#[cfg(feature = "render")]
fn render_part_diagram_resources(
    package: &OpcPackage,
    source_part: &str,
    shape_tree: &[ShapeTreeChild],
) -> HashMap<String, DiagramResources> {
    let mut frames = Vec::new();
    collect_smartart_frames(shape_tree, &mut frames);
    let mut resources = HashMap::new();
    for (frame, _) in frames {
        let GraphicDataPayload::SmartArt(relationship_ids) = frame.graphic_data.payload() else {
            continue;
        };
        let mut relationships = (**relationship_ids).clone();
        let data_part = resolve_diagram_part(
            package,
            source_part,
            &relationships.data,
            rel_types::DIAGRAM_DATA,
            CT_DiagramData::from_xml,
        );
        let drawing_id = match &data_part {
            DiagramPart::Parsed(data) => data.drawing_relationship_id().map(str::to_owned),
            _ => None,
        };
        relationships.drawing = drawing_id.clone();
        let diagram = DiagramResources {
            relationships: relationships.clone(),
            data: diagram_part_result(data_part),
            layout: diagram_part_result(resolve_diagram_part(
                package,
                source_part,
                &relationships.layout,
                rel_types::DIAGRAM_LAYOUT,
                CT_DiagramLayoutDefinition::from_xml,
            )),
            style: diagram_part_result(resolve_diagram_part(
                package,
                source_part,
                &relationships.style,
                rel_types::DIAGRAM_QUICK_STYLE,
                CT_DiagramStyleDefinition::from_xml,
            )),
            colors: diagram_part_result(resolve_diagram_part(
                package,
                source_part,
                &relationships.colors,
                rel_types::DIAGRAM_COLORS,
                CT_DiagramColorsDefinition::from_xml,
            )),
        };
        resources
            .entry(relationships.data.clone())
            .and_modify(|existing: &mut DiagramResources| {
                if existing.relationships != relationships {
                    existing.data = Err(format!(
                        "conflicting SmartArt relationship sets share data id `{}`",
                        relationships.data
                    ));
                }
            })
            .or_insert(diagram);
    }
    resources
}

#[cfg(feature = "render")]
fn diagram_part_result<T>(part: DiagramPart<T>) -> std::result::Result<Box<T>, String> {
    match part {
        DiagramPart::Parsed(value) => Ok(Box::new(value)),
        DiagramPart::External(target) => Err(format!("external SmartArt target `{target}`")),
        DiagramPart::MissingTarget(target) => Err(format!("missing SmartArt target `{target}`")),
        DiagramPart::Invalid(detail) => Err(format!("invalid SmartArt part: {detail}")),
    }
}

#[cfg(feature = "render")]
fn render_compatible_media(
    bytes: &[u8],
    content_type: &str,
) -> std::result::Result<bool, &'static str> {
    let format = ImageFormat::sniff(bytes);
    if let Some(format @ (ImageFormat::Png | ImageFormat::Jpeg)) = format
        && !render_media_within_decode_bounds(bytes, format)?
    {
        return Err("could not be decoded");
    }
    match oxoffice_pdf::classify_image_draw(bytes, content_type) {
        oxoffice_pdf::ImageDrawClass::UnsupportedFormat { kind: "emf" } => {
            return Err("uses unsupported image format emf");
        }
        oxoffice_pdf::ImageDrawClass::UnsupportedFormat { .. } => {
            return Err("uses unsupported image format wmf");
        }
        oxoffice_pdf::ImageDrawClass::CouldNotDecode => return Err("could not be decoded"),
        oxoffice_pdf::ImageDrawClass::NotThisDecoder => return Ok(false),
        oxoffice_pdf::ImageDrawClass::Drawn => {}
    }
    if format == Some(ImageFormat::Emf) {
        return Ok(true);
    }
    let Some(info) = probe(bytes).filter(|info| info.width_px > 0 && info.height_px > 0) else {
        return Err("could not be decoded");
    };
    if format == Some(ImageFormat::Jpeg) && (info.bit_depth != 8 || info.channels != 3) {
        return Ok(false);
    }
    let width = f64::from(info.width_px);
    let height = f64::from(info.height_px);
    let media_id = MediaId::from_bytes(bytes);
    Ok([Color::BLACK, Color::WHITE].into_iter().any(|background| {
        let background_element = PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            color: background,
        };
        let baseline = LayoutResult::new(
            vec![PageFrame::new(1, width, height, vec![background_element.clone()]).into()],
            Vec::new(),
            None,
            Vec::new(),
        );
        let candidate = LayoutResult::new(
            vec![
                PageFrame::new(
                    1,
                    width,
                    height,
                    vec![
                        background_element,
                        PositionedElement::Image {
                            rect: Rect {
                                x: 0.0,
                                y: 0.0,
                                width,
                                height,
                            },
                            data: bytes.to_vec(),
                            content_type: content_type.to_owned(),
                            media_id,
                        },
                    ],
                )
                .into(),
            ],
            Vec::new(),
            None,
            Vec::new(),
        );
        oxoffice_pdf::render_page_to_png(&baseline, 0, 72.0)
            .zip(oxoffice_pdf::render_page_to_png(&candidate, 0, 72.0))
            .is_some_and(|(baseline, candidate)| baseline != candidate)
    }))
}

#[cfg(feature = "render")]
const MAX_RENDER_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
#[cfg(feature = "render")]
const MAX_RENDER_PREVIEW_DECODED_BYTES: usize = 64 * 1024 * 1024;

#[cfg(feature = "render")]
fn render_media_within_decode_bounds(
    bytes: &[u8],
    format: ImageFormat,
) -> std::result::Result<bool, &'static str> {
    if bytes.len() > MAX_RENDER_PREVIEW_BYTES {
        return Err("exceeds the 16 MiB encoded-image render limit");
    }
    let Some(info) = probe(bytes) else {
        return Ok(false);
    };
    let Some(pixel_bytes) = usize::try_from(info.width_px)
        .ok()
        .and_then(|width| width.checked_mul(info.height_px as usize))
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return Err("has dimensions that overflow the decoded-image size");
    };
    if pixel_bytes > MAX_RENDER_PREVIEW_DECODED_BYTES {
        return Err("exceeds the 64 MiB decoded-image render limit");
    }
    if format != ImageFormat::Png {
        return Ok(true);
    }
    let interlace = *bytes.get(28).ok_or("could not be decoded")?;
    if interlace > 1 {
        return Err("could not be decoded");
    }
    let expected = (|| {
        let width = usize::try_from(info.width_px).ok()?;
        let height = usize::try_from(info.height_px).ok()?;
        let channels = usize::from(info.channels);
        let passes: &[(usize, usize, usize, usize)] = match interlace {
            0 => &[(0, 0, 1, 1)],
            1 => &[
                (0, 0, 8, 8),
                (4, 0, 8, 8),
                (0, 4, 4, 8),
                (2, 0, 4, 4),
                (0, 2, 2, 4),
                (1, 0, 2, 2),
                (0, 1, 1, 2),
            ],
            _ => return None,
        };
        passes.iter().try_fold(0usize, |total, &(x, y, dx, dy)| {
            if width <= x || height <= y {
                return Some(total);
            }
            let pass_width = (width - x).div_ceil(dx);
            let pass_height = (height - y).div_ceil(dy);
            total.checked_add(
                pass_width
                    .checked_mul(channels)?
                    .checked_add(1)?
                    .checked_mul(pass_height)?,
            )
        })
    })();
    let Some(expected) = expected else {
        return Err("has dimensions that overflow the decoded-image size");
    };
    if expected > MAX_RENDER_PREVIEW_DECODED_BYTES {
        return Err("exceeds the 64 MiB decoded-image render limit");
    }
    let mut idat = Vec::new();
    let mut offset = 33usize;
    while offset < bytes.len() {
        let Some(length_bytes) = bytes.get(offset..offset.saturating_add(4)) else {
            return Ok(false);
        };
        let length = u32::from_be_bytes(length_bytes.try_into().unwrap()) as usize;
        let Some(kind_start) = offset.checked_add(4) else {
            return Ok(false);
        };
        let Some(payload_start) = kind_start.checked_add(4) else {
            return Ok(false);
        };
        let Some(payload_end) = payload_start.checked_add(length) else {
            return Ok(false);
        };
        let Some(chunk_end) = payload_end.checked_add(4) else {
            return Ok(false);
        };
        let (Some(kind), Some(payload), Some(_)) = (
            bytes.get(kind_start..payload_start),
            bytes.get(payload_start..payload_end),
            bytes.get(payload_end..chunk_end),
        ) else {
            return Ok(false);
        };
        if kind == b"IDAT" {
            if idat.len().saturating_add(payload.len()) > MAX_RENDER_PREVIEW_BYTES {
                return Err("exceeds the 16 MiB encoded-image render limit");
            }
            idat.extend_from_slice(payload);
        }
        if kind == b"IEND" {
            break;
        }
        offset = chunk_end;
    }
    Ok(!idat.is_empty()
        && miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, expected)
            .is_ok_and(|decoded| decoded.len() >= expected))
}

#[cfg(feature = "render")]
fn render_source_shape_has_bounds(context: &ResolveCtx<'_>, item: &FlattenedItem<'_>) -> bool {
    let FlattenedItem::Shape { child, .. } = item else {
        return false;
    };
    match child {
        ShapeTreeChild::Shape(shape) => context
            .effective_xfrm(shape)
            .as_ref()
            .is_some_and(render_transform_has_bounds),
        ShapeTreeChild::Picture(picture) => {
            !picture.hidden()
                && context
                    .effective_picture_xfrm(picture)
                    .as_ref()
                    .is_some_and(render_transform_has_bounds)
        }
        ShapeTreeChild::GraphicFrame(frame) => render_transform_has_bounds(&frame.transform),
        ShapeTreeChild::Connector(connector) => connector
            .shape_properties
            .transform
            .as_ref()
            .is_some_and(render_connector_transform_has_bounds),
        ShapeTreeChild::AlternateContent(alternate) => alternate
            .chart_choice()
            .is_some_and(|frame| render_transform_has_bounds(&frame.transform)),
        ShapeTreeChild::GroupShape(_) => false,
    }
}

#[cfg(feature = "render")]
fn render_source_smartart_clip(
    item: &FlattenedItem<'_>,
    slide_clips: &HashMap<u32, Rect>,
    layout_clips: &HashMap<u32, Rect>,
    master_clips: &HashMap<u32, Rect>,
) -> Result<Option<Rect>> {
    let FlattenedItem::Shape {
        source,
        child,
        group_scale,
        ..
    } = item
    else {
        return Ok(None);
    };
    let Some(id) = child.non_visual_id() else {
        return Ok(None);
    };
    let frame = match source {
        FlattenedSource::Slide => slide_clips.get(&id).copied(),
        FlattenedSource::Layout => layout_clips.get(&id).copied(),
        FlattenedSource::Master => master_clips.get(&id).copied(),
        FlattenedSource::Background => None,
    };
    let Some(frame) = frame else {
        return Ok(None);
    };
    let frame = Rect {
        x: frame.x * group_scale.0,
        y: frame.y * group_scale.1,
        width: frame.width * group_scale.0,
        height: frame.height * group_scale.1,
    };
    if ![
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        group_scale.0,
        group_scale.1,
    ]
    .into_iter()
    .all(f64::is_finite)
        || frame.width <= 0.0
        || frame.height <= 0.0
    {
        return Err(render_failure(
            "SmartArt parent group has invalid clip scale",
        ));
    }
    Ok(Some(frame))
}

#[cfg(feature = "render")]
fn smartart_local_clip(frame: Rect, shape: Rect) -> Result<Rect> {
    let clip = Rect {
        x: frame.x - shape.x,
        y: frame.y - shape.y,
        width: frame.width,
        height: frame.height,
    };
    if ![
        clip.x,
        clip.y,
        clip.width,
        clip.height,
        shape.x,
        shape.y,
        shape.width,
        shape.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || clip.width <= 0.0
        || clip.height <= 0.0
        || shape.width <= 0.0
        || shape.height <= 0.0
    {
        return Err(render_failure(
            "SmartArt clip has invalid resolved geometry",
        ));
    }
    Ok(clip)
}

#[cfg(feature = "render")]
fn apply_smartart_clips(
    page: &mut PageFrame,
    slide: &ResolvedSlide,
    clips: &[Option<Rect>],
) -> Result<()> {
    if clips.len() != slide.shapes.len() {
        return Err(render_failure(format!(
            "SmartArt clip count {}, resolved shape count {}",
            clips.len(),
            slide.shapes.len()
        )));
    }
    let shape_offset = page
        .elements
        .len()
        .checked_sub(slide.shapes.len())
        .ok_or_else(|| render_failure("rendered page dropped resolved shapes"))?;
    for ((element, shape), frame) in page.elements[shape_offset..]
        .iter_mut()
        .zip(&slide.shapes)
        .zip(clips)
    {
        let Some(frame) = frame else {
            continue;
        };
        let clip = LayoutPath::rect(smartart_local_clip(*frame, shape.bounds)?);
        let PositionedElement::Group(group) = element else {
            return Err(render_failure(
                "SmartArt resolved shape did not lower to a render group",
            ));
        };
        if group.clip.as_ref() == Some(&clip) {
            continue;
        }
        if let Some(existing) = group.clip.replace(clip) {
            group.children = vec![PositionedElement::Group(GroupElement {
                transform: Transform::IDENTITY,
                clip: Some(existing),
                opacity: 1.0,
                effects: Vec::new(),
                children: std::mem::take(&mut group.children),
            })];
        }
    }
    Ok(())
}

#[cfg(feature = "render")]
fn render_transform_has_bounds(transform: &CT_Transform2D) -> bool {
    transform
        .extent
        .is_some_and(|extent| extent.cx.0 > 0 && extent.cy.0 > 0)
}

#[cfg(feature = "render")]
fn render_connector_transform_has_bounds(transform: &CT_Transform2D) -> bool {
    transform.extent.is_some_and(|extent| {
        extent.cx.0 >= 0 && extent.cy.0 >= 0 && (extent.cx.0 > 0 || extent.cy.0 > 0)
    })
}

#[cfg(feature = "render")]
fn render_effective_color_map(
    master: &CT_SlideMaster,
    layout: &CT_SlideLayout,
    slide: &CT_Slide,
) -> ColorMap {
    match slide
        .color_map_override
        .as_ref()
        .or(layout.color_map_override.as_ref())
    {
        Some(override_value) => match &override_value.kind {
            ColorMapOverrideKind::Master => master.color_map.clone(),
            ColorMapOverrideKind::Override(map) => map.clone(),
        },
        None => master.color_map.clone(),
    }
}

#[cfg(feature = "render")]
fn render_related_part(
    package: &OpcPackage,
    source_part: &str,
    relationship_type: &str,
) -> Result<String> {
    let relationship = package
        .get_part_rels(source_part)
        .and_then(|relationships| relationships.get_by_type(relationship_type))
        .ok_or_else(|| {
            render_failure(format!(
                "{source_part}: missing relationship {relationship_type}"
            ))
        })?;
    if relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
    {
        return Err(Error::ExternalRelationship {
            source_part: source_part.to_owned(),
            relationship_id: relationship.id.clone(),
        });
    }
    Ok(OpcPackage::resolve_rel_target(
        source_part,
        &relationship.target,
    ))
}
