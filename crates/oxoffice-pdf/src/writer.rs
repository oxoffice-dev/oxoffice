//! PDF document writer: assembles pages, fonts, images, metadata, and outlines.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use oxoffice_layout::{
    Color, FillRule, FontId, FormFieldFragment, FormFieldId, GradientStop, GroupElement,
    LayoutResult, LineCap, LineJoin, Paint, Path, PathCommand, PathElement, PositionedElement,
    Stroke, Transform, walk,
};
use pdf_writer::types::{
    ActionType, AnnotationFlags, AnnotationType, CidFontType, ColorSpaceOperand, FontFlags,
    FunctionShadingType, LineCapStyle, LineJoinStyle, OutputIntentSubtype, SystemInfo, TabOrder,
    TextRenderingMode,
};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::font::{self, PreparedFont};
use crate::forms;
use crate::image;
use crate::structure::{PreparedStructure, has_content_stream_output};
use crate::{PdfConformance, conformance};

struct AlphaEntry {
    key: u32,
    alpha: f32,
    name: String,
    reference: Ref,
}

struct AlphaStates {
    entries: Vec<AlphaEntry>,
    by_key: HashMap<u32, usize>,
}

impl AlphaStates {
    fn new(pages: &[Arc<oxoffice_layout::PageFrame>], alloc: &mut impl FnMut() -> Ref) -> Self {
        let mut keys = BTreeSet::new();
        for page in pages {
            collect_page_alpha_keys(page, &mut keys);
        }

        let entries: Vec<AlphaEntry> = keys
            .into_iter()
            .enumerate()
            .map(|(index, key)| AlphaEntry {
                key,
                alpha: f32::from_bits(key),
                name: format!("GS{index}"),
                reference: alloc(),
            })
            .collect();
        let by_key = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.key, index))
            .collect();
        Self { entries, by_key }
    }

    fn entry(&self, alpha: f64) -> Option<&AlphaEntry> {
        let key = alpha_key(alpha)?;
        self.by_key.get(&key).map(|index| &self.entries[*index])
    }

    fn page_entries(&self, page: &oxoffice_layout::PageFrame) -> Vec<&AlphaEntry> {
        let mut keys = BTreeSet::new();
        collect_page_alpha_keys(page, &mut keys);
        keys.into_iter()
            .filter_map(|key| self.by_key.get(&key).map(|index| &self.entries[*index]))
            .collect()
    }
}

fn alpha_key(alpha: f64) -> Option<u32> {
    let normalized = if alpha.is_finite() {
        alpha.clamp(0.0, 1.0) as f32
    } else {
        1.0
    };
    let normalized = if normalized == 0.0 { 0.0 } else { normalized };
    (normalized < 1.0).then(|| normalized.to_bits())
}

fn collect_page_alpha_keys(page: &oxoffice_layout::PageFrame, keys: &mut BTreeSet<u32>) {
    if let Some(paint) = &page.background {
        collect_paint_alpha(paint, keys);
    }
    collect_alpha_keys(&page.elements, keys);
}

fn collect_alpha_keys(elements: &[PositionedElement], keys: &mut BTreeSet<u32>) {
    for element in elements {
        match element {
            PositionedElement::Text(run) if !(run.text.is_empty() && run.glyph_ids.is_empty()) => {
                insert_alpha(keys, run.color.a)
            }
            PositionedElement::Text(_) => {}
            PositionedElement::MultilingualText(run)
                if !(run.logical_text.is_empty() && run.glyph_ids.is_empty()) =>
            {
                insert_alpha(keys, run.color.a)
            }
            PositionedElement::MultilingualText(_) => {}
            PositionedElement::Line { color, .. } | PositionedElement::FilledRect { color, .. } => {
                insert_alpha(keys, color.a)
            }
            PositionedElement::Path(path) => {
                if let Some(paint) = &path.fill {
                    collect_paint_alpha(paint, keys);
                }
                if let Some(stroke) = &path.stroke {
                    collect_paint_alpha(&stroke.paint, keys);
                }
            }
            PositionedElement::Group(group) => {
                insert_alpha(keys, group.opacity);
                collect_alpha_keys(&group.children, keys);
            }
            PositionedElement::MarkedContent { children, .. } => {
                collect_alpha_keys(children, keys);
            }
            PositionedElement::FormField(fragment) => {
                collect_alpha_keys(&fragment.children, keys);
                collect_alpha_keys(&fragment.alternate, keys);
            }
            _ => {}
        }
    }
}

fn collect_paint_alpha(paint: &Paint, keys: &mut BTreeSet<u32>) {
    match paint {
        Paint::Solid(color) => insert_alpha(keys, color.a),
        Paint::Grid { colors, .. } => {
            for color in colors {
                insert_alpha(keys, color.a);
            }
        }
        _ => {}
    }
}

fn insert_alpha(keys: &mut BTreeSet<u32>, alpha: f64) {
    if let Some(key) = alpha_key(alpha) {
        keys.insert(key);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum GradientTarget {
    /// The page background, registered at leaf 0 of its page.
    Background,
    Fill,
    Stroke,
}

enum GradientGeometry {
    Linear([f32; 4]),
    Radial([f32; 6]),
    Grid {
        bounds: oxoffice_layout::Rect,
        columns: u16,
        rows: u16,
        colors: Vec<Color>,
    },
}

struct GradientEntry {
    page_idx: usize,
    name: String,
    pattern_ref: Ref,
    shading_ref: Ref,
    stitching_ref: Ref,
    interval_refs: Vec<Ref>,
    matrix: Transform,
    geometry: GradientGeometry,
    stops: Vec<GradientStop>,
    extend: [bool; 2],
}

struct GradientRegistry {
    entries: Vec<GradientEntry>,
    by_occurrence: HashMap<(usize, usize, GradientTarget), usize>,
}

impl GradientRegistry {
    fn new(pages: &[Arc<oxoffice_layout::PageFrame>], alloc: &mut impl FnMut() -> Ref) -> Self {
        let mut registry = Self {
            entries: Vec::new(),
            by_occurrence: HashMap::new(),
        };

        for (page_idx, page) in pages.iter().enumerate() {
            let mut leaf_idx = 0;
            let page_flip = Transform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: -1.0,
                e: 0.0,
                f: page.height,
            };
            if let Some(background) = &page.background {
                registry.collect(
                    background,
                    page_idx,
                    0,
                    GradientTarget::Background,
                    page_flip,
                    alloc,
                );
            }
            walk(&page.elements, &mut |element, transform| {
                let current_idx = leaf_idx;
                leaf_idx += 1;
                let PositionedElement::Path(path) = element else {
                    return;
                };
                if let Some(fill) = &path.fill {
                    registry.collect(
                        fill,
                        page_idx,
                        current_idx,
                        GradientTarget::Fill,
                        transform.then(page_flip),
                        alloc,
                    );
                }
                if let Some(stroke) = &path.stroke {
                    registry.collect(
                        &stroke.paint,
                        page_idx,
                        current_idx,
                        GradientTarget::Stroke,
                        transform.then(page_flip),
                        alloc,
                    );
                }
            });
        }

        registry
    }

    fn collect(
        &mut self,
        paint: &Paint,
        page_idx: usize,
        leaf_idx: usize,
        target: GradientTarget,
        matrix: Transform,
        alloc: &mut impl FnMut() -> Ref,
    ) {
        if let Paint::Grid {
            bounds,
            columns,
            rows,
            colors,
        } = paint
        {
            if Paint::grid(*bounds, *columns, *rows, colors.clone()).is_none() {
                return;
            }
            let index = self.entries.len();
            self.entries.push(GradientEntry {
                page_idx,
                name: format!("P{index}"),
                pattern_ref: alloc(),
                shading_ref: alloc(),
                stitching_ref: alloc(),
                interval_refs: Vec::new(),
                matrix,
                geometry: GradientGeometry::Grid {
                    bounds: *bounds,
                    columns: *columns,
                    rows: *rows,
                    colors: colors.clone(),
                },
                stops: Vec::new(),
                extend: [true, true],
            });
            self.by_occurrence
                .insert((page_idx, leaf_idx, target), index);
            return;
        }
        let (geometry, stops, extend) = match paint {
            Paint::Linear {
                start,
                end,
                stops,
                extend,
            } => (
                GradientGeometry::Linear([
                    start.x as f32,
                    start.y as f32,
                    end.x as f32,
                    end.y as f32,
                ]),
                stops,
                *extend,
            ),
            Paint::Radial {
                center,
                radius,
                focal,
                stops,
                extend,
            } => (
                GradientGeometry::Radial([
                    focal.x as f32,
                    focal.y as f32,
                    0.0,
                    center.x as f32,
                    center.y as f32,
                    finite_f32(*radius).max(0.0),
                ]),
                stops,
                *extend,
            ),
            Paint::Solid(_) | Paint::Tile { .. } | Paint::Grid { .. } => return,
        };

        let Some(stops) = normalize_gradient_stops(stops) else {
            return;
        };
        let stops = complete_gradient_domain(stops);
        let index = self.entries.len();
        let interval_refs = (1..stops.len()).map(|_| alloc()).collect();
        self.entries.push(GradientEntry {
            page_idx,
            name: format!("P{index}"),
            pattern_ref: alloc(),
            shading_ref: alloc(),
            stitching_ref: alloc(),
            interval_refs,
            matrix,
            geometry,
            stops,
            extend: [extend.0, extend.1],
        });
        self.by_occurrence
            .insert((page_idx, leaf_idx, target), index);
    }

    fn entry(
        &self,
        page_idx: usize,
        leaf_idx: usize,
        target: GradientTarget,
    ) -> Option<&GradientEntry> {
        self.by_occurrence
            .get(&(page_idx, leaf_idx, target))
            .map(|index| &self.entries[*index])
    }

    fn page_entries(&self, page_idx: usize) -> impl Iterator<Item = &GradientEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.page_idx == page_idx)
    }
}

