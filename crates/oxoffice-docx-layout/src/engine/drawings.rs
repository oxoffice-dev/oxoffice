//! Charts and anchored drawings.

use super::*;

/// Whether any drawing or floating table in the document body wraps text
/// around itself.
///
/// A document without one can never reach the reflow path, so it does not pay
/// for it, and it also never needs the second pagination pass.
pub(super) fn document_has_wrapping_drawing(input: &LayoutInput) -> bool {
    /// Whether a `w:tblpPr` on the table itself floats it.
    ///
    /// The rule belongs to the lowering, so this asks it rather than repeating
    /// it. Direct properties only: a float declared by a table style is as rare
    /// as a drawing inside a nested table, and the conservative answer there is
    /// the same, look no deeper and lose the reflow rather than the table.
    fn table_floats(table: &oxoffice_docx_oxml::table::CT_Tbl) -> bool {
        table
            .properties
            .as_ref()
            .is_some_and(|properties| crate::table::floating_table(properties).is_some())
    }

    fn paragraph_wraps(para: &CT_P, view: RevisionView) -> bool {
        fn run_wraps(run: &CT_R) -> bool {
            run.content
                .iter()
                .filter_map(|rc| match rc {
                    RunContent::Drawing(d) => Some(d),
                    _ => None,
                })
                .chain(run.alt_drawings.iter())
                .any(|drawing| {
                    drawing
                        .anchor
                        .as_ref()
                        .is_some_and(|anchor| anchor.wrap != WrapType::None)
                })
        }

        if para.revisions.is_empty() && para.content_controls.is_empty() {
            para.runs.iter().any(run_wraps)
        } else {
            project_paragraph_runs(para, view)
                .iter()
                .any(|projected| run_wraps(projected.run))
        }
    }

    input
        .document
        .body
        .content
        .iter()
        .any(|content| match content {
            BodyContent::Paragraph(para) => paragraph_wraps(para, input.revision_view),
            BodyContent::Table(table) => {
                table_floats(table)
                    || table
                        .rows
                        .iter()
                        .flat_map(|row| row.cells.iter())
                        .flat_map(|cell| cell.content.iter())
                        .any(|content| match content {
                            oxoffice_docx_oxml::table::CellContent::Paragraph(para) => {
                                paragraph_wraps(para, input.revision_view)
                            }
                            // A drawing inside a nested table is rare enough that
                            // the conservative answer is to look no deeper.
                            oxoffice_docx_oxml::table::CellContent::Table(_) => false,
                            oxoffice_docx_oxml::table::CellContent::ContentControl(_) => false,
                        })
            }
            _ => false,
        })
}

