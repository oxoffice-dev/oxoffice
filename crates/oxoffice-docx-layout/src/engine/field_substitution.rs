//! Fields resolved after pagination: page background, STYLEREF, page fields, tab realignment.

use super::*;

/// Apply page background color from `w:background` element to all pages but
/// the blank pages added before odd-page and even-page sections.
pub(super) fn apply_page_background(
    pages: &mut [PageFrame],
    paint: &mut paginator::PagePaintRecords,
    input: &LayoutInput,
    blank_pages: &[usize],
) -> Result<()> {
    let bg_xml = match &input.document.background_xml {
        Some(xml) => xml,
        None => return Ok(()),
    };

    // Parse w:color attribute from background XML
    let xml_str = std::str::from_utf8(bg_xml).unwrap_or("");
    let color = extract_background_color(xml_str);
    let color = match color {
        Some(c) => c,
        None => return Ok(()),
    };

    // Insert a full-page FilledRect at position 0 on every page (renders underneath everything)
    paint.validate(pages)?;
    for (index, page) in pages.iter_mut().enumerate() {
        if blank_pages.contains(&index) {
            continue;
        }
        let mut painted = PaintedElements::from_parts(
            std::mem::take(&mut page.elements),
            std::mem::take(&mut paint.policies[index]),
        )?;
        let mut background = PaintedElements::non_field(vec![PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: page.width,
                height: page.height,
            },
            color,
        }])?;
        painted.splice_at(&[], 0..0, &mut background)?;
        (page.elements, paint.policies[index]) = painted.into_parts()?;
        let boundary = &mut paint.boundaries[index];
        boundary.behind_end = boundary
            .behind_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.regular_end = boundary
            .regular_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.body_front_end = boundary
            .body_front_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.furniture_front_end = boundary
            .furniture_front_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.elements_len = boundary
            .elements_len
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
    }
    paint.validate(pages)
}

/// Extract the background color hex from w:background XML.
fn extract_background_color(xml: &str) -> Option<Color> {
    // Look for w:color="RRGGBB" or color="RRGGBB"
    for attr in ["w:color=\"", "color=\""] {
        if let Some(start) = xml.find(attr) {
            let val_start = start + attr.len();
            if let Some(end) = xml[val_start..].find('"') {
                let hex = &xml[val_start..val_start + end];
                if hex.len() == 6 && hex != "auto" {
                    return Some(Color::from_hex(hex));
                }
            }
        }
    }
    None
}

pub(super) fn supported_styleref(instruction: &FieldInstruction) -> bool {
    instruction.name == "STYLEREF"
        && instruction.quotes_are_balanced()
        && instruction.arguments.len() == 1
        && field_text_argument(instruction, 0).is_some()
        && instruction
            .switches
            .iter()
            .all(|switch| match switch.name.as_str() {
                "*" | "#" => matches!(switch.argument, Some(FieldArgument::Text(_))),
                "l" | "n" | "r" | "w" | "t" | "p" => switch.argument.is_none(),
                _ => false,
            })
}