fn normalize_gradient_stops(stops: &[GradientStop]) -> Option<Vec<GradientStop>> {
    let mut normalized: Vec<GradientStop> = stops
        .iter()
        .map(|stop| GradientStop {
            offset: normalized_offset(stop.offset),
            color: composite_stop_alpha(stop.color),
        })
        .collect();
    normalized.sort_by(|left, right| left.offset.total_cmp(&right.offset));

    let mut deduplicated: Vec<GradientStop> = Vec::with_capacity(normalized.len());
    for stop in normalized {
        if let Some(previous) = deduplicated.last_mut()
            && previous.offset == stop.offset
        {
            *previous = stop;
        } else {
            deduplicated.push(stop);
        }
    }

    match deduplicated.as_slice() {
        [] => None,
        [stop] => Some(vec![
            GradientStop {
                offset: 0.0,
                color: stop.color,
            },
            GradientStop {
                offset: 1.0,
                color: stop.color,
            },
        ]),
        _ => Some(deduplicated),
    }
}

fn complete_gradient_domain(mut stops: Vec<GradientStop>) -> Vec<GradientStop> {
    let first = stops[0];
    if first.offset > 0.0 {
        stops.insert(
            0,
            GradientStop {
                offset: 0.0,
                color: first.color,
            },
        );
    }

    let last = stops[stops.len() - 1];
    if last.offset < 1.0 {
        stops.push(GradientStop {
            offset: 1.0,
            color: last.color,
        });
    }
    stops
}

fn normalized_offset(offset: f64) -> f64 {
    if offset.is_nan() {
        0.0
    } else {
        offset.clamp(0.0, 1.0)
    }
}

fn composite_stop_alpha(color: Color) -> Color {
    let alpha = if color.a.is_finite() {
        color.a.clamp(0.0, 1.0)
    } else {
        1.0
    };
    Color {
        r: color.r * alpha + (1.0 - alpha),
        g: color.g * alpha + (1.0 - alpha),
        b: color.b * alpha + (1.0 - alpha),
        a: 1.0,
    }
}

fn finite_f32(value: f64) -> f32 {
    if value.is_finite() { value as f32 } else { 0.0 }
}

fn accessibility_xmp(title: &str) -> Vec<u8> {
    let title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/">
<dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
<pdfuaid:part>1</pdfuaid:part>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
    .into_bytes()
}

/// Write a complete PDF document from layout results.
pub(crate) fn write_pdf(layout: &LayoutResult) -> Vec<u8> {
    let Ok(prepared_fonts) = font::prepare_fonts(layout) else {
        return Vec::new();
    };
    write_pdf_impl(layout, None, prepared_fonts)
}

