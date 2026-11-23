//! Drawings placed in line with text: pictures, charts and text boxes.

use super::*;

/// Push the inline item of a drawing placed in line with text: a chart, a
/// text box with its text, or a picture.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_inline_drawing(
    inline: &oxoffice_docx_oxml::drawing::CT_Inline,
    form_mark: Option<oxoffice_layout::FormMark>,
    inline_items: &mut Vec<InlineItem>,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let width = inline.extent_cx.to_pt();
    let height = inline.extent_cy.to_pt();
    // A picture also carries a `pic:spPr`, so a parsed shape alone does not
    // make a text box: an embed id makes a picture, and that wins.
    let shape = inline
        .shape
        .as_ref()
        .filter(|_| inline.embed_id.is_empty() && inline.chart_rel_id.is_none());
    let item = if let Some(relationship_id) = inline.chart_rel_id.as_deref() {
        InlineItem::Group {
            width,
            height,
            baseline: None,
            math: false,
            group: render_word_chart(relationship_id, width, height, input, fm, diagnostics)?,
        }
    } else if let Some(shape) = shape {
        InlineItem::Group {
            width,
            height,
            baseline: None,
            math: false,
            group: inline_shape_group(
                shape,
                (width, height),
                styles,
                input,
                media,
                fm,
                num_state,
                diagnostics,
            )?,
        }
    } else {
        media.record_picture_draw_owner(&inline.embed_id);
        InlineItem::Image {
            form_mark,
            width,
            height,
            media_id: media.id_for_relationship_with_diagnostic(&inline.embed_id, diagnostics),
        }
    };
    if let Some(alternate_text) = inline
        .description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        inline_items.push(InlineItem::Figure {
            item: Box::new(item),
            alternate_text: alternate_text.to_owned(),
            structure_id: None,
        });
    } else {
        inline_items.push(item);
    }
    Ok(())
}

/// A text box placed in line with text, `width` by `height`: its fill, and
/// its paragraphs and tables laid out at its width less its insets, drawn as
/// a floating text box draws them.
#[allow(clippy::too_many_arguments)]
fn inline_shape_group(
    shape: &oxoffice_docx_oxml::drawing::CT_Shape,
    (width, height): (f64, f64),
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<GroupElement> {
    let insets = shape.text_insets;
    let text_width = (width - insets.left.to_pt() - insets.right.to_pt()).max(0.0);
    let text_height = (height - insets.top.to_pt() - insets.bottom.to_pt()).max(0.0);
    let mut text = Vec::new();
    if let Some(body) = &shape.text_body {
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, _) => {
                    let (block, _, _) = layout_paragraph_with_source_and_direction(
                        paragraph,
                        text_width,
                        styles,
                        input,
                        media,
                        fm,
                        num_state,
                        diagnostics,
                        None,
                        None,
                        None,
                        None,
                    )?;
                    text.push(block);
                }
                MainStoryLayoutItem::Table(table, path) => {
                    let (table, semantics) = table::layout_table_with_provenance(
                        table,
                        text_width,
                        styles,
                        input,
                        media,
                        fm,
                        num_state,
                        diagnostics,
                        None,
                        &WordStory::Document,
                        &path,
                        None,
                    )?;
                    let geometry = PageGeometry {
                        page_width: text_width,
                        page_height: text_height,
                        ..PageGeometry::default()
                    };
                    for (block, _) in
                        paginator::story_table_rows(&table, &semantics, &geometry, media.media())?
                    {
                        text.push(block);
                    }
                }
            }
        }
    } else {
        for paragraph in &shape.text {
            let (block, _, _) = layout_paragraph_with_source_and_direction(
                paragraph,
                text_width,
                styles,
                input,
                media,
                fm,
                num_state,
                diagnostics,
                None,
                None,
                None,
                None,
            )?;
            text.push(block);
        }
    }
    let geometry = PageGeometry {
        do_not_use_html_paragraph_auto_spacing: input.do_not_use_html_paragraph_auto_spacing,
        ..PageGeometry::default()
    };
    let children = paginator::inline_shape_elements(
        block::ShapePreset::from_prst(shape.preset.as_deref()),
        shape.solid_fill.as_deref().map(Color::from_hex),
        &text,
        (width, height),
        [
            insets.left.to_pt(),
            insets.top.to_pt(),
            insets.right.to_pt(),
            insets.bottom.to_pt(),
        ],
        // Vertical text is still laid out across the box, so its lines are
        // not measured against the box's height.
        !shape.grows_to_fit_text && !shape.vertical_text,
        &geometry,
        media.media(),
    )?;
    Ok(GroupElement {
        transform: Transform::IDENTITY,
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children,
    })
}