pub(super) fn placed_styleref_values(
    input: &LayoutInput,
    registry: &SourceRegistry,
    numbering: &NumberingState,
    pages: &[Arc<oxoffice_layout::PageFrame>],
    placements: &[crate::WordFieldPlacement],
) -> HashMap<usize, HashMap<FieldSource, String>> {
    if input
        .sequence_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.navigation_failure.is_some())
    {
        return HashMap::new();
    }
    let mut source_pages = HashMap::new();
    for page in pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let source = match element {
                PositionedElement::Text(run) => run.source,
                PositionedElement::MultilingualText(run) => run.source,
                _ => None,
            };
            if let Some(source) = source {
                source_pages.entry(source.node).or_insert(page.page_number);
            }
        });
    }
    let mut aliases: HashMap<&str, Vec<String>> = HashMap::new();
    for style in &input.styles.styles {
        let names = aliases.entry(&style.style_id).or_default();
        names.push(style.style_id.to_lowercase());
        if let Some(name) = &style.name {
            names.push(name.to_lowercase());
        }
        names.sort();
        names.dedup();
    }
    let mut styles: HashMap<String, Vec<SourceNodeId>> = HashMap::new();
    let mut style_pages: HashMap<String, std::collections::BTreeMap<usize, Vec<SourceNodeId>>> =
        HashMap::new();
    for (index, path) in registry.nodes.iter().enumerate() {
        if path.story != WordStory::Document {
            continue;
        }
        let Some(node) = u32::try_from(index + 1).ok().and_then(SourceNodeId::new) else {
            continue;
        };
        let style = numbering
            .navigation
            .get(&node)
            .and_then(|facts| facts.style.as_deref());
        for alias in style
            .and_then(|style| aliases.get(style))
            .into_iter()
            .flatten()
        {
            styles.entry(alias.clone()).or_default().push(node);
            if let Some(page) = source_pages.get(&node) {
                style_pages
                    .entry(alias.clone())
                    .or_default()
                    .entry(*page)
                    .or_default()
                    .push(node);
            }
        }
    }
    let mut values: HashMap<usize, HashMap<FieldSource, String>> = HashMap::new();
    for placement in placements {
        let Some(instruction) = registry.styleref_requests.get(&placement.source) else {
            continue;
        };
        let Some(target) = field_text_argument(instruction, 0) else {
            continue;
        };
        let key = target.to_lowercase();
        let caller = &registry.nodes[placement.source.node.get() as usize - 1];
        let last = field_has_switch(instruction, "l");
        let selected = if caller.story == WordStory::Document && !last {
            styles.get(&key).and_then(|nodes| {
                let boundary =
                    nodes.partition_point(|node| node.get() < placement.source.node.get());
                boundary
                    .checked_sub(1)
                    .and_then(|i| nodes.get(i))
                    .or_else(|| nodes.iter().find(|node| **node != placement.source.node))
                    .copied()
            })
        } else {
            style_pages.get(&key).and_then(|pages| {
                pages
                    .get(&placement.physical_page)
                    .and_then(|nodes| {
                        if last {
                            nodes
                                .iter()
                                .rev()
                                .find(|node| **node != placement.source.node)
                        } else {
                            nodes.iter().find(|node| **node != placement.source.node)
                        }
                    })
                    .copied()
                    .or_else(|| {
                        pages
                            .range(..placement.physical_page)
                            .next_back()
                            .and_then(|(_, nodes)| nodes.last())
                            .copied()
                    })
                    .or_else(|| {
                        pages
                            .range((
                                std::ops::Bound::Excluded(placement.physical_page),
                                std::ops::Bound::Unbounded,
                            ))
                            .next()
                            .and_then(|(_, nodes)| nodes.first())
                            .copied()
                    })
            })
        };
        let Some(selected) = selected.filter(|node| source_pages.contains_key(node)) else {
            continue;
        };
        let Some(facts) = numbering.navigation.get(&selected) else {
            continue;
        };
        let numbered = ["n", "r", "w"]
            .into_iter()
            .any(|name| field_has_switch(instruction, name));
        let value = if numbered {
            match numbering.source(selected) {
                Some(number) => number
                    .numbered_reference_text(instruction, numbering.source(placement.source.node)),
                None if facts.suppressed_numbering => Ok("0".into()),
                None => continue,
            }
        } else {
            Ok(facts.text.clone())
        };
        let Ok(mut value) = value else { continue };
        if field_has_switch(instruction, "p") {
            if caller.story != WordStory::Document {
                continue;
            }
            value.push_str(if selected.get() < placement.source.node.get() {
                " above"
            } else {
                " below"
            });
        }
        let Ok(value) = format_numeric_field_general(instruction, &value) else {
            continue;
        };
        values
            .entry(placement.physical_page)
            .or_default()
            .insert(placement.source, value);
    }
    values
}