pub(crate) fn write_pdf_impl(
    layout: &LayoutResult,
    profile: Option<PdfConformance>,
    prepared_fonts: BTreeMap<FontId, PreparedFont>,
) -> Vec<u8> {
    let mut pdf = Pdf::new();

    // ── Reference ID allocation ──────────────────────────────────────
    let mut next_id = 1;
    let mut alloc = || {
        let r = Ref::new(next_id);
        next_id += 1;
        r
    };

    let catalog_id = alloc();
    let page_tree_id = alloc();
    let info_id = alloc();
    let outline_root_id = alloc();

    // Pre-allocate page IDs
    let page_ids: Vec<Ref> = layout.pages.iter().map(|_| alloc()).collect();
    let content_ids: Vec<Ref> = layout.pages.iter().map(|_| alloc()).collect();

    // ── Font preparation ─────────────────────────────────────────────
    // Preparation and failure checking completed before allocating output.
    // Preserve successful layout font order for stable existing PDF object IDs.
    let mut font_refs: BTreeMap<FontId, (Ref, Ref, Ref, Ref, Ref)> = BTreeMap::new();
    for source in &layout.fonts {
        if prepared_fonts.contains_key(&source.id) {
            font_refs.insert(source.id, (alloc(), alloc(), alloc(), alloc(), alloc()));
        }
    }

    // ── Image collection ─────────────────────────────────────────────
    // Collect all unique images across pages and allocate refs.
    struct ImageEntry {
        decoded: image::DecodedImage,
        xobject_ref: Ref,
        smask_ref: Option<Ref>,
    }

    let mut image_entries: Vec<ImageEntry> = Vec::new();
    // Map from (page_index, element_index) to image_entries index
    let mut image_map: HashMap<(usize, usize), usize> = HashMap::new();

    for (page_idx, page) in layout.pages.iter().enumerate() {
        let mut elem_idx = 0;
        walk(&page.elements, &mut |element, _| {
            if matches!(element, PositionedElement::Text(run) if run.text.is_empty() && run.glyph_ids.is_empty())
            {
                return;
            }
            let current_idx = elem_idx;
            elem_idx += 1;
            if let PositionedElement::Image {
                data, content_type, ..
            } = element
            {
                if data.is_empty() {
                    return;
                }
                if let Some(decoded) = image::decode_image(data, content_type) {
                    let xobject_ref = alloc();
                    let smask_ref = if decoded.alpha.is_some() {
                        Some(alloc())
                    } else {
                        None
                    };
                    let idx = image_entries.len();
                    image_entries.push(ImageEntry {
                        decoded,
                        xobject_ref,
                        smask_ref,
                    });
                    image_map.insert((page_idx, current_idx), idx);
                }
            }
        });
    }

    // ── Annotation refs ──────────────────────────────────────────────
    let mut annotation_refs: HashMap<(usize, usize), Ref> = HashMap::new();
    for (page_idx, page) in layout.pages.iter().enumerate() {
        let mut elem_idx = 0;
        walk(&page.elements, &mut |element, _| {
            let current_idx = elem_idx;
            elem_idx += 1;
            if let PositionedElement::LinkAnnotation { .. } = element {
                annotation_refs.insert((page_idx, current_idx), alloc());
            }
        });
    }

    let alpha_states = AlphaStates::new(&layout.pages, &mut alloc);
    let gradients = GradientRegistry::new(&layout.pages, &mut alloc);

    // ── Outline item refs ────────────────────────────────────────────
    let outline_item_ids: Vec<Ref> = layout.outlines.iter().map(|_| alloc()).collect();
    let tagged = layout
        .structure
        .as_ref()
        .and_then(|structure| PreparedStructure::new(structure, &layout.pages, &mut alloc));
    let declares_pdfua = tagged.is_some() && !layout_uses_notdef(layout);
    let metadata_id = (declares_pdfua || profile.is_some()).then(&mut alloc);
    let icc_profile_id = profile.map(|_| alloc());
    let forms = forms::FormPlan::new(
        &layout.pages,
        &layout.form_fields,
        tagged.is_some(),
        &mut alloc,
    );

    let language = layout
        .metadata
        .as_ref()
        .and_then(|meta| meta.language.as_deref())
        .filter(|language| oxoffice_layout::is_language_tag(language));

    // ── Write catalog ────────────────────────────────────────────────
    {
        let mut cat = pdf.catalog(catalog_id);
        cat.pages(page_tree_id);
        if !layout.outlines.is_empty() {
            cat.outlines(outline_root_id);
        }
        if let Some(tagged) = tagged.as_ref() {
            cat.pair(Name(b"StructTreeRoot"), tagged.root_ref);
            cat.mark_info().marked(true).suspects(false);
            cat.lang(TextStr(language.unwrap_or("und")));
            cat.viewer_preferences().display_doc_title(true);
            if let Some(metadata_id) = metadata_id {
                cat.metadata(metadata_id);
            }
        } else {
            if let Some(language) = language {
                cat.lang(TextStr(language));
            }
            if let Some(metadata_id) = metadata_id {
                cat.metadata(metadata_id);
            }
        }
        if let Some(forms) = forms.as_ref() {
            forms.write_catalog_entry(&mut cat);
        }
        if let Some(icc_profile_id) = icc_profile_id {
            let mut intents = cat.output_intents();
            let mut intent = intents.push();
            intent
                .subtype(OutputIntentSubtype::PDFA)
                .output_condition(TextStr("sRGB IEC61966-2.1"))
                .output_condition_identifier(TextStr("sRGB2014"))
                .registry_name(TextStr("https://registry.color.org"))
                .info(TextStr("sRGB2014"))
                .dest_output_profile(icc_profile_id);
            intent.finish();
            intents.finish();
        }
    }

    // ── Write page tree ──────────────────────────────────────────────
    {
        let mut pages = pdf.pages(page_tree_id);
        pages.count(layout.pages.len() as i32);
        pages.kids(page_ids.iter().copied());
    }

    // ── Write document info ──────────────────────────────────────────
    if let Some(meta) = &layout.metadata {
        let mut info = pdf.document_info(info_id);
        if let Some(title) = meta
            .title
            .as_deref()
            .or_else(|| (tagged.is_some() || profile.is_some()).then_some("Untitled document"))
        {
            info.title(TextStr(title));
        }
        if let Some(author) = &meta.author {
            info.author(TextStr(author));
        }
        if let Some(subject) = &meta.subject {
            info.subject(TextStr(subject));
        }
        if let Some(keywords) = &meta.keywords {
            info.keywords(TextStr(keywords));
        }
        if let Some(creator) = &meta.creator {
            info.creator(TextStr(creator));
        }
        info.producer(TextStr("oxoffice"));
    } else if tagged.is_some() || profile.is_some() {
        let mut info = pdf.document_info(info_id);
        info.title(TextStr("Untitled document"));
        info.producer(TextStr("oxoffice"));
    }

    if let Some(icc_profile_id) = icc_profile_id {
        let mut icc = pdf.icc_profile(icc_profile_id, conformance::SRGB2014);
        icc.n(3);
        icc.alternate().device_rgb();
        icc.finish();
    }

    // ── Write fonts ──────────────────────────────────────────────────
    for (font_id, prepared) in &prepared_fonts {
        let (type0_ref, cid_ref, descriptor_ref, stream_ref, cmap_ref) = font_refs[font_id];

        let source_face = font::instanced_face(&prepared.font_data)
            .expect("contributing font was prepared before PDF allocation");
        let mut base_name = sanitize_font_name(
            &prepared.font_data.family,
            source_face.is_bold(),
            source_face.is_italic(),
        );
        for (tag, value) in &prepared.font_data.variations {
            base_name.push('-');
            base_name.extend(
                tag.iter()
                    .copied()
                    .filter(u8::is_ascii_alphanumeric)
                    .map(char::from),
            );
            base_name.push_str(&value.to_string().replace('-', "m").replace('.', "p"));
        }
        if prepared.font_data.synthetic_bold {
            base_name.push_str("-SyntheticBold");
        }
        if prepared.font_data.synthetic_italic {
            base_name.push_str("-SyntheticItalic");
        }
        // A subset font's name starts with six capital letters and a plus
        // sign (ISO 32000-1, 9.6.4), here taken from the subset's digest.
        let base_name = format!("{}+{base_name}", subset_tag(&prepared.subset_bytes));

        // Type0 font (composite font)
        let mut type0 = pdf.type0_font(type0_ref);
        type0
            .base_font(Name(base_name.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid_ref)
            .to_unicode(cmap_ref);
        type0.finish();

        // CID font
        let mut cid = pdf.cid_font(cid_ref);
        cid.subtype(CidFontType::Type2);
        cid.base_font(Name(base_name.as_bytes()));
        cid.system_info(SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        });
        cid.font_descriptor(descriptor_ref);
        cid.default_width(0.0);
        cid.cid_to_gid_map_predefined(Name(b"Identity"));

        // Write widths
        if !prepared.widths.is_empty() {
            let mut widths = cid.widths();
            // Group consecutive glyph IDs for the W array
            let mut i = 0;
            while i < prepared.widths.len() {
                let start_gid = prepared.widths[i].0;
                let mut consecutive_widths = vec![prepared.widths[i].1 as f32];
                let mut j = i + 1;
                while j < prepared.widths.len()
                    && prepared.widths[j].0 == start_gid + (j - i) as u16
                {
                    consecutive_widths.push(prepared.widths[j].1 as f32);
                    j += 1;
                }
                widths.consecutive(start_gid, consecutive_widths.iter().copied());
                i = j;
            }
            widths.finish();
        }
        cid.finish();

        // Font descriptor
        if let Some(metrics) = font::get_font_metrics(&prepared.font_data) {
            let mut desc = pdf.font_descriptor(descriptor_ref);
            desc.name(Name(base_name.as_bytes()));

            let mut flags = FontFlags::empty();
            if source_face.is_italic()
                || prepared.font_data.synthetic_italic
                || prepared.font_data.variations.iter().any(|(tag, value)| {
                    (*tag == *b"ital" && *value != 0.0) || (*tag == *b"slnt" && *value != 0.0)
                })
            {
                flags |= FontFlags::ITALIC;
            }
            // Most text fonts are not fixed pitch and not symbolic
            flags |= FontFlags::NON_SYMBOLIC;
            desc.flags(flags);

            desc.bbox(Rect::new(
                metrics.bbox[0] as f32,
                metrics.bbox[1] as f32,
                metrics.bbox[2] as f32,
                metrics.bbox[3] as f32,
            ));
            desc.italic_angle(metrics.italic_angle as f32);
            desc.ascent(metrics.ascent as f32);
            desc.descent(metrics.descent as f32);
            desc.cap_height(metrics.cap_height as f32);
            desc.stem_v(metrics.stem_v as f32);
            desc.font_file2(stream_ref);
            desc.finish();
        }

        // Font stream (subset TrueType data)
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&prepared.subset_bytes, 6);
        let mut stream = pdf.stream(stream_ref, &compressed);
        stream.filter(Filter::FlateDecode);
        stream.pair(Name(b"Length1"), prepared.subset_bytes.len() as i32);
        stream.finish();

        // ToUnicode CMap stream
        let compressed_cmap = miniz_oxide::deflate::compress_to_vec_zlib(&prepared.cmap_bytes, 6);
        let mut cmap_stream = pdf.stream(cmap_ref, &compressed_cmap);
        cmap_stream.filter(Filter::FlateDecode);
        cmap_stream.finish();
    }

    // ── Write image XObjects ─────────────────────────────────────────
    for entry in &image_entries {
        let dec = &entry.decoded;

        if dec.is_jpeg {
            // JPEG pass-through
            let mut img = pdf.image_xobject(entry.xobject_ref, &dec.data);
            img.filter(Filter::DctDecode);
            img.width(dec.width as i32);
            img.height(dec.height as i32);
            set_color_space(&mut img, dec.color_space);
            img.bits_per_component(8);
            img.finish();
        } else {
            // Raw pixel data, compress with Deflate
            let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&dec.data, 6);
            let mut img = pdf.image_xobject(entry.xobject_ref, &compressed);
            img.filter(Filter::FlateDecode);
            img.width(dec.width as i32);
            img.height(dec.height as i32);
            set_color_space(&mut img, dec.color_space);
            img.bits_per_component(8);
            if let Some(smask_ref) = entry.smask_ref {
                img.s_mask(smask_ref);
            }
            img.finish();

            // Write soft mask (alpha channel) if present
            if let (Some(alpha), Some(smask_ref)) = (&dec.alpha, entry.smask_ref) {
                let compressed_alpha = miniz_oxide::deflate::compress_to_vec_zlib(alpha, 6);
                let mut mask = pdf.image_xobject(smask_ref, &compressed_alpha);
                mask.filter(Filter::FlateDecode);
                mask.width(dec.width as i32);
                mask.height(dec.height as i32);
                mask.color_space().device_gray();
                mask.bits_per_component(8);
                mask.finish();
            }
        }
    }

    for entry in &alpha_states.entries {
        let mut state = pdf.ext_graphics(entry.reference);
        state.stroking_alpha(entry.alpha);
        state.non_stroking_alpha(entry.alpha);
        state.finish();
    }

    write_gradient_objects(&mut pdf, &gradients, &alpha_states);

    // ── Write pages ──────────────────────────────────────────────────
    let image_xobjects: Vec<Ref> = image_entries
        .iter()
        .map(|entry| entry.xobject_ref)
        .collect();
    let resource_refs = PageResourceRefs {
        prepared_fonts: &prepared_fonts,
        font_refs: &font_refs,
        image_map: &image_map,
        image_xobjects: &image_xobjects,
        alpha_states: &alpha_states,
        gradients: &gradients,
    };
    let mut field_drawings = BTreeMap::new();
    for (page_idx, page) in layout.pages.iter().enumerate() {
        let page_id = page_ids[page_idx];
        let content_id = content_ids[page_idx];

        // Build content stream
        let content_bytes = build_page_content(
            page_idx,
            page,
            PageContentResources {
                prepared_fonts: &prepared_fonts,
                font_refs: &font_refs,
                image_map: &image_map,
                alpha_states: &alpha_states,
                gradients: &gradients,
                structure: tagged.as_ref(),
                forms: forms.as_ref(),
            },
            &mut field_drawings,
        );

        // Compress and write content stream
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&content_bytes, 6);
        let mut stream = pdf.stream(content_id, &compressed);
        stream.filter(Filter::FlateDecode);
        stream.finish();

        // Write page dictionary
        let mut page_dict = pdf.page(page_id);
        page_dict.parent(page_tree_id);
        page_dict.media_box(Rect::new(0.0, 0.0, page.width as f32, page.height as f32));
        page_dict.contents(content_id);
        if tagged.is_some() {
            page_dict.struct_parents(page_idx as i32);
        }

        // Resources: fonts + images
        let mut resources = page_dict.resources();
        write_page_resources(&mut resources, page_idx, page, &resource_refs);
        resources.finish();

        // Annotations (hyperlinks)
        let mut page_annotations = Vec::new();
        let mut elem_idx = 0;
        walk(&page.elements, &mut |element, _| {
            let current_idx = elem_idx;
            elem_idx += 1;
            if matches!(element, PositionedElement::LinkAnnotation { .. })
                && let Some(reference) = annotation_refs.get(&(page_idx, current_idx))
            {
                page_annotations.push(*reference);
            }
        });

        if let Some(forms) = forms.as_ref() {
            let before = page_annotations.len();
            page_annotations.extend(forms.widgets_on(page_idx));
            // Tagged pages tab through their fields in structure order.
            if tagged.is_some() && page_annotations.len() > before {
                page_dict.tab_order(TabOrder::StructureOrder);
            }
        }
        if !page_annotations.is_empty() {
            page_dict.annotations(page_annotations.iter().copied());
        }

        page_dict.finish();
    }

    // ── Write link annotations ───────────────────────────────────────
    for (page_idx, page) in layout.pages.iter().enumerate() {
        let mut elem_idx = 0;
        walk(&page.elements, &mut |element, transform| {
            let current_idx = elem_idx;
            elem_idx += 1;
            if let PositionedElement::LinkAnnotation { rect, url } = element
                && let Some(&annot_ref) = annotation_refs.get(&(page_idx, current_idx))
            {
                let rect = transform.transform_rect_bbox(*rect);
                let page_height = page.height;
                let mut annot = pdf.annotation(annot_ref);
                annot.subtype(AnnotationType::Link);
                annot.rect(Rect::new(
                    rect.x as f32,
                    (page_height - rect.y - rect.height) as f32,
                    (rect.x + rect.width) as f32,
                    (page_height - rect.y) as f32,
                ));
                annot.border(0.0, 0.0, 0.0, None);
                if profile.is_some() {
                    annot.flags(AnnotationFlags::PRINT);
                }
                annot
                    .action()
                    .action_type(ActionType::Uri)
                    .uri(Str(url.as_bytes()));
                annot.finish();
            }
        });
    }

    // ── Write outlines/bookmarks ─────────────────────────────────────
    if !layout.outlines.is_empty() {
        write_outlines(
            &mut pdf,
            outline_root_id,
            &layout.outlines,
            &outline_item_ids,
            &page_ids,
            layout,
        );
    }

    if let Some(forms) = forms.as_ref() {
        let page_heights: Vec<f64> = layout.pages.iter().map(|page| page.height).collect();
        forms.write(
            &mut pdf,
            &page_ids,
            &page_heights,
            &field_drawings,
            &mut |page_idx, resources| {
                write_page_resources(resources, page_idx, &layout.pages[page_idx], &resource_refs)
            },
        );
    }

    if let Some(tagged) = tagged.as_ref() {
        let widgets = forms
            .as_ref()
            .map(forms::FormPlan::tagged_widgets)
            .unwrap_or_default();
        tagged.write(&mut pdf, &page_ids, &widgets);
    }

    // The file identifier is a digest of everything written so far, so two
    // documents get different identifiers and one document always the same.
    // The metadata stream, which carries it, comes last.
    // The archival part goes in too: it is only written in the metadata.
    let file_id = file_identifier(pdf.as_bytes(), profile.map(PdfConformance::part));
    if let Some(metadata_id) = metadata_id {
        let xmp = if let Some(profile) = profile {
            conformance::archival_xmp(layout, profile, declares_pdfua, &file_id)
        } else {
            let title = layout
                .metadata
                .as_ref()
                .and_then(|meta| meta.title.as_deref())
                .unwrap_or("Untitled document");
            accessibility_xmp(title)
        };
        pdf.metadata(metadata_id, &xmp).finish();
    }
    pdf.set_file_id((file_id.to_vec(), file_id.to_vec()));
    pdf.finish()
}

