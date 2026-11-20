//! Text inheritance resolution.

use rpptx_oxml::placeholder::PhType;
use rpptx_oxml::shape_tree::CT_Shape;

use oxml_drawing::fill::{Fill, SolidFill};
use oxml_drawing::text::{
    CT_TextCharacterProperties, CT_TextListStyle, CT_TextParagraphProperties,
};

use crate::ResolveCtx;

/// Resolved paragraph and character properties without opaque XML or actions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectiveTextProperties {
    pub paragraph: CT_TextParagraphProperties,
    pub run: CT_TextCharacterProperties,
    /// Source language selected by the same inheritance chain as the run.
    pub language: Option<String>,
}

/// The independently resolved properties for all nine DrawingML list levels.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectiveListStyle {
    levels: [EffectiveTextProperties; 9],
}

impl EffectiveListStyle {
    /// Returns the zero-based DrawingML level selected by `a:pPr/@lvl`.
    pub fn level(&self, level: u8) -> Option<&EffectiveTextProperties> {
        self.levels.get(usize::from(level))
    }
}

impl ResolveCtx<'_> {
    /// Resolves sources one through six for every list level of one shape.
    pub fn effective_list_style(&self, shape: &CT_Shape) -> EffectiveListStyle {
        let key = shape
            .placeholder
            .as_ref()
            .map(|placeholder| placeholder.key());
        let mut effective = if let Some(cached) = self.list_style_cache.borrow().get(&key) {
            cached.clone()
        } else {
            let resolved = self.resolve_list_style_prefix(shape);
            self.list_style_cache
                .borrow_mut()
                .insert(key, resolved.clone());
            resolved
        };
        // The colour of the style's font reference overrides the inherited
        // text styles, and the shape's own list style overrides it in turn.
        // A colour that does not resolve leaves the inherited colour.
        if let Some(color) = shape
            .style()
            .and_then(|style| style.font_reference.color.as_ref())
            .filter(|color| self.concrete_color(color).is_ok())
        {
            let mut solid = SolidFill::default();
            solid.color = Some(color.clone());
            for level in &mut effective.levels {
                level.run.fill = Some(Fill::Solid(solid.clone()));
            }
        }
        if let Some(list_style) = shape.text_body.as_ref().and_then(|body| body.list_style()) {
            merge_list_style(&mut effective, list_style);
        }
        effective
    }

    /// Applies paragraph and run properties to the selected zero-based level.
    pub fn effective_text_properties(
        &self,
        shape: &CT_Shape,
        paragraph: Option<&CT_TextParagraphProperties>,
        run: Option<&CT_TextCharacterProperties>,
    ) -> EffectiveTextProperties {
        let style = self.effective_list_style(shape);
        let level = paragraph
            .and_then(|properties| properties.level)
            .unwrap_or(0);
        let mut effective = style
            .level(level)
            .cloned()
            .unwrap_or_else(EffectiveTextProperties::default);
        if let Some(properties) = paragraph {
            merge_paragraph(&mut effective, properties);
        }
        if let Some(properties) = run {
            merge_character(&mut effective, properties);
        }
        effective
    }

    /// Resolves presentation defaults and the master's other style for table text.
    pub(crate) fn effective_table_text_properties(
        &self,
        table_style: Option<&CT_TextCharacterProperties>,
        paragraph: Option<&CT_TextParagraphProperties>,
        run: Option<&CT_TextCharacterProperties>,
    ) -> EffectiveTextProperties {
        let mut style = EffectiveListStyle::default();
        merge_list_style(&mut style, self.default_text_style);
        if let Some(styles) = &self.master.text_styles {
            merge_list_style(&mut style, &styles.other_style);
        }
        let level = paragraph
            .and_then(|properties| properties.level)
            .unwrap_or(0);
        let mut effective = style
            .level(level)
            .cloned()
            .unwrap_or_else(EffectiveTextProperties::default);
        if let Some(properties) = table_style {
            merge_character(&mut effective, properties);
        }
        if let Some(properties) = paragraph {
            merge_paragraph(&mut effective, properties);
        }
        if let Some(properties) = run {
            merge_character(&mut effective, properties);
        }
        effective
    }

    fn resolve_list_style_prefix(&self, shape: &CT_Shape) -> EffectiveListStyle {
        let mut effective = EffectiveListStyle::default();
        merge_list_style(&mut effective, self.default_text_style);

        if let Some(styles) = &self.master.text_styles {
            let master_style = match shape
                .placeholder
                .as_ref()
                .map(|placeholder| placeholder.effective_type())
            {
                Some(PhType::Title | PhType::CenteredTitle) => &styles.title_style,
                Some(PhType::Body | PhType::Subtitle | PhType::Object) => &styles.body_style,
                _ => &styles.other_style,
            };
            merge_list_style(&mut effective, master_style);
        }

        let (layout_placeholder, master_placeholder) = self.placeholder_chain(shape);
        if let Some(list_style) = master_placeholder
            .and_then(|placeholder| placeholder.text_body.as_ref())
            .and_then(|body| body.list_style())
        {
            merge_list_style(&mut effective, list_style);
        }
        if let Some(list_style) = layout_placeholder
            .and_then(|placeholder| placeholder.text_body.as_ref())
            .and_then(|body| body.list_style())
        {
            merge_list_style(&mut effective, list_style);
        }
        effective
    }
}