fn render_annotation_replacement(
    recipe: Arc<WordAnnotationRecipe>,
    value: &str,
    placement: [u64; 4],
    fonts: &mut FontManager,
) -> Result<(PaintedElements, Vec<Diagnostic>)> {
    let decode_color = |bits: [u64; 4]| Color {
        r: f64::from_bits(bits[0]),
        g: f64::from_bits(bits[1]),
        b: f64::from_bits(bits[2]),
        a: f64::from_bits(bits[3]),
    };
    let style = &recipe.style;
    let base = AnnotationBase {
        occurrence: Some(recipe.occurrence),
        field_kind: recipe.field_kind,
        kern: style.kern,
        font_id: style.font_id,
        font_size: f64::from_bits(style.font_size),
        color: decode_color(style.color),
        bold: style.bold,
        italic: style.italic,
        baseline_offset: f64::from_bits(style.baseline_offset),
        spacing: f64::from_bits(style.spacing),
        underline: style.underline,
        strike: style.strike,
        dstrike: style.dstrike,
        highlight: style.highlight.map(decode_color),
        font_family: style.font_family.as_deref(),
        fallback_emphasis: recipe.fallback_emphasis.as_ref(),
    };
    let mut items = Vec::new();
    let mut styles = HashMap::new();
    let mut diagnostics = Vec::new();
    if value.is_empty() {
        push_empty_field_annotation(
            &mut items,
            &mut styles,
            recipe.kind.clone(),
            &base,
            recipe.source_span,
        )?;
    } else {
        let mut applied = match &recipe.kind {
            WordAnnotationKind::EastAsian(layout) => push_east_asian_layout_text(
                &mut items,
                &mut styles,
                fonts,
                layout,
                value,
                &base,
                recipe.source_span,
            )?,
            WordAnnotationKind::Emphasis(mark) => push_emphasis_marked_text(
                &mut items,
                &mut styles,
                fonts,
                &mut diagnostics,
                mark,
                value,
                &base,
            )?,
        };
        if !applied && let Some(mark) = &recipe.fallback_emphasis {
            applied = push_emphasis_marked_text(
                &mut items,
                &mut styles,
                fonts,
                &mut diagnostics,
                mark,
                value,
                &base,
            )?;
        }
        if !applied {
            let mut shaped =
                fonts.shape_text_with_kerning(base.font_id, value, base.font_size, base.kern)?;
            for advance in &mut shaped.advances {
                *advance += base.spacing;
            }
            shaped.width = shaped.advances.iter().sum();
            let baseline = f64::from_bits(placement[1]) - base.baseline_offset;
            let mut output = PaintedElements::default();
            let mut children = Vec::new();
            if let Some(color) = base.highlight {
                children.push(PositionedElement::FilledRect {
                    rect: Rect {
                        x: f64::from_bits(placement[0]),
                        y: f64::from_bits(placement[2]),
                        width: shaped.width,
                        height: f64::from_bits(placement[3]),
                    },
                    color,
                });
            }
            let mut policies = PaintPolicies::default();
            policies.emit(
                &mut children,
                PositionedElement::Text(GlyphRun {
                    origin: Point {
                        x: f64::from_bits(placement[0]),
                        y: baseline,
                    },
                    font_id: base.font_id,
                    font_size: base.font_size,
                    glyph_ids: shaped.glyph_ids,
                    advances: shaped.advances,
                    text: value.to_owned(),
                    source: recipe.source_span,
                    color: base.color,
                    bold: base.bold,
                    italic: base.italic,
                    field_kind: base.field_kind,
                    field_source: Some(recipe.occurrence.source),
                    note: None,
                    note_reference_source: None,
                    tab_aligned: None,
                }),
                annotation_field_owner(&base),
            )?;
            let metrics = fonts.metrics(base.font_id, base.font_size)?;
            let mut decorations = Vec::new();
            push_annotation_decorations(
                &mut decorations,
                &base,
                baseline,
                shaped.width,
                metrics.ascent,
                metrics.descent,
            );
            for decoration in &mut decorations {
                let PositionedElement::Line { start, end, .. } = decoration else {
                    return Err(LayoutError::Layout(
                        "annotation fallback decoration kind changed".into(),
                    ));
                };
                start.x += f64::from_bits(placement[0]);
                end.x += f64::from_bits(placement[0]);
            }
            children.extend(decorations);
            output.append(&mut PaintedElements::from_parts(children, policies)?)?;
            let (roots, mut policies) = output
                .wrap(PositionedElement::Group(GroupElement {
                    transform: Transform::IDENTITY,
                    clip: None,
                    opacity: 1.0,
                    effects: Vec::new(),
                    children: Vec::new(),
                }))?
                .into_parts()?;
            let count = policies.field_count();
            policies.attach_annotation(
                &roots,
                &[0],
                recipe,
                WordAnnotationRootKind::Group,
                count,
                placement,
            )?;
            return Ok((PaintedElements::from_parts(roots, policies)?, diagnostics));
        }
    }
    let mut output = PaintedElements::default();
    let mut x = f64::from_bits(placement[0]);
    for (index, item) in items.into_iter().enumerate() {
        let InlineItem::Group {
            width,
            baseline,
            mut group,
            ..
        } = item
        else {
            return Err(LayoutError::Layout(
                "annotation replacement lost its atomic producer".into(),
            ));
        };
        let owner = styles
            .get(&index)
            .and_then(|style| style.annotation.as_ref())
            .ok_or_else(|| {
                LayoutError::Layout("annotation replacement lost its private companion".into())
            })?;
        let y = baseline.map_or(f64::from_bits(placement[2]), |baseline| {
            f64::from_bits(placement[1]) - baseline
        });
        group.transform = group.transform.then(Transform {
            e: x,
            f: y,
            ..Transform::IDENTITY
        });
        let children = PaintedElements::from_parts(
            std::mem::take(&mut group.children),
            owner.policies.clone(),
        )?;
        output.append(&mut children.wrap(PositionedElement::Group(group))?)?;
        x += width;
    }
    let (roots, mut policies) = output
        .wrap(PositionedElement::Group(GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: Vec::new(),
        }))?
        .into_parts()?;
    let count = policies.field_count();
    policies.attach_annotation(
        &roots,
        &[0],
        recipe,
        if value.is_empty() {
            WordAnnotationRootKind::EmptySpan
        } else {
            WordAnnotationRootKind::Group
        },
        count,
        placement,
    )?;
    Ok((PaintedElements::from_parts(roots, policies)?, diagnostics))
}