/// The first 16 bytes of the SHA-256 digest of `bytes` and of the PDF/A
/// part, if any.
fn file_identifier(bytes: &[u8], archival_part: Option<u8>) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    if let Some(part) = archival_part {
        hasher.update(b"PDF/A-");
        hasher.update([part]);
    }
    let digest = hasher.finalize();
    let mut id = [0; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

fn layout_uses_notdef(layout: &LayoutResult) -> bool {
    layout.pages.iter().any(|page| {
        let mut found = false;
        walk(&page.elements, &mut |element, _| {
            if let PositionedElement::Text(run) = element
                && run.glyph_ids.contains(&0)
            {
                found = true;
            }
            if let PositionedElement::MultilingualText(run) = element
                && run.glyph_ids.contains(&0)
            {
                found = true;
            }
        });
        found
    })
}

fn write_gradient_objects(pdf: &mut Pdf, gradients: &GradientRegistry, alpha_states: &AlphaStates) {
    for entry in &gradients.entries {
        if let GradientGeometry::Grid {
            bounds,
            columns,
            rows,
            colors,
        } = &entry.geometry
        {
            let mut content = Content::new();
            let dx = bounds.width / f64::from(*columns);
            let dy = bounds.height / f64::from(*rows);
            // Merge only identical adjacent source samples, retaining every colour and alpha.
            for row in 0..usize::from(*rows) {
                let mut col = 0;
                while col < usize::from(*columns) {
                    let color = colors[row * usize::from(*columns) + col];
                    let mut end = col + 1;
                    while end < usize::from(*columns)
                        && colors[row * usize::from(*columns) + end] == color
                    {
                        end += 1;
                    }
                    content.save_state();
                    set_fill_color(&mut content, color, alpha_states);
                    content.rect(
                        (bounds.x + col as f64 * dx) as f32,
                        (bounds.y + row as f64 * dy) as f32,
                        ((end - col) as f64 * dx) as f32,
                        dy as f32,
                    );
                    content.fill_nonzero();
                    content.restore_state();
                    col = end;
                }
            }
            let bytes = content.finish();
            let mut pattern = pdf.tiling_pattern(entry.pattern_ref, &bytes);
            pattern.paint_type(pdf_writer::types::PaintType::Colored);
            pattern.tiling_type(pdf_writer::types::TilingType::ConstantSpacing);
            pattern.bbox(Rect::new(
                bounds.x as f32,
                bounds.y as f32,
                (bounds.x + bounds.width) as f32,
                (bounds.y + bounds.height) as f32,
            ));
            pattern.x_step(bounds.width as f32);
            pattern.y_step(bounds.height as f32);
            pattern.matrix([
                entry.matrix.a as f32,
                entry.matrix.b as f32,
                entry.matrix.c as f32,
                entry.matrix.d as f32,
                entry.matrix.e as f32,
                entry.matrix.f as f32,
            ]);
            let mut resources = pattern.resources();
            let mut states = resources.ext_g_states();
            for state in &alpha_states.entries {
                states.pair(Name(state.name.as_bytes()), state.reference);
            }
            states.finish();
            resources.finish();
            pattern.finish();
            continue;
        }
        let mut pattern = pdf.shading_pattern(entry.pattern_ref);
        pattern.shading_ref(entry.shading_ref);
        pattern.matrix([
            entry.matrix.a as f32,
            entry.matrix.b as f32,
            entry.matrix.c as f32,
            entry.matrix.d as f32,
            entry.matrix.e as f32,
            entry.matrix.f as f32,
        ]);
        pattern.finish();

        let mut shading = pdf.function_shading(entry.shading_ref);
        match &entry.geometry {
            GradientGeometry::Linear(coords) => {
                shading.shading_type(FunctionShadingType::Axial);
                shading.coords(*coords);
            }
            GradientGeometry::Radial(coords) => {
                shading.shading_type(FunctionShadingType::Radial);
                shading.coords(*coords);
            }
            GradientGeometry::Grid { .. } => unreachable!("handled above"),
        }
        shading.color_space().device_rgb();
        shading
            .insert(Name(b"Domain"))
            .array()
            .items([0.0_f32, 1.0_f32]);
        shading.extend(entry.extend);
        shading.function(entry.stitching_ref);
        shading.finish();

        let mut stitching = pdf.stitching_function(entry.stitching_ref);
        stitching.domain([0.0, 1.0]);
        stitching.functions(entry.interval_refs.iter().copied());
        stitching.bounds(
            entry.stops[1..entry.stops.len() - 1]
                .iter()
                .map(|stop| stop.offset as f32),
        );
        stitching.encode(entry.interval_refs.iter().flat_map(|_| [0.0_f32, 1.0_f32]));
        stitching.finish();

        for ((start, end), reference) in entry
            .stops
            .windows(2)
            .map(|pair| (&pair[0], &pair[1]))
            .zip(entry.interval_refs.iter())
        {
            let mut function = pdf.exponential_function(*reference);
            function.domain([0.0, 1.0]);
            function.c0([
                start.color.r as f32,
                start.color.g as f32,
                start.color.b as f32,
            ]);
            function.c1([end.color.r as f32, end.color.g as f32, end.color.b as f32]);
            function.n(1.0);
            function.finish();
        }
    }
}

struct PageResourceRefs<'a> {
    prepared_fonts: &'a BTreeMap<FontId, PreparedFont>,
    font_refs: &'a BTreeMap<FontId, (Ref, Ref, Ref, Ref, Ref)>,
    image_map: &'a HashMap<(usize, usize), usize>,
    image_xobjects: &'a [Ref],
    alpha_states: &'a AlphaStates,
    gradients: &'a GradientRegistry,
}