fn merge_list_style(effective: &mut EffectiveListStyle, source: &CT_TextListStyle) {
    for level in 0..9 {
        if let Some(properties) = &source.default_paragraph_properties {
            merge_paragraph(&mut effective.levels[level], properties);
        }
        if let Some(properties) = source.level(level + 1) {
            merge_paragraph(&mut effective.levels[level], properties);
        }
    }
}

fn merge_paragraph(effective: &mut EffectiveTextProperties, source: &CT_TextParagraphProperties) {
    if source.left_margin.is_some() {
        effective.paragraph.left_margin = source.left_margin;
    }
    if source.right_margin.is_some() {
        effective.paragraph.right_margin = source.right_margin;
    }
    if source.level.is_some() {
        effective.paragraph.level = source.level;
    }
    if source.indent.is_some() {
        effective.paragraph.indent = source.indent;
    }
    if source.alignment.is_some() {
        effective.paragraph.alignment = source.alignment;
    }
    if source.right_to_left.is_some() {
        effective.paragraph.right_to_left = source.right_to_left;
    }
    if source.line_spacing.is_some() {
        effective.paragraph.line_spacing = source.line_spacing.clone();
    }
    if source.space_before.is_some() {
        effective.paragraph.space_before = source.space_before.clone();
    }
    if source.space_after.is_some() {
        effective.paragraph.space_after = source.space_after.clone();
    }
    if let Some(source_bullet) = &source.bullet {
        let bullet = effective.paragraph.bullet.get_or_insert_default();
        if source_bullet.color.is_some() {
            bullet.color = source_bullet.color.clone();
        }
        if source_bullet.size.is_some() {
            bullet.size = source_bullet.size.clone();
        }
        if source_bullet.font.is_some() {
            bullet.font = source_bullet.font.clone();
        }
        if source_bullet.choice.is_some() {
            bullet.choice = source_bullet.choice.clone();
        }
    }
    if let Some(properties) = &source.default_run_properties {
        merge_character(effective, properties);
    }
}

fn merge_character(effective: &mut EffectiveTextProperties, source: &CT_TextCharacterProperties) {
    if let Some(language) = source.language() {
        effective.language = Some(language.to_owned());
    }
    merge_character_properties(&mut effective.run, source);
}

fn merge_character_properties(
    effective: &mut CT_TextCharacterProperties,
    source: &CT_TextCharacterProperties,
) {
    if source.font_size.is_some() {
        effective.font_size = source.font_size;
    }
    if source.bold.is_some() {
        effective.bold = source.bold;
    }
    if source.italic.is_some() {
        effective.italic = source.italic;
    }
    if source.all_caps == Some(false) && source.small_caps == Some(false) {
        // cap="none" clears both modes as one owned value.
        effective.all_caps = Some(false);
        effective.small_caps = Some(false);
    } else {
        if source.all_caps.is_some() {
            effective.set_all_caps(source.all_caps);
        }
        if source.small_caps.is_some() {
            effective.set_small_caps(source.small_caps);
        }
    }
    if source.highlight.is_some() {
        effective.highlight = source.highlight.clone();
    }
    if source.underline.is_some() {
        effective.underline = source.underline;
    }
    if source.strike.is_some() {
        effective.strike = source.strike;
    }
    if source.spacing.is_some() {
        effective.spacing = source.spacing.clone();
    }
    if source.baseline.is_some() {
        effective.baseline = source.baseline.clone();
    }
    if source.fill.is_some() {
        effective.fill = source.fill.clone();
    }
    if source.latin.is_some() {
        effective.latin = source.latin.clone();
    }
    if source.east_asian.is_some() {
        effective.east_asian = source.east_asian.clone();
    }
    if source.complex_script.is_some() {
        effective.complex_script = source.complex_script.clone();
    }
    if source.symbol.is_some() {
        effective.symbol = source.symbol.clone();
    }
}