fn field_replacement(
    element: &PositionedElement,
    value: &str,
    policy: Option<(bool, bool)>,
    occurrence: Option<WordFieldOccurrence>,
    fonts: &mut FontManager,
) -> Result<PaintedElements> {
    let owner = policy.map_or(FieldPaintOwner::Neutral, |(kern, legacy)| {
        FieldPaintOwner::Word {
            kern,
            legacy,
            occurrence,
        }
    });
    let mut output = PaintedElements::default();
    match element {
        PositionedElement::Text(run) => {
            let shaped = if let Some((kern, _)) = policy {
                fonts.shape_text_with_kerning(run.font_id, value, run.font_size, kern)?
            } else {
                fonts.shape_text(run.font_id, value, run.font_size)?
            };
            let mut replacement = run.clone();
            replacement.text = value.to_owned();
            replacement.glyph_ids = shaped.glyph_ids;
            replacement.advances = shaped.advances;
            let element = PositionedElement::Text(replacement);
            if run.field_kind.is_some() || run.field_source.is_some() {
                output.push_field(element, owner)?;
            } else {
                output.push_non_field(element)?;
            }
        }
        PositionedElement::MultilingualText(run) => {
            if policy.is_some_and(|(_, legacy)| legacy) {
                return Err(LayoutError::Layout(
                    "legacy field owner reached a rich positioned leaf".into(),
                ));
            }
            if value.is_empty() {
                let mut replacement = run.clone();
                replacement.logical_text.clear();
                replacement.glyph_ids.clear();
                replacement.x_advances.clear();
                replacement.y_advances.clear();
                replacement.x_offsets.clear();
                replacement.y_offsets.clear();
                replacement.clusters.clear();
                let element = PositionedElement::MultilingualText(replacement);
                if run.field_kind.is_some() || run.field_source.is_some() {
                    output.push_field(element, owner)?;
                } else {
                    output.push_non_field(element)?;
                }
                return Ok(output);
            }
            let segment = TextSegment {
                form_mark: None,
                text: value.to_owned(),
                direction: run.direction,
                source: run.source,
                font_id: run.font_id,
                measure_font: None,
                font_size: run.font_size,
                glyph_ids: Vec::new(),
                advances: Vec::new(),
                width: 0.0,
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
                color: run.color,
                bold: run.bold,
                italic: run.italic,
                underline: None,
                strike: false,
                dstrike: false,
                highlight: None,
                baseline_offset: 0.0,
                hyperlink_url: None,
                field_kind: run.field_kind,
                field_source: run.field_source,
                note: run.note,
                note_reference_source: run.note_reference_source,
            };
            let mut spans = if let Some((kern, legacy)) = policy {
                fonts.shape_paragraph_with_kerning(
                    vec![(segment, run.language.clone())],
                    run.direction,
                    true,
                    &[kern],
                    legacy,
                )?
            } else {
                fonts.shape_multilingual_text(
                    segment,
                    run.language.as_deref(),
                    run.direction,
                    true,
                )?
            };
            if spans.len() == 1 {
                let mut child = run.clone();
                apply_multilingual_field_span(
                    &mut child,
                    &spans.pop().expect("one field span"),
                    run.logical_index,
                );
                if child.field_kind.is_some() || child.field_source.is_some() {
                    output.push_field(PositionedElement::MultilingualText(child), owner)?;
                } else {
                    output.push_non_field(PositionedElement::MultilingualText(child))?;
                }
            } else {
                let natural_width = spans.iter().map(|span| span.width()).sum::<f64>();
                if spans.is_empty() || !natural_width.is_finite() {
                    return Err(LayoutError::Layout("invalid rich field replacement".into()));
                }
                let items = spans
                    .into_iter()
                    .map(InlineItem::MultilingualText)
                    .collect::<Vec<_>>();
                let params = oxoffice_layout::LineBreakParams {
                    available_width: natural_width.max(f64::EPSILON),
                    wrap: false,
                    ..Default::default()
                };
                let lines = break_multilingual_into_lines(&items, &params, fonts, run.direction)?;
                let [line] = lines.as_slice() else {
                    return Err(LayoutError::Layout(
                        "rich field replacement crossed a line".into(),
                    ));
                };
                let mut x = run.origin.x;
                for item in &line.items {
                    let LineItem::MultilingualText(span) = item else {
                        return Err(LayoutError::Layout(
                            "rich field replacement lost its producer".into(),
                        ));
                    };
                    let mut child = run.clone();
                    apply_multilingual_field_span(&mut child, span, run.logical_index);
                    child.origin.x = x;
                    x += span.width();
                    if child.field_kind.is_some() || child.field_source.is_some() {
                        output.push_field(PositionedElement::MultilingualText(child), owner)?;
                    } else {
                        output.push_non_field(PositionedElement::MultilingualText(child))?;
                    }
                }
                output = output.wrap(PositionedElement::Group(GroupElement {
                    transform: Transform::IDENTITY,
                    clip: None,
                    opacity: 1.0,
                    effects: Vec::new(),
                    children: Vec::new(),
                }))?;
            }
        }
        _ => {
            return Err(LayoutError::Layout(
                "field replacement requires a glyph leaf".into(),
            ));
        }
    }
    Ok(output)
}

impl FieldOccurrenceState {
    fn new(painted: &PaintedElements) -> Result<Self> {
        let occurrences = painted.policies().annotation_occurrences()?;
        let mut annotated = std::collections::HashSet::new();
        annotated
            .try_reserve(occurrences.len())
            .map_err(|_| LayoutError::Layout("annotation occurrence allocation refused".into()))?;
        annotated.extend(occurrences);
        Ok(Self {
            annotated,
            consumed: std::collections::HashSet::new(),
            diagnostics: Vec::new(),
        })
    }