/// Write one page's resources. A form field's appearance drawn on the page
/// gets the same dictionary, so the names it draws with resolve alike.
fn write_page_resources(
    resources: &mut pdf_writer::writers::Resources<'_>,
    page_idx: usize,
    page: &oxoffice_layout::PageFrame,
    refs: &PageResourceRefs<'_>,
) {
    // Font resources
    if !refs.prepared_fonts.is_empty() {
        let mut font_dict = resources.fonts();
        for (font_id, (type0_ref, _, _, _, _)) in refs.font_refs {
            let font_name = format!("F{}", font_id.0);
            font_dict.pair(Name(font_name.as_bytes()), *type0_ref);
        }
        font_dict.finish();
    }

    // Image XObject resources
    // Sorted by element index. `image_map` is hashed, and its iteration
    // order reached the /XObject dictionary, so the same page named its
    // images in a different order on every run.
    let mut page_images: Vec<(usize, usize)> = refs
        .image_map
        .iter()
        .filter(|((pi, _), _)| *pi == page_idx)
        .map(|((_, ei), img_idx)| (*ei, *img_idx))
        .collect();
    page_images.sort_unstable();
    let page_image_names: Vec<(String, Ref)> = page_images
        .into_iter()
        .map(|(ei, img_idx)| {
            (
                format!("Im{}_{}", page_idx, ei),
                refs.image_xobjects[img_idx],
            )
        })
        .collect();

    if !page_image_names.is_empty() {
        let mut xobjects = resources.x_objects();
        for (name, xobj_ref) in &page_image_names {
            xobjects.pair(Name(name.as_bytes()), *xobj_ref);
        }
        xobjects.finish();
    }

    let page_alpha_states = refs.alpha_states.page_entries(page);
    if !page_alpha_states.is_empty() {
        let mut states = resources.ext_g_states();
        for entry in page_alpha_states {
            states.pair(Name(entry.name.as_bytes()), entry.reference);
        }
        states.finish();
    }

    let page_gradients: Vec<_> = refs.gradients.page_entries(page_idx).collect();
    if !page_gradients.is_empty() {
        let mut patterns = resources.patterns();
        for entry in page_gradients {
            patterns.pair(Name(entry.name.as_bytes()), entry.pattern_ref);
        }
        patterns.finish();
    }
}

/// Build the content stream for a single page.
#[derive(Clone, Copy)]
struct PageContentResources<'a> {
    prepared_fonts: &'a BTreeMap<FontId, PreparedFont>,
    font_refs: &'a BTreeMap<FontId, (Ref, Ref, Ref, Ref, Ref)>,
    image_map: &'a HashMap<(usize, usize), usize>,
    alpha_states: &'a AlphaStates,
    gradients: &'a GradientRegistry,
    structure: Option<&'a PreparedStructure<'a>>,
    forms: Option<&'a forms::FormPlan<'a>>,
}

fn build_page_content<'a>(
    page_idx: usize,
    page: &'a oxoffice_layout::PageFrame,
    resources: PageContentResources<'a>,
    field_drawings: &'a mut BTreeMap<FormFieldId, forms::FieldDrawing>,
) -> Vec<u8> {
    let mut content = Content::new();
    let page_height = page.height as f32;
    content.save_state();
    content.transform([1.0, 0.0, 0.0, -1.0, 0.0, page_height]);
    emit_background(&mut content, page_idx, page, resources);

    let mut state = EmitState {
        page_idx,
        page_height: page.height,
        prepared_fonts: resources.prepared_fonts,
        font_refs: resources.font_refs,
        image_map: resources.image_map,
        alpha_states: resources.alpha_states,
        gradients: resources.gradients,
        leaf_index: 0,
        structure: resources.structure,
        next_mcid: 0,
        forms: resources.forms,
        field_drawings,
        groups: Vec::new(),
        open_marks: 0,
    };
    emit_elements(
        &mut content,
        &page.elements,
        &mut state,
        Transform::IDENTITY,
        None,
    );

    content.restore_state();
    content.finish().to_vec()
}

/// Paint the page background under every element, as the rasteriser does.
fn emit_background(
    content: &mut Content,
    page_idx: usize,
    page: &oxoffice_layout::PageFrame,
    resources: PageContentResources<'_>,
) {
    // Tile paint resolves to nothing, exactly as it does for a path fill.
    let Some(paint) = page.background.as_ref().and_then(|background| {
        resolve_paint(
            background,
            page_idx,
            0,
            GradientTarget::Background,
            resources.gradients,
        )
    }) else {
        return;
    };
    // A tagged page marks this decorative paint as an artifact.
    let tagged = resources.structure.is_some();
    if tagged {
        content.begin_marked_content(Name(b"Artifact"));
    }
    content.save_state();
    set_fill_paint(content, paint, resources.alpha_states);
    content.rect(0.0, 0.0, page.width as f32, page.height as f32);
    content.fill_nonzero();
    content.restore_state();
    if tagged {
        content.end_marked_content();
    }
}

struct EmitState<'a> {
    page_idx: usize,
    page_height: f64,
    prepared_fonts: &'a BTreeMap<FontId, PreparedFont>,
    font_refs: &'a BTreeMap<FontId, (Ref, Ref, Ref, Ref, Ref)>,
    image_map: &'a HashMap<(usize, usize), usize>,
    alpha_states: &'a AlphaStates,
    gradients: &'a GradientRegistry,
    leaf_index: usize,
    structure: Option<&'a PreparedStructure<'a>>,
    next_mcid: i32,
    forms: Option<&'a forms::FormPlan<'a>>,
    /// What the page draws for each placed form field, kept out of the page.
    field_drawings: &'a mut BTreeMap<FormFieldId, forms::FieldDrawing>,
    /// The groups around the element being drawn, outermost first.
    groups: Vec<&'a GroupElement>,
    /// Marked content sequences open around the element being drawn.
    open_marks: usize,
}

