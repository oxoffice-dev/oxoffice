//! Header, footer and watermark layout.

use super::*;

/// Lay out header and footer content (both Default and First-page).
pub(super) fn layout_header_footer(
    engine: &mut Engine,
    sect_pr: &CT_SectPr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    reference_state: &mut NumberingState,
) -> Result<Option<(HeaderFooterContent, HeaderFooterSemantics)>> {
    let mut has_content = false;
    let mut header_blocks = Vec::new();
    let mut footer_blocks = Vec::new();
    let mut first_header_blocks = Vec::new();
    let mut first_footer_blocks = Vec::new();
    let mut even_header_blocks = Vec::new();
    let mut even_footer_blocks = Vec::new();
    let mut header_directions = Vec::new();
    let mut footer_directions = Vec::new();
    let mut first_header_directions = Vec::new();
    let mut first_footer_directions = Vec::new();
    let mut even_header_directions = Vec::new();
    let mut even_footer_directions = Vec::new();
    let mut watermark = None;
    let mut first_watermark = None;
    let mut even_watermark = None;
    let even_headers_active = sect_pr
        .header_refs
        .iter()
        .any(|reference| reference.hdr_ftr_type == HdrFtrType::Even);

    let geometry = sect_pr_to_geometry(sect_pr);
    // Headers and footers span the text measure whatever the body's columns do.
    let width = geometry.text_measure();

    for href in &sect_pr.header_refs {
        let (target_blocks, target_directions, target_watermark) = match href.hdr_ftr_type {
            HdrFtrType::Default => (&mut header_blocks, &mut header_directions, &mut watermark),
            HdrFtrType::First => (
                &mut first_header_blocks,
                &mut first_header_directions,
                &mut first_watermark,
            ),
            HdrFtrType::Even => (
                &mut even_header_blocks,
                &mut even_header_directions,
                &mut even_watermark,
            ),
        };
        if let Some(hdr) = input.headers.get(&href.rel_id) {
            let mut story_num_state = reference_state.references_only();
            let content = layout_header_footer_variant(
                engine,
                HeaderFooterStoryKind::Header,
                href.hdr_ftr_type,
                sect_pr,
                &href.rel_id,
                hdr,
                input,
                styles,
                media,
                &mut story_num_state,
                diagnostics,
                sources,
                width,
                geometry.clone(),
            )?;
            reference_state.merge_references(&story_num_state);
            engine
                .numbering_by_source
                .extend(story_num_state.take_resolved());
            target_blocks.extend(content.blocks);
            target_directions.extend(content.directions);
            if target_watermark.is_none() {
                *target_watermark = content.watermark;
            }
            has_content = true;
        }
    }

    for fref in &sect_pr.footer_refs {
        let (target_blocks, target_directions) = match fref.hdr_ftr_type {
            HdrFtrType::Default => (&mut footer_blocks, &mut footer_directions),
            HdrFtrType::First => (&mut first_footer_blocks, &mut first_footer_directions),
            HdrFtrType::Even => (&mut even_footer_blocks, &mut even_footer_directions),
        };
        if let Some(ftr) = input.footers.get(&fref.rel_id) {
            let mut story_num_state = reference_state.references_only();
            let content = layout_header_footer_variant(
                engine,
                HeaderFooterStoryKind::Footer,
                fref.hdr_ftr_type,
                sect_pr,
                &fref.rel_id,
                ftr,
                input,
                styles,
                media,
                &mut story_num_state,
                diagnostics,
                sources,
                width,
                geometry.clone(),
            )?;
            reference_state.merge_references(&story_num_state);
            engine
                .numbering_by_source
                .extend(story_num_state.take_resolved());
            target_blocks.extend(content.blocks);
            target_directions.extend(content.directions);
            has_content = true;
        }
    }

    if has_content {
        Ok(Some((
            HeaderFooterContent {
                header_blocks,
                footer_blocks,
                first_header_blocks,
                first_footer_blocks,
                even_header_blocks,
                even_footer_blocks,
                even_headers_active,
                watermark,
                first_watermark,
                even_watermark,
            },
            HeaderFooterSemantics {
                header_directions,
                footer_directions,
                first_header_directions,
                first_footer_directions,
                even_header_directions,
                even_footer_directions,
            },
        )))
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_header_footer_variant(
    engine: &mut Engine,
    story_kind: HeaderFooterStoryKind,
    variant: HdrFtrType,
    sect_pr: &CT_SectPr,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    width: f64,
    geometry: PageGeometry,
) -> Result<HeaderFooterVariantContent> {
    let rich_story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    let rich_body = input.story_bodies.get(&rich_story);
    let cache_safe = rich_body.is_none_or(|body| {
        body.content.iter().all(|content| match content {
            BodyContent::Paragraph(paragraph) => paragraph_is_cache_safe(paragraph, styles),
            _ => false,
        })
    }) && header_footer_section_is_cache_safe(sect_pr)
        && header_footer_part_is_cache_safe(part, styles);
    let resolved_part_bytes = match story_kind {
        HeaderFooterStoryKind::Header => part.to_xml_header(),
        HeaderFooterStoryKind::Footer => part.to_xml_footer(),
    };
    if !cache_safe || resolved_part_bytes.is_err() {
        return layout_header_footer_variant_uncached(
            story_kind,
            relationship_id,
            part,
            input,
            styles,
            media,
            &mut engine.font_manager,
            num_state,
            diagnostics,
            sources,
            false,
            width,
            geometry,
        );
    }

    let key = HeaderFooterCacheKey {
        story: story_kind,
        variant,
        section: sect_pr.clone(),
        relationship_id: relationship_id.to_owned(),
        part: part.clone(),
        resolved_part_bytes: resolved_part_bytes.expect("checked resolved part bytes"),
        with_provenance: sources.is_some(),
        rich_body: rich_body.cloned(),
        source_part_name: input.story_part_names.get(&rich_story).cloned(),
    };
    let hit = engine
        .header_footer_cache_reads_enabled
        .then(|| {
            engine
                .header_footer_cache
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| {
                    (
                        entry.content.clone(),
                        entry.diagnostics.clone(),
                        entry.picture_draw_owners.clone(),
                        entry.font_trace.clone(),
                    )
                })
        })
        .flatten();
    if let Some((mut content, cached_diagnostics, picture_draw_owners, font_trace)) = hit {
        rebind_header_footer_sources(
            story_kind,
            relationship_id,
            part,
            &mut content.blocks,
            &mut content.directions,
            sources,
        )?;
        diagnostics.extend(cached_diagnostics);
        media.replay_picture_draw_owners(&picture_draw_owners);
        engine.font_manager.replay_font_trace_events(&font_trace)?;
        return Ok(content);
    }

    let diagnostics_start = diagnostics.len();
    let owners_start = media.picture_draw_owner_checkpoint();
    engine.font_manager.begin_paragraph_font_trace();
    let content_result = layout_header_footer_variant_uncached(
        story_kind,
        relationship_id,
        part,
        input,
        styles,
        media,
        &mut engine.font_manager,
        num_state,
        diagnostics,
        None,
        true,
        width,
        geometry,
    );
    let font_trace = engine.font_manager.finish_paragraph_font_events();
    let mut content = content_result?;
    let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
    let picture_draw_owners = media.picture_draw_owners_since(owners_start);
    if let Some(font_trace) = font_trace {
        let bytes =
            header_footer_cache_entry_bytes(&key, &content, &cached_diagnostics, &font_trace);
        let bytes = bytes.saturating_add(picture_draw_owner_bytes(&picture_draw_owners));
        engine.stage_header_footer_cache_entry(HeaderFooterCacheEntry {
            key,
            content: content.clone(),
            diagnostics: cached_diagnostics,
            picture_draw_owners,
            font_trace,
            bytes,
        });
    }
    rebind_header_footer_sources(
        story_kind,
        relationship_id,
        part,
        &mut content.blocks,
        &mut content.directions,
        sources,
    )?;
    Ok(content)
}

#[allow(clippy::too_many_arguments)]
fn layout_header_footer_variant_uncached(
    story_kind: HeaderFooterStoryKind,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    cache_source: bool,
    width: f64,
    geometry: PageGeometry,
) -> Result<HeaderFooterVariantContent> {
    let watermark = if story_kind == HeaderFooterStoryKind::Header {
        match part.watermarks().first() {
            Some(projected) => layout_watermark(
                projected,
                relationship_id,
                input,
                media,
                fm,
                geometry.clone(),
                diagnostics,
            )?,
            None => None,
        }
    } else {
        None
    };
    let story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    let part_media = media.scoped_to_part(relationship_id);
    let mut blocks = Vec::with_capacity(part.paragraphs.len());
    let mut directions = Vec::with_capacity(part.paragraphs.len());
    let items = input
        .story_bodies
        .get(&story)
        .map(body_layout_items)
        .unwrap_or_else(|| {
            part.paragraphs
                .iter()
                .enumerate()
                .map(|(index, paragraph)| MainStoryLayoutItem::Paragraph(paragraph, vec![index]))
                .collect()
        });
    for item in items {
        match item {
            MainStoryLayoutItem::Paragraph(paragraph, path) => {
                let source = if cache_source {
                    Some(CACHE_SOURCE_NODE)
                } else {
                    sources.and_then(|sources| sources.id(&story, &path))
                };
                let mut properties = style_resolver::resolve_paragraph_properties(
                    paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.style_id.as_deref()),
                    styles,
                );
                if let Some(direct) = &paragraph.properties {
                    properties.merge_from(direct);
                }
                let frame = properties
                    .frame
                    .filter(|frame| frame.drop_cap.as_deref().is_none_or(|kind| kind == "none"));
                let measure = frame
                    .as_ref()
                    .and_then(|frame| frame.w)
                    .map_or(width, |value| value.to_pt().max(1.0));
                let (mut block, direction, mut paint) = layout_paragraph_with_source_and_direction(
                    paragraph,
                    measure,
                    styles,
                    input,
                    &part_media,
                    fm,
                    num_state,
                    diagnostics,
                    source,
                    sources.and_then(|registry| registry.id(&story, &path)),
                    None,
                    None,
                )?;
                if !cache_source && let Some(registry) = sources {
                    registry.bind_text_boxes(&mut block, &mut paint, source)?;
                }
                blocks.push(block);
                directions.push(paginator::HeaderFooterBlockSemantics {
                    paint: Arc::new(paint),
                    direction,
                    placement: frame.map_or(paginator::HeaderFooterPlacement::Flow, |properties| {
                        paginator::HeaderFooterPlacement::Frame {
                            properties,
                            paragraphs: 1,
                        }
                    }),
                    ..Default::default()
                });
            }
            MainStoryLayoutItem::Table(table, path) => {
                let (table, semantics) = table::layout_table_with_provenance(
                    table,
                    width,
                    styles,
                    input,
                    &part_media,
                    fm,
                    num_state,
                    diagnostics,
                    sources,
                    &story,
                    &path,
                    None,
                )?;
                let rows =
                    paginator::story_table_rows(&table, &semantics, &geometry, part_media.media())?;
                let table = Arc::new(table);
                let semantics = Arc::new(semantics);
                directions.extend(rows.iter().enumerate().map(|(row, (_, paint))| {
                    paginator::HeaderFooterBlockSemantics {
                        paint: Arc::new(paint.clone()),
                        placement: paginator::HeaderFooterPlacement::TableRow {
                            table: Arc::clone(&table),
                            semantics: Arc::clone(&semantics),
                            row,
                        },
                        ..Default::default()
                    }
                }));
                blocks.extend(rows.into_iter().map(|(block, _)| block));
            }
        }
    }
    let mut index = 0;
    while index < blocks.len() {
        if let paginator::HeaderFooterPlacement::Frame { properties, .. } =
            &directions[index].placement
        {
            let properties = properties.clone();
            let mut count = 1;
            while index + count < blocks.len()
                && matches!(&directions[index + count].placement, paginator::HeaderFooterPlacement::Frame { properties: next, .. } if *next == properties)
            {
                directions[index + count].placement =
                    paginator::HeaderFooterPlacement::FrameContinuation;
                count += 1;
            }
            directions[index].placement = paginator::HeaderFooterPlacement::Frame {
                properties,
                paragraphs: count,
            };
            price_furniture_paragraphs(
                &mut blocks[index..index + count],
                &mut directions[index..index + count],
                input.do_not_use_html_paragraph_auto_spacing,
            );
            index += count;
        } else {
            index += 1;
        }
    }
    let ordinary = blocks
        .iter()
        .zip(&directions)
        .map(|(block, facts)| {
            matches!(facts.placement, paginator::HeaderFooterPlacement::Flow).then_some(block)
        })
        .collect::<Vec<_>>();
    let groups = block::paragraph_border_groups(&ordinary);
    let mut after: f64 = 0.0;
    for index in 0..blocks.len() {
        let flow = matches!(
            &directions[index].placement,
            paginator::HeaderFooterPlacement::Flow
        ) || matches!(&directions[index].placement, paginator::HeaderFooterPlacement::TableRow { table, .. } if table.floating.is_none());
        if !flow {
            continue;
        }
        let gap = if input.do_not_use_html_paragraph_auto_spacing {
            after + blocks[index].space_before
        } else {
            after.max(blocks[index].space_before)
        };
        directions[index].flow_gap = gap;
        if matches!(
            directions[index].placement,
            paginator::HeaderFooterPlacement::Flow
        ) {
            directions[index].border_gap = if groups[index].0 { gap } else { 0.0 };
            blocks[index].remove_group_border_edges(groups[index].0, groups[index].1);
        }
        after = blocks[index].space_after;
    }
    Ok(HeaderFooterVariantContent {
        blocks,
        directions,
        watermark,
    })
}