    fn consume(&mut self, occurrence: WordFieldOccurrence) -> Result<()> {
        self.consumed
            .try_reserve(1)
            .map_err(|_| LayoutError::Layout("annotation traversal allocation refused".into()))?;
        self.consumed.insert(occurrence);
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn late_field_value(
    kind: FieldKind,
    source: Option<FieldSource>,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
) -> (Option<String>, bool) {
    let sections = source
        .and_then(|source| field_sections.get(&source))
        .copied()
        .unwrap_or((section_values.0, section_values.1, page_number));
    let result = source.and_then(|source| page_field_values.get(&source));
    let value = if let Some(value) = result {
        Some(value.display.clone())
    } else {
        match kind {
            FieldKind::Page => Some(sections.2.to_string()),
            FieldKind::NumPages => Some(total_pages.to_string()),
            FieldKind::Section => Some(sections.0.to_string()),
            FieldKind::SectionPages => Some(sections.1.to_string()),
            FieldKind::TargetPage(target) => bookmark_pages.get(&target).map(ToString::to_string),
            FieldKind::SequenceRepeat => {
                sequence_repeat_display(sequence, source, sequence_boundary)
            }
            FieldKind::Target(_) | FieldKind::SequenceContext(_) => None,
        }
    };
    let clear = matches!(kind, FieldKind::TargetPage(_))
        && (result.is_some_and(|value| value.target_unplaced) || value.is_none());
    (value, clear)
}

pub(super) fn substitute_styleref(
    painted: &mut PaintedElements,
    values: &HashMap<FieldSource, String>,
    seen: &mut std::collections::HashSet<FieldSource>,
    fonts: &mut FontManager,
    changes: &mut Vec<FieldWidthChange>,
    state: &mut FieldOccurrenceState,
) -> Result<()> {
    painted.policies().validate(painted.elements())?;
    let mut index = 0;
    while index < painted.len() {
        if let Some((recipe, _, placement)) = painted.policies().annotation(&[index]) {
            let source = recipe.occurrence.source;
            let Some(value) = values.get(&source) else {
                index += 1;
                continue;
            };
            if state.consumed.contains(&recipe.occurrence) || value.is_empty() {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
                state.consume(recipe.occurrence)?;
                continue;
            }
            let (mut replacement, diagnostics) =
                render_annotation_replacement(Arc::clone(&recipe), value, placement, fonts)?;
            state.consume(recipe.occurrence)?;
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
            state.diagnostics.extend(diagnostics);
            index += 1;
            continue;
        }
        let element = &painted.elements()[index];
        if let Some(children_len) = match element {
            PositionedElement::Group(group) => Some(group.children.len()),
            PositionedElement::MarkedContent { children, .. } => Some(children.len()),
            _ => None,
        } {
            let mut children =
                painted.splice_at(&[index], 0..children_len, &mut PaintedElements::default())?;
            substitute_styleref(&mut children, values, seen, fonts, changes, state)?;
            painted.splice_at(&[index], 0..0, &mut children)?;
            index += 1;
            continue;
        }
        let source = match element {
            PositionedElement::Text(run) => run.field_source,
            PositionedElement::MultilingualText(run) => run.field_source,
            _ => None,
        };
        let Some((source, value)) =
            source.and_then(|source| values.get(&source).map(|value| (source, value)))
        else {
            index += 1;
            continue;
        };
        let policy = painted.policies().required(&[index])?;
        let occurrence = painted.policies().field_occurrence(&[index])?;
        let annotated_owner = occurrence.filter(|occurrence| state.annotated.contains(occurrence));
        let consumed = annotated_owner.map_or_else(
            || seen.contains(&source),
            |occurrence| state.consumed.contains(&occurrence),
        );
        let value = if consumed { "" } else { value.as_str() };
        if let Some(occurrence) = annotated_owner {
            state.consume(occurrence)?;
        } else {
            seen.try_reserve(1)
                .map_err(|_| LayoutError::Layout("field traversal allocation refused".into()))?;
            seen.insert(source);
        }
        if value.is_empty() {
            if let PositionedElement::Text(old) = element
                && let Some(aligned) = old.tab_aligned
            {
                changes.push(FieldWidthChange {
                    aligned,
                    baseline: old.origin.y,
                    font_size: old.font_size,
                    x: old.origin.x,
                    delta: -old.advances.iter().sum::<f64>(),
                });
            }
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let mut replacement = field_replacement(element, value, policy, occurrence, fonts)?;
        if let (PositionedElement::Text(old), [PositionedElement::Text(new)]) =
            (element, replacement.elements())
            && let Some(aligned) = old.tab_aligned
        {
            changes.push(FieldWidthChange {
                aligned,
                baseline: old.origin.y,
                font_size: old.font_size,
                x: old.origin.x,
                delta: new.advances.iter().sum::<f64>() - old.advances.iter().sum::<f64>(),
            });
        }
        painted.splice_at(&[], index..index + 1, &mut replacement)?;
        index += 1;
    }
    Ok(())
}

pub(super) fn sequence_event_boundary(elements: &[PositionedElement], previous: usize) -> usize {
    elements
        .iter()
        .fold(previous, |boundary, element| match element {
            PositionedElement::Text(run) => match run.field_kind {
                Some(FieldKind::SequenceContext(event)) => boundary.max(event + 1),
                _ => boundary,
            },
            PositionedElement::MultilingualText(run) => match run.field_kind {
                Some(FieldKind::SequenceContext(event)) => boundary.max(event + 1),
                _ => boundary,
            },
            PositionedElement::Group(group) => sequence_event_boundary(&group.children, boundary),
            PositionedElement::MarkedContent { children, .. } => {
                sequence_event_boundary(children, boundary)
            }
            _ => boundary,
        })
}

fn sequence_repeat_display(
    snapshot: Option<&crate::WordSequenceSnapshot>,
    source: Option<FieldSource>,
    boundary: usize,
) -> Option<String> {
    let snapshot = snapshot?;
    let (instruction, stored_display) = snapshot.repeat_requests.get(&source?)?;
    let value = field_text_argument(instruction, 0)
        .and_then(|identifier| snapshot.context_value(boundary, identifier))
        .and_then(|value| format_sequence_display(instruction, value.to_string()).ok());
    Some(value.unwrap_or_else(|| stored_display.clone()))
}

/// Replace field placeholder GlyphRuns with actual values.
///
/// The changes in width of fields in aligned text (after a right, centre or
/// decimal tab stop, or on a right-aligned or centred line) are added to
/// `changes`, for [`realign_tab_aligned_text`] to move that text once every
/// field of the page, STYLEREF included, holds its value.
#[allow(clippy::too_many_arguments)]
pub(super) fn substitute_fields(
    elements: &mut PaintedElements,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
    fm: &mut FontManager,
    changes: &mut Vec<FieldWidthChange>,
) -> Result<FieldOccurrenceState> {
    let mut state = FieldOccurrenceState::new(elements)?;
    substitute_field_values(
        elements,
        page_number,
        total_pages,
        section_values,
        field_sections,
        bookmark_pages,
        page_field_values,
        sequence,
        sequence_boundary,
        fm,
        changes,
        &mut state,
    )?;
    Ok(state)
}

/// A tab-aligned field whose value is wider or narrower than its placeholder.
pub(super) struct FieldWidthChange {
    aligned: oxoffice_layout::TabAlignedField,
    baseline: f64,
    font_size: f64,
    /// Where the placeholder started.
    x: f64,
    delta: f64,
}

impl FieldWidthChange {
    fn same_text(&self, other: &Self) -> bool {
        (self.baseline - other.baseline).abs() < 0.01
            && (self.aligned.start - other.aligned.start).abs() < 0.01
    }
}

/// How far a position moves once the fields of tab-aligned text hold their
/// values, or `None` outside such text.
///
/// Text after a right, centre or decimal tab moves back by the share of the
/// changes its stop aligns on, as far as its tab allows, and past each field
/// before the position by that field's change. A tab with no width left
/// starts its text where the text before it now ends.
fn realigned_offset(changes: &[FieldWidthChange], x: f64, baseline: f64) -> Option<f64> {
    let mut starts = changes
        .iter()
        .filter(|change| (change.baseline - baseline).abs() < 0.01)
        .collect::<Vec<_>>();
    starts.sort_by(|a, b| a.aligned.start.total_cmp(&b.aligned.start));
    starts.dedup_by(|a, b| a.same_text(b));
    // Where the previous aligned text ended, and how far that end moved.
    let mut carry = None::<(f64, f64)>;
    for first in starts {
        let aligned = first.aligned;
        let fields = changes.iter().filter(|change| change.same_text(first));
        let shift: f64 = fields
            .clone()
            .map(|change| change.aligned.shift * change.delta)
            .sum();
        let old_width = aligned.gap.max(0.0);
        let moved = carry
            .filter(|(end, _)| (aligned.start - old_width - end).abs() < 0.01)
            .map_or(0.0, |(_, moved)| moved);
        let start = moved + (aligned.gap - moved - shift).max(0.0) - old_width;
        if x > aligned.start - 0.01 && x < aligned.end - 0.001 {
            return Some(
                start
                    + fields
                        .filter(|change| change.x < x - 0.001)
                        .map(|change| change.delta)
                        .sum::<f64>(),
            );
        }
        carry = Some((
            aligned.end,
            start + fields.map(|change| change.delta).sum::<f64>(),
        ));
    }
    None
}

/// Moves a rectangle over tab-aligned text with that text, and widens it by
/// the change of the fields it covers. Returns whether it moved.
fn realign_rect(rect: &mut oxoffice_layout::Rect, changes: &[FieldWidthChange]) -> bool {
    let covers = |change: &&FieldWidthChange| {
        rect.y <= change.baseline && change.baseline <= rect.y + rect.height
    };
    if let Some(offset) = changes
        .iter()
        .filter(covers)
        .find_map(|change| realigned_offset(changes, rect.x, change.baseline))
    {
        let width_change = own_width_change(changes, rect.x, covers);
        rect.width += width_change;
        rect.x += offset;
        return true;
    }
    false
}

/// Stretches or shrinks a fill that ends where tab-aligned text starts, a
/// filled tab's, so it still reaches that text once its fields hold their
/// values, as [`realign_tab_leader`] does for a leader.
fn realign_tab_fill(rect: &mut oxoffice_layout::Rect, changes: &[FieldWidthChange]) {
    let end = rect.x + rect.width;
    let Some(change) = changes.iter().find(|change| {
        rect.y <= change.baseline
            && change.baseline <= rect.y + rect.height
            && (end - change.aligned.start).abs() < 0.01
    }) else {
        return;
    };
    if let Some(offset) = realigned_offset(changes, change.aligned.start, change.baseline) {
        rect.width = (rect.width + offset).max(0.0);
    }
}

pub(super) fn realign_tab_aligned_text(
    elements: &mut [PositionedElement],
    changes: &[FieldWidthChange],
) {
    for element in elements.iter_mut() {
        match element {
            PositionedElement::Text(run) => {
                if let Some(offset) = realigned_offset(changes, run.origin.x, run.origin.y) {
                    run.origin.x += offset;
                } else {
                    realign_tab_leader(run, changes);
                }
            }
            PositionedElement::MultilingualText(run) => {
                if let Some(offset) = realigned_offset(changes, run.origin.x, run.origin.y) {
                    run.origin.x += offset;
                }
            }
            PositionedElement::Line { start, end, .. } => {
                // An underline or a strike lies between the ascent and the
                // descent of the text it marks.
                let marks = |change: &&FieldWidthChange| {
                    start.y > change.baseline - change.font_size
                        && start.y < change.baseline + change.font_size / 2.0
                };
                if let Some(offset) = changes
                    .iter()
                    .filter(marks)
                    .find_map(|change| realigned_offset(changes, start.x, change.baseline))
                {
                    end.x += offset + own_width_change(changes, start.x, marks);
                    start.x += offset;
                }
            }
            PositionedElement::FilledRect { rect, .. } => {
                if !realign_rect(rect, changes) {
                    realign_tab_fill(rect, changes);
                }
            }
            PositionedElement::LinkAnnotation { rect, .. } => {
                realign_rect(rect, changes);
            }
            // A form marker moves with the text it stands over, as a link
            // rectangle does.
            PositionedElement::FormField(fragment) if fragment.mark.is_some() => {
                realign_rect(&mut fragment.rect, changes);
            }
            PositionedElement::MarkedContent { children, .. } => {
                realign_tab_aligned_text(children, changes)
            }
            _ => {}
        }
    }
}

/// The change in width of the field that starts where a mark starts, so a
/// field's own highlight or underline takes its new width.
fn own_width_change(
    changes: &[FieldWidthChange],
    x: f64,
    on_line: impl Fn(&&FieldWidthChange) -> bool,
) -> f64 {
    changes
        .iter()
        .filter(on_line)
        .find(|change| (change.x - x).abs() < 0.01)
        .map_or(0.0, |change| change.delta)
}

/// Lengthen or shorten a tab leader that ends where tab-aligned text starts,
/// so it still reaches the text once the text moves.
fn realign_tab_leader(run: &mut GlyphRun, changes: &[FieldWidthChange]) {
    let end = run.origin.x + run.advances.iter().sum::<f64>();
    // Leader glyphs step evenly, and the last one has no spacing after it,
    // so a leader stops short of its tab's end by less than two steps.
    let last = run.advances.last().copied().unwrap_or(0.0);
    let step = if run.advances.len() > 1 {
        run.advances[0]
    } else {
        last
    };
    if step <= 0.0 {
        return;
    }
    let Some(change) = changes.iter().find(|change| {
        (run.origin.y - change.baseline).abs() < 0.01
            && end < change.aligned.start + 0.01
            && change.aligned.start - end < 2.0 * step
    }) else {
        return;
    };
    let Some(&glyph) = run.glyph_ids.first() else {
        return;
    };
    let Some(ch) = run.text.chars().next() else {
        return;
    };
    if run.glyph_ids.iter().any(|id| *id != glyph)
        || run.text.chars().any(|other| other != ch)
        || run.text.chars().count() != run.glyph_ids.len()
    {
        return;
    }
    let Some(offset) = realigned_offset(changes, change.aligned.start, change.baseline) else {
        return;
    };
    let target = change.aligned.start + offset;
    let mut end = end;
    while end > target + 0.01 && run.glyph_ids.len() > 1 {
        run.glyph_ids.pop();
        run.advances.pop();
        run.text.pop();
        if let Some(previous) = run.advances.last_mut() {
            *previous = last;
        }
        end -= step;
    }
    while end + step <= target + 0.01 {
        if let Some(previous) = run.advances.last_mut() {
            *previous = step;
        }
        run.glyph_ids.push(glyph);
        run.advances.push(last);
        run.text.push(ch);
        end += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn substitute_field_values(
    painted: &mut PaintedElements,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
    fm: &mut FontManager,
    changes: &mut Vec<FieldWidthChange>,
    state: &mut FieldOccurrenceState,
) -> Result<()> {
    painted.policies().validate(painted.elements())?;
    let mut index = 0;
    while index < painted.len() {
        if let Some((recipe, _, placement)) = painted.policies().annotation(&[index]) {
            let Some(kind) = recipe.field_kind else {
                index += 1;
                continue;
            };
            if matches!(kind, FieldKind::Target(_) | FieldKind::SequenceContext(_)) {
                return Err(LayoutError::Layout(
                    "structural marker became a visible annotation".into(),
                ));
            }
            if state.consumed.contains(&recipe.occurrence) {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
                continue;
            }
            let (value, clear_kind) = late_field_value(
                kind,
                Some(recipe.occurrence.source),
                page_number,
                total_pages,
                section_values,
                field_sections,
                bookmark_pages,
                page_field_values,
                sequence,
                sequence_boundary,
            );
            if let Some(value) = value {
                let recipe = if clear_kind {
                    let mut changed = recipe.as_ref().clone();
                    changed.field_kind = None;
                    Arc::new(changed)
                } else {
                    recipe
                };
                let (mut replacement, diagnostics) =
                    render_annotation_replacement(Arc::clone(&recipe), &value, placement, fm)?;
                state.consume(recipe.occurrence)?;
                painted.splice_at(&[], index..index + 1, &mut replacement)?;
                state.diagnostics.extend(diagnostics);
            } else if clear_kind {
                painted.clear_annotation_kind(&[index])?;
            }
            index += 1;
            continue;
        }
        let element = &painted.elements()[index];
        let (children_len, group) = match element {
            PositionedElement::Group(group) => (Some(group.children.len()), true),
            PositionedElement::MarkedContent { children, .. } => (Some(children.len()), false),
            _ => (None, false),
        };
        if let Some(children_len) = children_len {
            let mut children =
                painted.splice_at(&[index], 0..children_len, &mut PaintedElements::default())?;
            if group {
                let mut local_changes = Vec::new();
                substitute_field_values(
                    &mut children,
                    page_number,
                    total_pages,
                    section_values,
                    field_sections,
                    bookmark_pages,
                    page_field_values,
                    sequence,
                    sequence_boundary,
                    fm,
                    &mut local_changes,
                    state,
                )?;
                if !local_changes.is_empty() {
                    realign_tab_aligned_text(children.coordinate_elements_mut(), &local_changes);
                }
            } else {
                substitute_field_values(
                    &mut children,
                    page_number,
                    total_pages,
                    section_values,
                    field_sections,
                    bookmark_pages,
                    page_field_values,
                    sequence,
                    sequence_boundary,
                    fm,
                    changes,
                    state,
                )?;
            }
            let empty = children.len() == 0;
            painted.splice_at(&[index], 0..0, &mut children)?;
            if empty && !group {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            } else {
                index += 1;
            }
            continue;
        }
        let (kind, source) = match element {
            PositionedElement::Text(run) => (run.field_kind, run.field_source),
            PositionedElement::MultilingualText(run) => (run.field_kind, run.field_source),
            _ => (None, None),
        };
        let Some(kind) = kind else {
            index += 1;
            continue;
        };
        let policy = painted.policies().required(&[index])?;
        let occurrence = painted.policies().field_occurrence(&[index])?;
        if matches!(kind, FieldKind::Target(_) | FieldKind::SequenceContext(_)) {
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let annotated_owner = occurrence.filter(|occurrence| state.annotated.contains(occurrence));
        if annotated_owner.is_some_and(|occurrence| state.consumed.contains(&occurrence)) {
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let (value, clear_kind) = late_field_value(
            kind,
            source,
            page_number,
            total_pages,
            section_values,
            field_sections,
            bookmark_pages,
            page_field_values,
            sequence,
            sequence_boundary,
        );
        let mut leaf = match element {
            PositionedElement::Text(run) => PositionedElement::Text(run.clone()),
            PositionedElement::MultilingualText(run) => {
                PositionedElement::MultilingualText(run.clone())
            }
            _ => unreachable!(),
        };
        if clear_kind {
            match &mut leaf {
                PositionedElement::Text(run) => run.field_kind = None,
                PositionedElement::MultilingualText(run) => run.field_kind = None,
                _ => unreachable!(),
            }
        }
        if let Some(value) = value {
            let mut replacement = field_replacement(&leaf, &value, policy, occurrence, fm)?;
            if let (PositionedElement::Text(old), [PositionedElement::Text(new)]) =
                (element, replacement.elements())
                && let Some(aligned) = old.tab_aligned
            {
                let delta = new.advances.iter().sum::<f64>() - old.advances.iter().sum::<f64>();
                if delta.abs() > 1e-9 {
                    changes.push(FieldWidthChange {
                        aligned,
                        baseline: old.origin.y,
                        font_size: old.font_size,
                        x: old.origin.x,
                        delta,
                    });
                }
            }
            if let Some(occurrence) = annotated_owner {
                state.consume(occurrence)?;
            }
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
        } else if clear_kind {
            let mut replacement = PaintedElements::default();
            let still_field = match &leaf {
                PositionedElement::Text(run) => {
                    run.field_kind.is_some() || run.field_source.is_some()
                }
                PositionedElement::MultilingualText(run) => {
                    run.field_kind.is_some() || run.field_source.is_some()
                }
                _ => false,
            };
            if still_field {
                replacement.push_field(
                    leaf,
                    policy.map_or(FieldPaintOwner::Neutral, |(kern, legacy)| {
                        FieldPaintOwner::Word {
                            kern,
                            legacy,
                            occurrence,
                        }
                    }),
                )?;
            } else {
                replacement.push_non_field(leaf)?;
            }
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
        }
        index += 1;
    }
    Ok(())
}

fn apply_multilingual_field_span(
    run: &mut oxoffice_layout::MultilingualGlyphRun,
    span: &oxoffice_layout::MultilingualTextSegment,
    logical_base: usize,
) {
    run.font_id = span.font_id();
    run.glyph_ids = span.glyph_ids().to_vec();
    run.x_advances = span.x_advances().to_vec();
    run.y_advances = span.y_advances().to_vec();
    run.x_offsets = span.x_offsets().to_vec();
    run.y_offsets = span.y_offsets().to_vec();
    run.clusters = span.clusters().to_vec();
    run.logical_text = span.text().to_owned();
    run.logical_index = logical_base + span.logical_index();
    run.script = span.script();
    run.language = span.language().map(str::to_owned);
    run.direction = span.direction();
    run.bidi_level = span.bidi_level();
}

pub(super) fn mark_remaining_artifacts(
    elements: &mut Vec<PositionedElement>,
    policies: &mut PaintPolicies,
) -> Result<()> {
    policies.validate(elements)?;
    let mut painted =
        PaintedElements::from_parts(std::mem::take(elements), std::mem::take(policies))?;
    for index in 0..painted.len() {
        if matches!(
            &painted.elements()[index],
            PositionedElement::MarkedContent { .. }
                | PositionedElement::LinkAnnotation { .. }
                | PositionedElement::FormField(_)
        ) {
            continue;
        }
        let child = painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
        painted.splice_at(
            &[],
            index..index,
            &mut child.wrap(PositionedElement::MarkedContent {
                structure: None,
                children: Vec::new(),
            })?,
        )?;
    }
    (*elements, *policies) = painted.into_parts()?;
    Ok(())
}