fn emit_elements<'a>(
    content: &mut Content,
    elements: &'a [PositionedElement],
    state: &mut EmitState<'a>,
    accumulated_transform: Transform,
    inherited_actual_text: Option<&str>,
) {
    for element in elements {
        let actual_text = inherited_actual_text.or_else(|| {
            let run = match element {
                PositionedElement::MultilingualText(run) => Some(run),
                PositionedElement::MarkedContent { children, .. } if children.len() == 1 => {
                    match &children[0] {
                        PositionedElement::MultilingualText(run) => Some(run),
                        _ => None,
                    }
                }
                _ => None,
            }?;
            run.is_valid().then_some(run.logical_text.as_str())
        });
        if let PositionedElement::FormField(fragment) = element {
            emit_form_field(content, fragment, state, accumulated_transform, actual_text);
            continue;
        }
        if let PositionedElement::MarkedContent {
            structure,
            children,
        } = element
        {
            if state.structure.is_none() && structure.is_some() {
                emit_elements(content, children, state, accumulated_transform, actual_text);
                continue;
            }
            if !has_content_stream_output(children) {
                emit_elements(content, children, state, accumulated_transform, actual_text);
                continue;
            }
            match structure {
                Some(id) => {
                    let tag = state
                        .structure
                        .and_then(|prepared| prepared.role_name(*id))
                        .unwrap_or(b"Span");
                    content
                        .begin_marked_content_with_properties(Name(tag))
                        .properties()
                        .identify(state.next_mcid);
                    state.next_mcid += 1;
                }
                None => {
                    content.begin_marked_content(Name(b"Artifact"));
                }
            }
            state.open_marks += 1;
            emit_elements(content, children, state, accumulated_transform, actual_text);
            state.open_marks -= 1;
            content.end_marked_content();
            continue;
        }
        let elem_idx = state.leaf_index;
        if !matches!(element, PositionedElement::Group(_))
            && !matches!(element, PositionedElement::Text(run) if run.text.is_empty() && run.glyph_ids.is_empty())
            && !matches!(element, PositionedElement::MultilingualText(run) if run.logical_text.is_empty() && run.glyph_ids.is_empty())
        {
            state.leaf_index += 1;
        }
        match element {
            PositionedElement::Text(run) if !(run.text.is_empty() && run.glyph_ids.is_empty()) => {
                if let Some(prepared) = state.prepared_fonts.get(&run.font_id)
                    && state.font_refs.contains_key(&run.font_id)
                {
                    let font_name = format!("F{}", run.font_id.0);

                    content.save_state();

                    // Set text color
                    content.set_fill_rgb(
                        run.color.r as f32,
                        run.color.g as f32,
                        run.color.b as f32,
                    );
                    apply_alpha(content, run.color.a, state.alpha_states);

                    apply_text_synthesis(
                        content,
                        &prepared.font_data,
                        run.font_size,
                        run.color,
                        run.origin.y,
                    );
                    content.begin_text();
                    content.set_font(Name(font_name.as_bytes()), run.font_size as f32);

                    // Cancel the page flip so glyphs remain upright.
                    content.set_text_matrix([
                        1.0,
                        0.0,
                        0.0,
                        -1.0,
                        run.origin.x as f32,
                        run.origin.y as f32,
                    ]);

                    // Remap glyph IDs and emit with TJ operator
                    emit_glyphs(
                        content,
                        &run.glyph_ids,
                        &run.advances,
                        run.font_size,
                        &prepared.remapper,
                        &prepared.widths,
                    );

                    content.end_text();
                    content.restore_state();
                }
            }
            PositionedElement::Text(_) => {}
            PositionedElement::MultilingualText(run) => {
                if let Some(prepared) = state.prepared_fonts.get(&run.font_id)
                    && state.font_refs.contains_key(&run.font_id)
                {
                    let font_name = format!("F{}", run.font_id.0);
                    content.save_state();
                    let page_flip = Transform {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: -1.0,
                        e: 0.0,
                        f: state.page_height,
                    };
                    let current_pdf_transform = page_flip.then(accumulated_transform);
                    let text_matrix = Transform {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: -1.0,
                        e: run.origin.x,
                        f: run.origin.y,
                    };
                    // Give ActualText page-oriented geometry, then restore the
                    // current transform before painting the unchanged run.
                    let corrected_actual_text =
                        if let Some(current_inverse) = inverse_transform(current_pdf_transform) {
                            content.transform(transform_array(current_inverse));
                            true
                        } else {
                            false
                        };
                    content
                        .begin_marked_content_with_properties(Name(b"Span"))
                        .properties()
                        .actual_text(TextStr(actual_text.unwrap_or(&run.logical_text)));
                    if corrected_actual_text {
                        content.transform(transform_array(current_pdf_transform));
                    }
                    content.set_fill_rgb(
                        run.color.r as f32,
                        run.color.g as f32,
                        run.color.b as f32,
                    );
                    apply_alpha(content, run.color.a, state.alpha_states);
                    apply_text_synthesis(
                        content,
                        &prepared.font_data,
                        run.font_size,
                        run.color,
                        run.origin.y,
                    );
                    content.begin_text();
                    content.set_font(Name(font_name.as_bytes()), run.font_size as f32);
                    emit_multilingual_glyphs(
                        content,
                        run,
                        transform_array(text_matrix),
                        &prepared.remapper,
                        &prepared.widths,
                    );
                    content.end_text();
                    content.end_marked_content();
                    content.restore_state();
                }
            }
            PositionedElement::Line {
                start,
                end,
                width,
                color,
                dash_pattern,
            } => {
                content.save_state();
                content.set_stroke_rgb(color.r as f32, color.g as f32, color.b as f32);
                apply_alpha(content, color.a, state.alpha_states);
                content.set_line_width(*width as f32);
                if let Some((on, off)) = dash_pattern {
                    content.set_dash_pattern([*on as f32, *off as f32], 0.0);
                }
                content.move_to(start.x as f32, start.y as f32);
                content.line_to(end.x as f32, end.y as f32);
                content.stroke();
                content.restore_state();
            }
            PositionedElement::FilledRect { rect, color } => {
                content.save_state();
                content.set_fill_rgb(color.r as f32, color.g as f32, color.b as f32);
                apply_alpha(content, color.a, state.alpha_states);
                content.rect(
                    rect.x as f32,
                    rect.y as f32,
                    rect.width as f32,
                    rect.height as f32,
                );
                content.fill_nonzero();
                content.restore_state();
            }
            PositionedElement::Image { rect, data, .. } => {
                if !data.is_empty() && state.image_map.contains_key(&(state.page_idx, elem_idx)) {
                    let img_name = format!("Im{}_{}", state.page_idx, elem_idx);

                    content.save_state();
                    // Cancel the page flip while preserving the image rectangle.
                    content.transform([
                        rect.width as f32,
                        0.0,
                        0.0,
                        -rect.height as f32,
                        rect.x as f32,
                        (rect.y + rect.height) as f32,
                    ]);
                    content.x_object(Name(img_name.as_bytes()));
                    content.restore_state();
                }
            }
            PositionedElement::LinkAnnotation { .. } => {
                // Link annotations are written separately, not in the content stream
            }
            PositionedElement::Path(path) => {
                emit_path(
                    content,
                    path,
                    state.page_idx,
                    elem_idx,
                    state.alpha_states,
                    state.gradients,
                );
            }
            PositionedElement::Group(group) => {
                content.save_state();
                apply_group_state(content, group, state.alpha_states);
                state.groups.push(group);
                emit_elements(
                    content,
                    &group.children,
                    state,
                    group.transform.then(accumulated_transform),
                    actual_text,
                );
                state.groups.pop();
                content.restore_state();
            }
            _ => {
                // Later oxoffice-layout elements remain unsupported until assigned.
            }
        }
    }
}

/// Draw a form field fragment's children into its field's appearance, or as
/// page content when the field has no widget on this page. Either way the
/// children carry no structure marks: the structure does not count them.
fn emit_form_field<'a>(
    content: &mut Content,
    fragment: &'a FormFieldFragment,
    state: &mut EmitState<'a>,
    accumulated_transform: Transform,
    actual_text: Option<&str>,
) {
    debug_assert!(
        fragment.mark.is_none(),
        "a producer's form marker reached the writer"
    );
    let structure = state.structure.take();
    let placed = state
        .forms
        .and_then(|forms| forms.placed_on(fragment.field, state.page_idx));
    match placed {
        None => {
            // Tagged content outside any marked sequence must be an artifact.
            let artifact = structure.is_some() && state.open_marks == 0;
            if artifact {
                content.begin_marked_content(Name(b"Artifact"));
            }
            emit_elements(
                content,
                &fragment.children,
                state,
                accumulated_transform,
                actual_text,
            );
            if artifact {
                content.end_marked_content();
            }
        }
        Some(placed) => {
            let current = field_drawing(
                &fragment.children,
                state,
                accumulated_transform,
                actual_text,
            );
            let other = if placed.is_check_box() {
                // The other state holds text only, drawn apart so that it
                // does not move the leaf count naming the page's images and
                // gradients.
                let no_images = HashMap::new();
                let mut no_fields = BTreeMap::new();
                let mut other_state = EmitState {
                    page_idx: state.page_idx,
                    page_height: state.page_height,
                    prepared_fonts: state.prepared_fonts,
                    font_refs: state.font_refs,
                    image_map: &no_images,
                    alpha_states: state.alpha_states,
                    gradients: state.gradients,
                    leaf_index: 0,
                    structure: None,
                    next_mcid: 0,
                    forms: None,
                    field_drawings: &mut no_fields,
                    groups: state.groups.clone(),
                    open_marks: 0,
                };
                let text: Vec<PositionedElement> = fragment
                    .alternate
                    .iter()
                    .filter(|element| {
                        matches!(
                            element,
                            PositionedElement::Text(_) | PositionedElement::MultilingualText(_)
                        )
                    })
                    .cloned()
                    .collect();
                field_drawing(&text, &mut other_state, accumulated_transform, actual_text)
            } else {
                Vec::new()
            };
            let drawing = state.field_drawings.entry(fragment.field).or_default();
            append_drawing(&mut drawing.current, &current);
            append_drawing(&mut drawing.other, &other);
        }
    }
    state.structure = structure;
}

/// Append one fragment's drawing to its field's appearance. A content
/// stream ends without whitespace, so the next fragment's `q` would fuse
/// with the previous `Q` into an unknown operator.
fn append_drawing(appearance: &mut Vec<u8>, drawing: &[u8]) {
    if !appearance.is_empty() && !drawing.is_empty() {
        appearance.push(b'\n');
    }
    appearance.extend_from_slice(drawing);
}

/// Children drawn in page coordinates under the groups around them, as an
/// appearance stream draws them.
fn field_drawing<'a>(
    children: &'a [PositionedElement],
    state: &mut EmitState<'a>,
    accumulated_transform: Transform,
    actual_text: Option<&str>,
) -> Vec<u8> {
    let mut drawing = Content::new();
    drawing.save_state();
    drawing.transform([1.0, 0.0, 0.0, -1.0, 0.0, state.page_height as f32]);
    for group in &state.groups {
        apply_group_state(&mut drawing, group, state.alpha_states);
    }
    emit_elements(
        &mut drawing,
        children,
        state,
        accumulated_transform,
        actual_text,
    );
    drawing.restore_state();
    drawing.finish().to_vec()
}

