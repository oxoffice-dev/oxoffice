//! Headers and footers: their content, their semantics and where they sit on a page.

use super::*;

/// Header/footer content already laid out as paragraph blocks.
pub struct HeaderFooterContent {
    pub header_blocks: Vec<ParagraphBlock>,
    pub footer_blocks: Vec<ParagraphBlock>,
    /// First-page header blocks (used when title_pg is true).
    pub first_header_blocks: Vec<ParagraphBlock>,
    /// First-page footer blocks (used when title_pg is true).
    pub first_footer_blocks: Vec<ParagraphBlock>,
    /// Even-page header blocks.
    pub even_header_blocks: Vec<ParagraphBlock>,
    /// Even-page footer blocks.
    pub even_footer_blocks: Vec<ParagraphBlock>,
    /// Whether Word selects the even header and footer variants.
    pub even_headers_active: bool,
    /// Default-header watermark, already positioned in page coordinates.
    pub watermark: Option<oxoffice_layout::GroupElement>,
    /// First-page-header watermark.
    pub first_watermark: Option<oxoffice_layout::GroupElement>,
    /// Even-page-header watermark.
    pub even_watermark: Option<oxoffice_layout::GroupElement>,
}

/// The header and footer variant Word selects for one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoryVariant {
    First,
    Even,
    Default,
}

impl HeaderFooterContent {
    pub(super) fn header(&self, variant: StoryVariant) -> &[ParagraphBlock] {
        match variant {
            StoryVariant::First => &self.first_header_blocks,
            StoryVariant::Even => &self.even_header_blocks,
            StoryVariant::Default => &self.header_blocks,
        }
    }

    pub(super) fn footer(&self, variant: StoryVariant) -> &[ParagraphBlock] {
        match variant {
            StoryVariant::First => &self.first_footer_blocks,
            StoryVariant::Even => &self.even_footer_blocks,
            StoryVariant::Default => &self.footer_blocks,
        }
    }

    pub(super) fn watermark(
        &self,
        variant: StoryVariant,
    ) -> Option<&oxoffice_layout::GroupElement> {
        match variant {
            StoryVariant::First => self.first_watermark.as_ref(),
            StoryVariant::Even => self.even_watermark.as_ref(),
            StoryVariant::Default => self.watermark.as_ref(),
        }
    }
}

/// Source-free placement policy beside the existing shaped story blocks.
#[derive(Debug, Clone, Default)]
pub(crate) enum HeaderFooterPlacement {
    #[default]
    Flow,
    Frame {
        properties: Box<oxoffice_docx_oxml::CT_FramePr>,
        paragraphs: usize,
    },
    FrameContinuation,
    TableRow {
        table: Arc<crate::table::TableBlock>,
        semantics: Arc<crate::block::TableSemantics>,
        row: usize,
    },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HeaderFooterBlockSemantics {
    pub paint: Arc<WordParagraphPaint>,
    pub direction: oxoffice_layout::TextDirection,
    pub border_gap: f64,
    pub flow_gap: f64,
    pub placement: HeaderFooterPlacement,
}

impl HeaderFooterBlockSemantics {
    fn is_flow(&self) -> bool {
        matches!(&self.placement, HeaderFooterPlacement::Flow)
            || matches!(&self.placement, HeaderFooterPlacement::TableRow { table, .. } if table.floating.is_none())
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>().saturating_add(
            self.paint
                .retained_bytes()
                .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
                .unwrap_or(usize::MAX),
        );
        if let HeaderFooterPlacement::Frame { properties, .. } = &self.placement {
            bytes = bytes.saturating_add(std::mem::size_of_val(properties.as_ref()));
            for value in [
                &properties.drop_cap,
                &properties.wrap,
                &properties.h_anchor,
                &properties.v_anchor,
                &properties.x_align,
                &properties.y_align,
                &properties.h_rule,
            ]
            .into_iter()
            .flatten()
            {
                bytes = bytes.saturating_add(value.capacity());
            }
            bytes = bytes.saturating_add(
                properties
                    .extra_attributes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(String, String)>()),
            );
            for (name, value) in &properties.extra_attributes {
                bytes = bytes
                    .saturating_add(name.capacity())
                    .saturating_add(value.capacity());
            }
        }
        // Rich table stories are excluded by the existing retained-cache gate.
        debug_assert!(!matches!(
            self.placement,
            HeaderFooterPlacement::TableRow { .. }
        ));
        bytes
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HeaderFooterSemantics {
    // Each private entry carries direction and the priced interior frame gap.
    pub header_directions: Vec<HeaderFooterBlockSemantics>,
    pub footer_directions: Vec<HeaderFooterBlockSemantics>,
    pub first_header_directions: Vec<HeaderFooterBlockSemantics>,
    pub first_footer_directions: Vec<HeaderFooterBlockSemantics>,
    pub even_header_directions: Vec<HeaderFooterBlockSemantics>,
    pub even_footer_directions: Vec<HeaderFooterBlockSemantics>,
}

impl HeaderFooterSemantics {
    pub(super) fn header(&self, variant: StoryVariant) -> &[HeaderFooterBlockSemantics] {
        match variant {
            StoryVariant::First => &self.first_header_directions,
            StoryVariant::Even => &self.even_header_directions,
            StoryVariant::Default => &self.header_directions,
        }
    }

