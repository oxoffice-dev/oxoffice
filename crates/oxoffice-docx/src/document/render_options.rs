//! Options of a render: page images, PDF and the fonts it may use.

/// Options that select a native document render projection.
///
/// Built from [`Default`] and set field by field, so that new options break
/// no caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RenderOptions {
    /// The tracked-revision view to render.
    pub revision_view: oxoffice_docx_layout::RevisionView,
    /// How the tracked view marks revisions.
    pub revision_style: oxoffice_docx_layout::RevisionStyle,
    /// Resolve font families in the system fonts first, the bundled fonts
    /// as fallback.
    ///
    /// By default (`false`, with `bundled_fonts_only` also `false`) families
    /// resolve in the bundled fonts, and a system font only draws the
    /// characters they cannot, such as most Chinese, Japanese and Korean
    /// text. A build without the `system-fonts` feature refuses `true`.
    /// Deterministic helpers ignore it. Exclusive with `bundled_fonts_only`.
    pub use_system_fonts: bool,
    /// Use the bundled fonts only, never a system font: the render is the
    /// same on every machine, and characters the bundled fonts lack are drawn
    /// as empty boxes, which a diagnostic names. The deterministic helpers
    /// always render so. Exclusive with `use_system_fonts`.
    pub bundled_fonts_only: bool,
    /// Export the content controls and legacy form fields of the main body
    /// with its text boxes, and the notes as fillable PDF form fields. A control tagged `signature` or `signature:<name>` becomes an
    /// empty signature field. Image rendering draws the controls as usual,
    /// and the tracked view refuses this option.
    pub fillable_forms: bool,
}

impl RenderOptions {
    /// These options with `revision_view` set.
    pub const fn with_revision_view(mut self, view: oxoffice_docx_layout::RevisionView) -> Self {
        self.revision_view = view;
        self
    }

    /// These options with `revision_style` set.
    pub const fn with_revision_style(mut self, style: oxoffice_docx_layout::RevisionStyle) -> Self {
        self.revision_style = style;
        self
    }

    /// These options with `use_system_fonts` set.
    pub const fn with_system_fonts(mut self, use_system_fonts: bool) -> Self {
        self.use_system_fonts = use_system_fonts;
        self
    }

    /// These options with `bundled_fonts_only` set.
    pub const fn with_bundled_fonts_only(mut self, bundled_fonts_only: bool) -> Self {
        self.bundled_fonts_only = bundled_fonts_only;
        self
    }

    /// These options with `fillable_forms` set.
    pub const fn with_fillable_forms(mut self, fillable_forms: bool) -> Self {
        self.fillable_forms = fillable_forms;
        self
    }
}

/// Why the tracked view refuses fillable forms: a field value would mix
/// inserted and deleted text.
pub(super) const FILLABLE_FORMS_NEED_ACCEPTED_VIEW: &str =
    "fillable forms need the accepted revision view, not the tracked one";

/// The refusal for an explicit system-font request on a feature-off build.
pub(super) const SYSTEM_FONTS_UNAVAILABLE: &str =
    "system fonts are not available in this build (the system-fonts feature is disabled)";

/// The refusal for both font switches at once.
pub(super) const FONT_SWITCHES_EXCLUSIVE: &str =
    "system fonts first and bundled fonts only exclude each other";

/// Which font tier an implicit layout publisher uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FontPolicy {
    /// Bundled fonts only (`bundled_fonts_only`).
    BundledOnly,
    /// The default: bundled fonts first, a system font for what they lack.
    BundledWithSystemFallback,
    /// System fonts first (`use_system_fonts`).
    SystemWithBundledFallback,
}

pub(super) fn font_policy(options: RenderOptions) -> FontPolicy {
    match (options.use_system_fonts, options.bundled_fonts_only) {
        (true, _) => FontPolicy::SystemWithBundledFallback,
        (false, true) => FontPolicy::BundledOnly,
        (false, false) => FontPolicy::BundledWithSystemFallback,
    }
}