/// A group's transform, clip and opacity, as its children are drawn under.
fn apply_group_state(content: &mut Content, group: &GroupElement, alpha_states: &AlphaStates) {
    content.transform([
        group.transform.a as f32,
        group.transform.b as f32,
        group.transform.c as f32,
        group.transform.d as f32,
        group.transform.e as f32,
        group.transform.f as f32,
    ]);
    if let Some(clip) = &group.clip {
        emit_path_geometry(content, clip);
        match clip.fill_rule {
            FillRule::NonZero => content.clip_nonzero(),
            FillRule::EvenOdd => content.clip_even_odd(),
        };
        content.end_path();
    }
    apply_alpha(content, group.opacity, alpha_states);
}

fn apply_alpha(content: &mut Content, alpha: f64, states: &AlphaStates) {
    if let Some(entry) = states.entry(alpha) {
        content.set_parameters(Name(entry.name.as_bytes()));
    }
}

enum ResolvedPaint<'a> {
    Solid(Color),
    Gradient(&'a GradientEntry),
}

fn emit_path(
    content: &mut Content,
    element: &PathElement,
    page_idx: usize,
    leaf_idx: usize,
    alpha_states: &AlphaStates,
    gradients: &GradientRegistry,
) {
    let fill = element.fill.as_ref().and_then(|paint| {
        resolve_paint(paint, page_idx, leaf_idx, GradientTarget::Fill, gradients)
    });
    let stroke = element.stroke.as_ref().and_then(|stroke| {
        resolve_paint(
            &stroke.paint,
            page_idx,
            leaf_idx,
            GradientTarget::Stroke,
            gradients,
        )
        .map(|paint| (stroke, paint))
    });

    if fill.is_none() && stroke.is_none() {
        return;
    }

    content.save_state();

    match (fill, stroke) {
        (Some(ResolvedPaint::Solid(fill)), Some((stroke, ResolvedPaint::Solid(stroke_color))))
            if alpha_key(fill.a) == alpha_key(stroke_color.a) =>
        {
            set_fill_color(content, fill, alpha_states);
            emit_stroke_style(content, stroke, stroke_color);
            emit_path_geometry(content, &element.path);
            match element.path.fill_rule {
                FillRule::NonZero => content.fill_nonzero_and_stroke(),
                FillRule::EvenOdd => content.fill_even_odd_and_stroke(),
            };
        }
        (Some(fill), Some((stroke, stroke_paint))) => {
            content.save_state();
            set_fill_paint(content, fill, alpha_states);
            emit_path_geometry(content, &element.path);
            match element.path.fill_rule {
                FillRule::NonZero => content.fill_nonzero(),
                FillRule::EvenOdd => content.fill_even_odd(),
            };
            content.restore_state();
            content.save_state();
            emit_stroke_paint(content, stroke, stroke_paint, alpha_states);
            emit_path_geometry(content, &element.path);
            content.stroke();
            content.restore_state();
        }
        (Some(fill), None) => {
            set_fill_paint(content, fill, alpha_states);
            emit_path_geometry(content, &element.path);
            match element.path.fill_rule {
                FillRule::NonZero => content.fill_nonzero(),
                FillRule::EvenOdd => content.fill_even_odd(),
            };
        }
        (None, Some((stroke, stroke_paint))) => {
            emit_stroke_paint(content, stroke, stroke_paint, alpha_states);
            emit_path_geometry(content, &element.path);
            content.stroke();
        }
        (None, None) => {}
    }

    content.restore_state();
}

fn resolve_paint<'a>(
    paint: &Paint,
    page_idx: usize,
    leaf_idx: usize,
    target: GradientTarget,
    gradients: &'a GradientRegistry,
) -> Option<ResolvedPaint<'a>> {
    match paint {
        Paint::Solid(color) => Some(ResolvedPaint::Solid(*color)),
        Paint::Linear { .. } | Paint::Radial { .. } | Paint::Grid { .. } => gradients
            .entry(page_idx, leaf_idx, target)
            .map(ResolvedPaint::Gradient),
        Paint::Tile { .. } => None,
    }
}

fn set_fill_paint(content: &mut Content, paint: ResolvedPaint<'_>, alpha_states: &AlphaStates) {
    match paint {
        ResolvedPaint::Solid(color) => set_fill_color(content, color, alpha_states),
        ResolvedPaint::Gradient(entry) => {
            content.set_fill_color_space(ColorSpaceOperand::Pattern);
            content.set_fill_pattern([], Name(entry.name.as_bytes()));
        }
    }
}

fn emit_stroke_paint(
    content: &mut Content,
    stroke: &Stroke,
    paint: ResolvedPaint<'_>,
    alpha_states: &AlphaStates,
) {
    match paint {
        ResolvedPaint::Solid(color) => {
            emit_stroke_style(content, stroke, color);
            apply_alpha(content, color.a, alpha_states);
        }
        ResolvedPaint::Gradient(entry) => {
            emit_stroke_geometry_style(content, stroke);
            content.set_stroke_color_space(ColorSpaceOperand::Pattern);
            content.set_stroke_pattern([], Name(entry.name.as_bytes()));
        }
    }
}

fn set_fill_color(
    content: &mut Content,
    color: oxoffice_layout::Color,
    alpha_states: &AlphaStates,
) {
    content.set_fill_rgb(color.r as f32, color.g as f32, color.b as f32);
    apply_alpha(content, color.a, alpha_states);
}

fn emit_stroke_style(content: &mut Content, stroke: &Stroke, color: oxoffice_layout::Color) {
    emit_stroke_geometry_style(content, stroke);
    content.set_stroke_rgb(color.r as f32, color.g as f32, color.b as f32);
}

fn emit_stroke_geometry_style(content: &mut Content, stroke: &Stroke) {
    let width = if stroke.width.is_finite() {
        stroke.width.max(0.0) as f32
    } else {
        0.0
    };

    content.set_line_width(width);
    content.set_line_cap(match stroke.cap {
        LineCap::Butt => LineCapStyle::ButtCap,
        LineCap::Round => LineCapStyle::RoundCap,
        LineCap::Square => LineCapStyle::ProjectingSquareCap,
    });
    content.set_line_join(match stroke.join {
        LineJoin::Miter => LineJoinStyle::MiterJoin,
        LineJoin::Round => LineJoinStyle::RoundJoin,
        LineJoin::Bevel => LineJoinStyle::BevelJoin,
    });
    content.set_miter_limit(10.0);
    if let Some(dash) = &stroke.dash {
        content.set_dash_pattern(dash.iter().map(|value| *value as f32), 0.0);
    }
}

fn emit_path_geometry(content: &mut Content, path: &Path) {
    for command in &path.commands {
        match command {
            PathCommand::MoveTo(point) => {
                content.move_to(point.x as f32, point.y as f32);
            }
            PathCommand::LineTo(point) => {
                content.line_to(point.x as f32, point.y as f32);
            }
            PathCommand::CurveTo { c1, c2, to } => {
                content.cubic_to(
                    c1.x as f32,
                    c1.y as f32,
                    c2.x as f32,
                    c2.y as f32,
                    to.x as f32,
                    to.y as f32,
                );
            }
            PathCommand::Close => {
                content.close_path();
            }
        }
    }
}

/// Emit glyph IDs using the TJ operator with per-glyph positioning.
///
/// The TJ operator alternates between glyph strings and numeric adjustments.
/// After showing a glyph, PDF auto-advances by the glyph's declared width
/// (from the CID font's W table, in thousandths of text space).
/// The adjustment value is *subtracted* from the current position.
///
/// To achieve the shaped advance: adjustment = declared_width - (advance_pt / font_size * 1000)
fn emit_glyphs(
    content: &mut Content,
    glyph_ids: &[u16],
    advances: &[f64],
    font_size: f64,
    remapper: &subsetter::GlyphRemapper,
    widths: &[(u16, f64)],
) {
    if glyph_ids.is_empty() {
        return;
    }

    // Build a lookup from new_gid → declared width (in 1000ths of em)
    let width_map: HashMap<u16, f64> = widths.iter().copied().collect();

    let mut positioned = content.show_positioned();
    let mut items = positioned.items();

    for (i, &gid) in glyph_ids.iter().enumerate() {
        let new_gid = remapper.get(gid).unwrap_or(0);

        // Encode glyph ID as 2-byte big-endian
        let bytes = new_gid.to_be_bytes();
        items.show(Str(&bytes));

        // Calculate adjustment for next glyph
        if i + 1 < glyph_ids.len() {
            let advance_pt = advances[i];
            // declared_width is the width PDF will auto-advance by (in 1000ths of em)
            // default_width is 0.0, so glyphs not in W table get no auto-advance
            let declared_width = width_map.get(&new_gid).copied().unwrap_or(0.0);
            // We want net advance = advance_pt
            // net = (declared_width - adjustment) / 1000 * font_size = advance_pt
            // => adjustment = declared_width - advance_pt / font_size * 1000
            let adjustment = (declared_width - advance_pt / font_size * 1000.0) as f32;
            items.adjust(adjustment);
        }
    }

    items.finish();
    positioned.finish();
}