pub(super) fn render_word_chart(
    relationship_id: &str,
    width: f64,
    height: f64,
    input: &LayoutInput,
    fm: &mut FontManager,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<GroupElement> {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    let rendered = match input.charts.get(relationship_id) {
        Some(Ok(chart)) => oxoffice_chart::render_chart(
            &chart.chart,
            bounds,
            &input.chart_theme,
            &input.chart_color_map,
            fm,
        )
        .map_err(|error| error.to_string()),
        Some(Err(message)) => Err(message.clone()),
        None => Err("relationship was not resolved from the document part".to_owned()),
    };
    match rendered {
        Ok(group) => {
            if let Some(Ok(chart)) = input.charts.get(relationship_id) {
                for code in oxoffice_chart::unsupported_number_formats(&chart.chart)
                    .map_err(|error| LayoutError::Layout(error.to_string()))?
                {
                    push_unique_diagnostic(
                        diagnostics,
                        format!(
                            "Word chart relationship {relationship_id}: number format `{code}` is not supported and its labels use General"
                        ),
                    );
                }
            }
            Ok(group)
        }
        Err(detail) => {
            diagnostics.push(Diagnostic {
                message: format!("Word chart relationship {relationship_id}: {detail}"),
            });
            oxoffice_chart::render_chart_placeholder(bounds, fm)
                .map_err(|error| oxoffice_layout::LayoutError::Layout(error.to_string()))
        }
    }
}

/// Collect the floating drawings anchored to a paragraph.
///
/// The offsets stay paired with the frame they are measured from. Resolving
/// them here is not possible: a paragraph-relative offset needs the laid-out
/// position of the paragraph, which only the paginator knows.
///
/// A shape's text box is laid out here rather than later, because breaking it
/// into lines needs the font manager.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_anchored_drawings(
    para: &CT_P,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
) -> Result<(Vec<block::AnchoredDrawing>, Vec<WordAnchorPaint>)> {
    let mut out = Vec::new();
    let mut paints = Vec::new();

    // Drawings written plainly, and drawings recovered from an
    // mc:AlternateContent block, are both anchored the same way.
    for (run_index, projected) in project_paragraph_runs(para, input.revision_view)
        .into_iter()
        .enumerate()
    {
        let run = projected.run;
        let plain = run.content.iter().filter_map(|rc| match rc {
            RunContent::Drawing(d) => Some(d),
            _ => None,
        });
        for (drawing_index, drawing) in plain.chain(run.alt_drawings.iter()).enumerate() {
            let Some(anchor) = drawing.anchor.as_ref() else {
                continue;
            };

            // A picture also carries a pic:spPr, so a parsed shape alone does
            // not mean this is a shape. An embed id is what makes it a
            // picture, and that takes precedence.
            let shape = if anchor.embed_id.is_empty() && anchor.chart_rel_id.is_none() {
                anchor.shape.as_ref()
            } else {
                None
            };

            let mut content_paint = WordAnchorPaint::Image;
            let content = if let Some(relationship_id) = anchor.chart_rel_id.as_deref() {
                content_paint = WordAnchorPaint::Group {
                    policies: Default::default(),
                };
                block::AnchoredContent::Group(render_word_chart(
                    relationship_id,
                    anchor.extent_cx.to_pt(),
                    anchor.extent_cy.to_pt(),
                    input,
                    fm,
                    diagnostics,
                )?)
            } else {
                match shape {
                    Some(shape) => {
                        // A shape's text box wraps at the shape width.
                        let mut text = Vec::new();
                        let mut paragraph_paints = Vec::new();
                        if let Some(body) = &shape.text_body {
                            let mut local_sources = source_node
                                .map(|_| SourceRegistry::for_body(input, body))
                                .transpose()?;
                            let mut inverse = None;
                            if input.sequence_snapshot.is_some()
                                && let Some(parent) = source_node
                                && let Some(sources) = &mut local_sources
                            {
                                match sources.bind_sequence_shape_sources(
                                    input,
                                    parent,
                                    run_index,
                                    drawing_index,
                                    shape.source_text_box_owner,
                                ) {
                                    Ok(map) => inverse = Some(map),
                                    Err(message) => {
                                        diagnostics.push(Diagnostic { message });
                                        local_sources = None;
                                    }
                                }
                            }
                            for item in body_layout_items(body) {
                                match item {
                                    MainStoryLayoutItem::Paragraph(paragraph, path) => {
                                        let node = local_sources.as_ref().and_then(|sources| {
                                            sources.id(&WordStory::Document, &path)
                                        });
                                        let (mut block, _, mut paint) =
                                            layout_paragraph_with_source_and_direction(
                                                paragraph,
                                                anchor.extent_cx.to_pt(),
                                                styles,
                                                input,
                                                media,
                                                fm,
                                                num_state,
                                                diagnostics,
                                                node,
                                                node,
                                                None,
                                                None,
                                            )?;
                                        if let Some(sources) = &local_sources {
                                            sources
                                                .bind_text_boxes(&mut block, &mut paint, node)?;
                                        }
                                        text.push(block);
                                        paragraph_paints.push(paint);
                                    }
                                    MainStoryLayoutItem::Table(table, path) => {
                                        let (table, semantics) =
                                            table::layout_table_with_provenance(
                                                table,
                                                anchor.extent_cx.to_pt(),
                                                styles,
                                                input,
                                                media,
                                                fm,
                                                num_state,
                                                diagnostics,
                                                local_sources.as_ref(),
                                                &WordStory::Document,
                                                &path,
                                                None,
                                            )?;
                                        let geometry = PageGeometry {
                                            page_width: anchor.extent_cx.to_pt(),
                                            page_height: anchor.extent_cy.to_pt(),
                                            ..PageGeometry::default()
                                        };
                                        for (block, paint) in paginator::story_table_rows(
                                            &table,
                                            &semantics,
                                            &geometry,
                                            media.media(),
                                        )? {
                                            text.push(block);
                                            paragraph_paints.push(paint);
                                        }
                                    }
                                }
                            }
                            if let Some(inverse) = &inverse {
                                for (paragraph, paint) in text.iter_mut().zip(&mut paragraph_paints)
                                {
                                    remap_text_box_sources(paragraph, inverse)?;
                                    paint.remap_sources(inverse)?;
                                }
                            }
                        } else {
                            for p in &shape.text {
                                let (block, _, paint) = layout_paragraph_with_source_and_direction(
                                    p,
                                    anchor.extent_cx.to_pt(),
                                    styles,
                                    input,
                                    media,
                                    fm,
                                    num_state,
                                    diagnostics,
                                    source_node.map(|_| CACHE_SOURCE_NODE),
                                    source_node.map(|_| CACHE_SOURCE_NODE),
                                    None,
                                    None,
                                )?;
                                text.push(block);
                                paragraph_paints.push(paint);
                            }
                        }
                        content_paint = WordAnchorPaint::Shape {
                            paragraphs: paragraph_paints,
                        };
                        block::AnchoredContent::Shape {
                            preset: block::ShapePreset::from_prst(shape.preset.as_deref()),
                            fill: shape.solid_fill.as_deref().map(Color::from_hex),
                            text,
                        }
                    }
                    None if anchor.embed_id.is_empty() => continue,
                    None => {
                        media.record_picture_draw_owner(&anchor.embed_id);
                        let media_id = media
                            .id_for_relationship_with_diagnostic(&anchor.embed_id, diagnostics);
                        if let Some((opacity, rotation)) = anchor.picture_effects() {
                            let width = anchor.extent_cx.to_pt();
                            let height = anchor.extent_cy.to_pt();
                            let image = media.media().get(&media_id);
                            content_paint = WordAnchorPaint::Group {
                                policies: Default::default(),
                            };
                            block::AnchoredContent::Group(GroupElement {
                                transform: Transform::rotate_about(
                                    rotation,
                                    width / 2.0,
                                    height / 2.0,
                                ),
                                clip: None,
                                opacity,
                                effects: Vec::new(),
                                children: vec![PositionedElement::Image {
                                    rect: Rect {
                                        x: 0.0,
                                        y: 0.0,
                                        width,
                                        height,
                                    },
                                    data: image.map_or_else(Vec::new, |image| image.data.clone()),
                                    content_type: image.map_or_else(String::new, |image| {
                                        image.content_type.clone()
                                    }),
                                    media_id,
                                }],
                            })
                        } else {
                            block::AnchoredContent::Image { media_id }
                        }
                    }
                }
            };

            paints.push(content_paint);
            out.push(block::AnchoredDrawing {
                rotation_degrees: if shape.is_some() {
                    anchor.shape_rotation_degrees().unwrap_or(0.0)
                } else {
                    0.0
                },
                behind_doc: anchor.behind_doc,
                rel_h: anchor.pos_h_relative_from,
                off_h: anchor.pos_h_offset.to_pt(),
                rel_v: anchor.pos_v_relative_from,
                off_v: anchor.pos_v_offset.to_pt(),
                width: anchor.extent_cx.to_pt(),
                height: anchor.extent_cy.to_pt(),
                alternate_text: anchor.description.clone(),
                structure_id: None,
                wrap: anchor.wrap,
                dist_top: anchor.dist_t.to_pt(),
                dist_bottom: anchor.dist_b.to_pt(),
                dist_left: anchor.dist_l.to_pt(),
                dist_right: anchor.dist_r.to_pt(),
                align_h: anchor.pos_h_align,
                align_v: anchor.pos_v_align,
                content,
            });
        }
    }
    Ok((out, paints))
}