fn price_furniture_paragraphs(
    blocks: &mut [ParagraphBlock],
    facts: &mut [paginator::HeaderFooterBlockSemantics],
    additive: bool,
) {
    let groups = block::paragraph_border_groups(&blocks.iter().map(Some).collect::<Vec<_>>());
    let mut after: f64 = 0.0;
    for (index, block) in blocks.iter_mut().enumerate() {
        let gap = if additive {
            after + block.space_before
        } else {
            after.max(block.space_before)
        };
        facts[index].flow_gap = gap;
        facts[index].border_gap = if groups[index].0 { gap } else { 0.0 };
        block.remove_group_border_edges(groups[index].0, groups[index].1);
        after = block.space_after;
    }
}

fn layout_watermark(
    watermark: &VmlWatermark,
    header_relationship_id: &str,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    geometry: PageGeometry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<GroupElement>> {
    let (width, height, rotation, opacity) = match watermark {
        VmlWatermark::Text {
            width_pt,
            height_pt,
            rotation_degrees,
            opacity,
            ..
        }
        | VmlWatermark::Image {
            width_pt,
            height_pt,
            rotation_degrees,
            opacity,
            ..
        } => (*width_pt, *height_pt, *rotation_degrees, *opacity),
    };
    let translate = Transform {
        e: geometry.margin_left + (geometry.content_width() - width) / 2.0,
        f: geometry.margin_top + (geometry.content_height() - height) / 2.0,
        ..Transform::IDENTITY
    };
    let transform = Transform::rotate_about(rotation, width / 2.0, height / 2.0).then(translate);
    let children = match watermark {
        VmlWatermark::Text {
            text,
            color,
            font_family,
            ..
        } => {
            let Some(color) = vml_color(color) else {
                diagnostics.push(Diagnostic {
                    message: format!("VML watermark colour {color:?} is unsupported"),
                });
                return Ok(None);
            };
            let estimated = width / (text.chars().count().max(1) as f64 * 0.62);
            let font_size = (height * 0.62).min(estimated).max(1.0);
            let font_id = fm.resolve_font_for_text(
                font_family.as_deref().or(Some("Calibri")),
                false,
                false,
                text,
            )?;
            let shaped = fm.shape_text(font_id, text, font_size)?;
            let metrics = fm.metrics(font_id, font_size)?;
            vec![PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: (width - shaped.width) / 2.0,
                    y: (height + metrics.ascent - metrics.descent) / 2.0,
                },
                font_id,
                font_size,
                glyph_ids: shaped.glyph_ids,
                advances: shaped.advances,
                text: text.clone(),
                source: None,
                color,
                bold: false,
                italic: false,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            })]
        }
        VmlWatermark::Image {
            relationship_id, ..
        } => {
            let scoped_id = format!("{header_relationship_id}\0{relationship_id}");
            let Some(image) = input.images.get(&scoped_id) else {
                diagnostics.push(Diagnostic {
                    message: format!(
                        "VML watermark image relationship {relationship_id} in header {header_relationship_id} was not resolved"
                    ),
                });
                return Ok(None);
            };
            media.record_related_picture_draw_owner(relationship_id, header_relationship_id);
            let data = image.data.clone();
            vec![PositionedElement::Image {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height,
                },
                content_type: image.content_type.clone(),
                media_id: media.id_for_relationship(&scoped_id),
                data,
            }]
        }
    };
    Ok(Some(GroupElement {
        transform,
        clip: None,
        opacity,
        effects: Vec::new(),
        children,
    }))
}

fn vml_color(value: &str) -> Option<Color> {
    let normalized = value.trim().to_ascii_lowercase();
    let hex = match normalized.as_str() {
        "black" => "000000",
        "silver" => "c0c0c0",
        "gray" | "grey" => "808080",
        "white" => "ffffff",
        "maroon" => "800000",
        "red" => "ff0000",
        "purple" => "800080",
        "fuchsia" | "magenta" => "ff00ff",
        "green" => "008000",
        "lime" => "00ff00",
        "olive" => "808000",
        "yellow" => "ffff00",
        "navy" => "000080",
        "blue" => "0000ff",
        "teal" => "008080",
        "aqua" | "cyan" => "00ffff",
        _ => normalized.trim_start_matches('#'),
    };
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| Color::from_hex(hex))
}