fn emit_horizontally_positioned_multilingual_glyphs(
    content: &mut Content,
    run: &oxoffice_layout::output::MultilingualGlyphRun,
    remapper: &subsetter::GlyphRemapper,
    widths: &[(u16, f64)],
) {
    let width_map: HashMap<u16, f64> = widths.iter().copied().collect();
    let mut positioned = content.show_positioned();
    let mut items = positioned.items();
    let mut advance = 0.0;
    let mut current = 0.0;
    for (index, &glyph_id) in run.glyph_ids.iter().enumerate() {
        let target = advance + run.x_offsets[index];
        let adjustment = (current - target) / run.font_size * 1000.0;
        if adjustment != 0.0 {
            items.adjust(adjustment as f32);
        }
        let new_gid = remapper.get(glyph_id).unwrap_or(0);
        let bytes = new_gid.to_be_bytes();
        items.show(Str(&bytes));
        current = target + width_map.get(&new_gid).copied().unwrap_or(0.0) / 1000.0 * run.font_size;
        advance += run.x_advances[index];
    }
    items.finish();
    positioned.finish();
}

fn transform_array(transform: Transform) -> [f32; 6] {
    [
        transform.a as f32,
        transform.b as f32,
        transform.c as f32,
        transform.d as f32,
        transform.e as f32,
        transform.f as f32,
    ]
}

fn inverse_transform(transform: Transform) -> Option<Transform> {
    let determinant = transform.a * transform.d - transform.b * transform.c;
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let inverse = Transform {
        a: transform.d / determinant,
        b: -transform.b / determinant,
        c: -transform.c / determinant,
        d: transform.a / determinant,
        e: (transform.c * transform.f - transform.d * transform.e) / determinant,
        f: (transform.b * transform.e - transform.a * transform.f) / determinant,
    };
    [
        inverse.a, inverse.b, inverse.c, inverse.d, inverse.e, inverse.f,
    ]
    .into_iter()
    .all(f64::is_finite)
    .then_some(inverse)
}

/// Emit a rich run at its shaped per-glyph positions.
fn apply_text_synthesis(
    content: &mut Content,
    font: &oxoffice_layout::FontData,
    size: f64,
    color: Color,
    baseline_y: f64,
) {
    if font.synthetic_italic {
        // Transform the complete painted run, including the centered stroke.
        // The text matrix stays upright and preserves all advance/offset math.
        let shear = 0.212_556_56;
        content.transform([
            1.0,
            0.0,
            -shear,
            1.0,
            (baseline_y * f64::from(shear)) as f32,
            0.0,
        ]);
    }
    if font.synthetic_bold {
        content.set_stroke_rgb(color.r as f32, color.g as f32, color.b as f32);
        content.set_line_width((size / 24.0) as f32);
        content.set_line_cap(LineCapStyle::RoundCap);
        content.set_line_join(LineJoinStyle::RoundJoin);
        content.set_text_rendering_mode(TextRenderingMode::FillStroke);
    }
}

fn emit_multilingual_glyphs(
    content: &mut Content,
    run: &oxoffice_layout::output::MultilingualGlyphRun,
    text_matrix: [f32; 6],
    remapper: &subsetter::GlyphRemapper,
    widths: &[(u16, f64)],
) {
    if !run.is_valid() {
        return;
    }
    content.set_text_matrix(text_matrix);
    if run.y_advances.iter().all(|value| *value == 0.0)
        && run.y_offsets.iter().all(|value| *value == 0.0)
    {
        emit_horizontally_positioned_multilingual_glyphs(content, run, remapper, widths);
        return;
    }
    let mut advance_x = 0.0;
    let mut advance_y = 0.0;
    let mut positioned_x = 0.0;
    let mut positioned_y = 0.0;
    for (index, &glyph_id) in run.glyph_ids.iter().enumerate() {
        let target_x = advance_x + run.x_offsets[index];
        let target_y = advance_y + run.y_offsets[index];
        let delta_x = target_x - positioned_x;
        let delta_y = target_y - positioned_y;
        if delta_x != 0.0 || delta_y != 0.0 {
            content.next_line(delta_x as f32, delta_y as f32);
        }
        emit_glyphs(
            content,
            std::slice::from_ref(&glyph_id),
            std::slice::from_ref(&run.x_advances[index]),
            run.font_size,
            remapper,
            widths,
        );
        positioned_x = target_x;
        positioned_y = target_y;
        advance_x += run.x_advances[index];
        advance_y += run.y_advances[index];
    }
}

/// Write the outline tree (bookmarks) with hierarchical structure.
///
/// H2 entries become children of the preceding H1, H3 of the preceding H2, etc.
fn write_outlines(
    pdf: &mut Pdf,
    root_id: Ref,
    outlines: &[oxoffice_layout::OutlineEntry],
    item_ids: &[Ref],
    page_ids: &[Ref],
    layout: &LayoutResult,
) {
    if outlines.is_empty() {
        return;
    }

    // Build parent/children/prev/next relationships based on heading levels.
    // parent_idx[i] = index of this item's parent, or None if top-level
    // children[i] = indices of children of item i
    let n = outlines.len();
    let mut parent_idx: Vec<Option<usize>> = vec![None; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];

    // Stack tracks (index, level) of open ancestors
    let mut stack: Vec<(usize, u32)> = Vec::new();

    for (i, entry) in outlines.iter().enumerate() {
        // Pop entries that are at the same or deeper level
        while let Some(&(_, lvl)) = stack.last() {
            if lvl >= entry.level {
                stack.pop();
            } else {
                break;
            }
        }

        if let Some(&(parent, _)) = stack.last() {
            parent_idx[i] = Some(parent);
            children[parent].push(i);
        }
        // else: top-level child of root

        stack.push((i, entry.level));
    }

    // Identify top-level items (children of root)
    let top_level: Vec<usize> = (0..n).filter(|i| parent_idx[*i].is_none()).collect();

    // Write outline root
    if let (Some(&first), Some(&last)) = (top_level.first(), top_level.last()) {
        let mut root = pdf.outline(root_id);
        root.first(item_ids[first]);
        root.last(item_ids[last]);
        root.count(n as i32); // total count including descendants
        root.finish();
    }

    // Write each outline item
    for (i, entry) in outlines.iter().enumerate() {
        let mut item = pdf.outline_item(item_ids[i]);
        item.title(TextStr(&entry.title));

        // Parent
        let actual_parent = match parent_idx[i] {
            Some(pi) => item_ids[pi],
            None => root_id,
        };
        item.parent(actual_parent);

        // Siblings (prev/next among items sharing the same parent)
        let siblings: &[usize] = if let Some(pi) = parent_idx[i] {
            &children[pi]
        } else {
            &top_level
        };
        if let Some(pos) = siblings.iter().position(|&x| x == i) {
            if pos > 0 {
                item.prev(item_ids[siblings[pos - 1]]);
            }
            if pos + 1 < siblings.len() {
                item.next(item_ids[siblings[pos + 1]]);
            }
        }

        // First/last child
        if !children[i].is_empty() {
            item.first(item_ids[children[i][0]]);
            item.last(item_ids[*children[i].last().unwrap()]);
            item.count(children[i].len() as i32);
        }

        // Page destination
        let page_idx = entry.page_index.min(page_ids.len().saturating_sub(1));
        if page_idx < page_ids.len() {
            let page_height = layout
                .pages
                .get(page_idx)
                .map(|p| p.height)
                .unwrap_or(792.0);
            let pdf_y = page_height - entry.y_position;
            item.dest()
                .page(page_ids[page_idx])
                .xyz(0.0, pdf_y as f32, None);
        }

        item.finish();
    }
}

/// Set color space on an image XObject.
fn set_color_space(img: &mut pdf_writer::writers::ImageXObject<'_>, cs: &str) {
    match cs {
        "DeviceGray" => {
            img.color_space().device_gray();
        }
        "DeviceCMYK" => {
            img.color_space().device_cmyk();
        }
        _ => {
            img.color_space().device_rgb();
        }
    }
}

/// Six capital letters naming a font subset, from the digest of its bytes.
fn subset_tag(subset: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(subset)
        .iter()
        .take(6)
        .map(|byte| char::from(b'A' + byte % 26))
        .collect()
}

/// Create a sanitized PostScript font name.
fn sanitize_font_name(family: &str, bold: bool, italic: bool) -> String {
    let mut name = family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>();

    if name.is_empty() {
        name = "Font".to_string();
    }

    if bold && italic {
        name.push_str("-BoldItalic");
    } else if bold {
        name.push_str("-Bold");
    } else if italic {
        name.push_str("-Italic");
    }

    name
}