    pub(super) fn footer(&self, variant: StoryVariant) -> &[HeaderFooterBlockSemantics] {
        match variant {
            StoryVariant::First => &self.first_footer_directions,
            StoryVariant::Even => &self.even_footer_directions,
            StoryVariant::Default => &self.footer_directions,
        }
    }
}

/// Height of a header or footer flow as `render_hf_blocks` stacks it.
///
/// Anchored drawings are positioned apart from the flow, so they add nothing.
fn story_flow_height(
    blocks: &[ParagraphBlock],
    directions: Option<&[HeaderFooterBlockSemantics]>,
) -> f64 {
    let mut height = 0.0;
    let mut after: f64 = 0.0;
    for (index, block) in blocks.iter().enumerate() {
        let facts = directions.and_then(|values| values.get(index));
        if facts.is_some_and(|facts| !facts.is_flow()) {
            continue;
        }
        height += facts.map_or_else(|| after.max(block.space_before), |facts| facts.flow_gap)
            + block.content_height();
        after = block.space_after;
    }
    height + after
}

impl PreparedFurniture {
    pub(super) fn block_height(&self, index: usize) -> f64 {
        self.blocks[index].content_height() + self.before_lines[index].iter().sum::<f64>()
    }

    fn state(&self) -> String {
        format!(
            "{:?}",
            (
                self.origin,
                self.height,
                &self.blocks,
                &self.paints,
                &self.before_lines,
                &self.starts,
                &self.placements,
                &self.wraps,
                &self.body_frame_wraps
            )
        )
    }
}

fn locate_furniture(
    blocks: Vec<ParagraphBlock>,
    before_lines: Vec<Vec<f64>>,
    paints: Option<Vec<Option<Arc<WordParagraphPaint>>>>,
    facts: Option<&[HeaderFooterBlockSemantics]>,
    geometry: &PageGeometry,
    origin: f64,
) -> PreparedFurniture {
    let paints = paints.unwrap_or_else(|| {
        (0..blocks.len())
            .map(|index| {
                facts
                    .and_then(|facts| facts.get(index))
                    .map(|facts| Arc::clone(&facts.paint))
            })
            .collect()
    });
    let mut starts = vec![origin; blocks.len()];
    let mut placements = vec![None; blocks.len()];
    let mut wraps = Vec::new();
    let mut body_frame_wraps = Vec::new();
    let mut floating_tables = Vec::new();
    let mut y = origin;
    let mut after: f64 = 0.0;
    for (index, paragraph) in blocks.iter().enumerate() {
        let entry = facts.and_then(|facts| facts.get(index));
        match entry.map(|entry| &entry.placement) {
            Some(HeaderFooterPlacement::Frame {
                properties,
                paragraphs,
            }) => {
                let end = index + paragraphs;
                let frame_facts = &facts.expect("frame facts accompany blocks")[index..end];
                let anchor_gap = facts
                    .and_then(|facts| {
                        facts[end..]
                            .iter()
                            .find(|facts| matches!(facts.placement, HeaderFooterPlacement::Flow))
                    })
                    .map_or(0.0, |facts| facts.flow_gap);
                let rect = frame_rect(
                    properties,
                    &blocks[index..end],
                    frame_facts,
                    geometry,
                    y + anchor_gap,
                );
                placements[index] = Some(rect);
                body_frame_wraps.push(PlacedWrap {
                    rect,
                    wrap: if matches!(properties.wrap.as_deref(), Some("none" | "notBeside")) {
                        WrapType::TopAndBottom
                    } else {
                        WrapType::Square
                    },
                    dist_top: properties.v_space.map_or(0.0, |value| value.to_pt()),
                    dist_bottom: properties.v_space.map_or(0.0, |value| value.to_pt()),
                    dist_left: properties.h_space.map_or(0.0, |value| value.to_pt()),
                    dist_right: properties.h_space.map_or(0.0, |value| value.to_pt()),
                });
                continue;
            }
            Some(HeaderFooterPlacement::FrameContinuation) => continue,
            Some(HeaderFooterPlacement::TableRow { table, row, .. }) => {
                let rect = if let Some(floating) = &table.floating {
                    let placed = if *row == 0 {
                        let mut placed = floating_wrap(
                            floating,
                            table.table_width,
                            table.content_height(),
                            geometry,
                            geometry,
                            y - geometry.margin_top,
                        );
                        settle_float(&mut placed, floating.overlap_allowed, &floating_tables);
                        floating_tables.push((placed, floating.overlap_allowed));
                        wraps.push(placed);
                        placed
                    } else {
                        // Later rows share the last physical table's settled
                        // whole rectangle. Drawings never enter this inventory.
                        floating_tables
                            .last()
                            .expect("floating furniture starts at row0")
                            .0
                    };
                    Rect {
                        y: placed.rect.y
                            + table.rows[..*row].iter().map(|row| row.height).sum::<f64>(),
                        height: table.rows[*row].height,
                        ..placed.rect
                    }
                } else {
                    y += entry.expect("table entry").flow_gap;
                    let rect = Rect {
                        x: geometry.margin_left + table.table_indent,
                        y,
                        width: table.table_width,
                        height: table.rows[*row].height,
                    };
                    y += rect.height;
                    after = 0.0;
                    rect
                };
                starts[index] = rect.y;
                placements[index] = Some(rect);
                continue;
            }
            Some(HeaderFooterPlacement::Flow) | None => {}
        }
        y += entry.map_or_else(|| after.max(paragraph.space_before), |entry| entry.flow_gap);
        starts[index] = y;
        for anchor in paragraph
            .anchored
            .iter()
            .filter(|anchor| anchor.wrap != WrapType::None)
        {
            wraps.push(PlacedWrap {
                rect: anchored_rect(
                    anchor,
                    geometry,
                    geometry,
                    y - geometry.margin_top,
                    paragraph.indent_left,
                ),
                wrap: anchor.wrap,
                dist_top: anchor.dist_top,
                dist_bottom: anchor.dist_bottom,
                dist_left: anchor.dist_left,
                dist_right: anchor.dist_right,
            });
        }
        y += paragraph.content_height() + before_lines[index].iter().sum::<f64>();
        after = paragraph.space_after;
    }
    PreparedFurniture {
        paints,
        blocks,
        before_lines,
        origin,
        height: y - origin + after,
        starts,
        placements,
        wraps,
        body_frame_wraps,
    }
}

pub(super) fn prepare_furniture(
    original: &[ParagraphBlock],
    facts: Option<&[HeaderFooterBlockSemantics]>,
    geometry: &PageGeometry,
    footer: bool,
    fm: &FontManager,
) -> Result<PreparedFurniture, String> {
    let origin = if footer {
        geometry.page_height - geometry.footer_distance - story_flow_height(original, facts)
    } else {
        geometry.header_distance
    };
    let mut previous = locate_furniture(
        original.to_vec(),
        vec![Vec::new(); original.len()],
        None,
        facts,
        geometry,
        origin,
    );
    // The legacy public interface carries no private source placement metadata.
    // Its existing positioned-furniture policy stays unchanged. Ordinary flow
    // spacing and clearance still use this same priced result.
    if facts.is_none() || previous.wraps.is_empty() {
        return Ok(previous);
    }
    let mut seen = vec![previous.state()];
    for _ in 0..32 {
        let origin = if footer {
            geometry.page_height - geometry.footer_distance - previous.height
        } else {
            geometry.header_distance
        };
        let obstacles = locate_furniture(
            previous.blocks.clone(),
            previous.before_lines.clone(),
            Some(previous.paints.clone()),
            facts,
            geometry,
            origin,
        )
        .wraps;
        let mut blocks = original.to_vec();
        let mut before_lines = vec![Vec::new(); original.len()];
        let mut paints = (0..original.len())
            .map(|index| {
                facts
                    .and_then(|facts| facts.get(index))
                    .map(|facts| Arc::clone(&facts.paint))
            })
            .collect::<Vec<_>>();
        let mut y = origin;
        let mut after: f64 = 0.0;
        for (index, source) in original.iter().enumerate() {
            let entry = facts.and_then(|facts| facts.get(index));
            if entry.is_some_and(|entry| !entry.is_flow()) {
                continue;
            }
            y += entry.map_or_else(|| after.max(source.space_before), |entry| entry.flow_gap);
            if entry.is_none_or(|entry| matches!(entry.placement, HeaderFooterPlacement::Flow))
                && let Some(reflowed) = reflow_around_wraps(
                    source,
                    (
                        entry.map_or(oxoffice_layout::TextDirection::Auto, |entry| {
                            entry.direction
                        }),
                        entry.map(|entry| entry.paint.as_ref()),
                    ),
                    &obstacles,
                    y - geometry.margin_top,
                    geometry,
                    fm,
                    WrapReflowPolicy::Furniture,
                )?
            {
                paints[index] = reflowed.paint.map(Arc::new);
                blocks[index] = reflowed.paragraph;
                before_lines[index] = reflowed.before_lines;
            }
            y += blocks[index].content_height() + before_lines[index].iter().sum::<f64>();
            after = source.space_after;
        }
        let current = locate_furniture(blocks, before_lines, Some(paints), facts, geometry, origin);
        let state = current.state();
        // This validates both the offered obstacles and the footer origin at
        // the actual resulting line/placement state, not just equal heights.
        if state == previous.state() {
            return Ok(current);
        }
        if seen.contains(&state) {
            return Err("positioned furniture preparation repeats a non-stable state".into());
        }
        seen.push(state);
        previous = current;
    }
    Err("positioned furniture preparation did not stabilize within 32 candidates".into())
}
