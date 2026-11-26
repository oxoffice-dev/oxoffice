//! DrawingML of pictures, text boxes and watermarks: building and checking it.

use super::*;

// Qualified physical geometry, separate from the legacy typed parser's
// permissive local-name fields. Foreign lookalikes remain opaque.
pub(super) fn picture_source_extent(
    raw: &[u8],
    scope: &BTreeMap<String, String>,
    anchored: bool,
) -> Result<Option<(i64, i64)>> {
    let closed = close_content_fragment_namespaces(raw, scope)?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut depth = 0usize;
    let mut extent = None;
    loop {
        let (namespace, event) = match reader.read_resolved_event() {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(error.to_string()));
            }
        };
        let word_drawing = matches!(namespace,ResolveResult::Bound(Namespace(uri)) if uri==drawing_ns::WP.as_bytes());
        drop(namespace);
        match event {
            Event::Start(ref tag) | Event::Empty(ref tag) => {
                let local = tag.local_name();
                if depth == 0
                    && (!word_drawing
                        || local.as_ref() != if anchored { b"anchor" } else { b"inline" })
                {
                    return Ok(None);
                }
                if depth == 1 && word_drawing && local.as_ref() == b"extent" {
                    if extent.is_some() {
                        return Err(Error::Other("picture has duplicate owned extents".into()));
                    }
                    let number = |name: &str| -> Result<i64> {
                        let attr = tag
                            .try_get_attribute(name)
                            .map_err(|error| Error::Other(error.to_string()))?
                            .ok_or_else(|| Error::Other(format!("picture extent has no {name}")))?;
                        let value = attr
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| Error::Other(error.to_string()))?;
                        value
                            .parse()
                            .map_err(|_| Error::Other("picture extent is not an integer".into()))
                    };
                    extent = Some((number("cx")?, number("cy")?));
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("picture source is malformed".into()))?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    extent
        .map(Some)
        .ok_or_else(|| Error::Other("picture has no owned extent".into()))
}

pub(super) fn picture_drawing(
    relationship_id: &str,
    drawing_id: u32,
    options: &PictureOptions,
) -> CT_Drawing {
    let source_rect = options.crop.map(|crop| SourceRect {
        left: crop.left,
        top: crop.top,
        right: crop.right,
        bottom: crop.bottom,
    });
    match options.anchor {
        Some(anchor_options) => {
            let mut anchor = CT_Anchor::background(
                relationship_id,
                options.width.to_emu(),
                options.height.to_emu(),
            );
            apply_picture_anchor(&mut anchor, anchor_options);
            anchor.doc_pr_id = drawing_id;
            anchor.name = options.name.clone();
            anchor.description = options.description.clone();
            anchor.title = options.title.clone();
            anchor.decorative = options.decorative;
            anchor.source_rect = source_rect;
            CT_Drawing::anchor(anchor)
        }
        None => {
            let mut inline = CT_Inline::new(
                relationship_id,
                options.width.to_emu(),
                options.height.to_emu(),
            );
            inline.doc_pr_id = drawing_id;
            inline.name = options.name.clone();
            inline.description = options.description.clone();
            inline.title = options.title.clone();
            inline.decorative = options.decorative;
            inline.source_rect = source_rect;
            CT_Drawing::inline(inline)
        }
    }
}

pub(super) fn validate_picture_options(options: &PictureOptions) -> Result<()> {
    if options.width.to_emu() <= 0 || options.height.to_emu() <= 0 {
        return Err(Error::Other(
            "picture width and height must be positive".to_owned(),
        ));
    }
    for value in [&options.name, &options.description, &options.title]
        .into_iter()
        .flatten()
    {
        oxoffice_core::xml::reject_non_xml_characters("picture metadata", value)?;
    }
    if let Some(crop) = options.crop {
        for (name, value) in [
            ("left", crop.left),
            ("top", crop.top),
            ("right", crop.right),
            ("bottom", crop.bottom),
        ] {
            if !(0..100_000).contains(&value) {
                return Err(Error::Other(format!(
                    "picture crop {name} must be between 0 and 99999"
                )));
            }
        }
        if crop.left + crop.right >= 100_000 || crop.top + crop.bottom >= 100_000 {
            return Err(Error::Other(
                "opposite picture crop values must leave a visible source rectangle".to_owned(),
            ));
        }
    }
    if let Some(anchor) = options.anchor {
        validate_picture_anchor(anchor)?;
    }
    Ok(())
}

fn validate_picture_anchor(anchor: PictureAnchor) -> Result<()> {
    for (name, value) in [
        ("top", anchor.distance_top),
        ("bottom", anchor.distance_bottom),
        ("left", anchor.distance_left),
        ("right", anchor.distance_right),
    ] {
        if value.to_emu() < 0 {
            return Err(Error::Other(format!(
                "drawing {name} wrap distance must be nonnegative"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_text_box_options(options: &TextBoxOptions) -> Result<()> {
    if options.width.to_emu() <= 0 || options.height.to_emu() <= 0 {
        return Err(Error::Other(
            "text box width and height must be positive".to_owned(),
        ));
    }
    drawingml_rotation(options.rotation_degrees)?;
    if let Some(color) = options.fill_color.as_deref()
        && !is_rgb_hex(color)
    {
        return Err(Error::Other(
            "text box fill color must be six hexadecimal digits".to_owned(),
        ));
    }
    validate_picture_anchor(options.anchor)
}

fn drawingml_rotation(rotation_degrees: f64) -> Result<i32> {
    if !rotation_degrees.is_finite() {
        return Err(Error::Other("text box rotation must be finite".to_owned()));
    }
    let rotation = rotation_degrees * 60_000.0;
    if rotation < f64::from(i32::MIN) || rotation > f64::from(i32::MAX) {
        return Err(Error::Other(
            "text box rotation exceeds the DrawingML angle range".to_owned(),
        ));
    }
    Ok(rotation.round() as i32)
}

pub(super) fn validate_text_watermark_options(options: &TextWatermarkOptions) -> Result<()> {
    if options.width.to_emu() <= 0 || options.height.to_emu() <= 0 {
        return Err(Error::Other(
            "watermark width and height must be positive".to_owned(),
        ));
    }
    if !options.rotation_degrees.is_finite() {
        return Err(Error::Other("watermark rotation must be finite".to_owned()));
    }
    if !options.opacity.is_finite() || !(0.0..=1.0).contains(&options.opacity) {
        return Err(Error::Other(
            "watermark opacity must be between zero and one".to_owned(),
        ));
    }
    if !is_rgb_hex(&options.color) {
        return Err(Error::Other(
            "watermark color must be six hexadecimal digits".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn is_rgb_hex(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn apply_picture_anchor(anchor: &mut CT_Anchor, options: PictureAnchor) {
    anchor.behind_doc = options.behind_text;
    anchor.pos_h_relative_from = options.horizontal_relative_from.into();
    anchor.pos_h_offset = oxoffice_docx_oxml::units::Emu(options.horizontal_offset.to_emu());
    anchor.pos_h_align = options.horizontal_alignment.map(Into::into);
    anchor.pos_v_relative_from = options.vertical_relative_from.into();
    anchor.pos_v_offset = oxoffice_docx_oxml::units::Emu(options.vertical_offset.to_emu());
    anchor.pos_v_align = options.vertical_alignment.map(Into::into);
    anchor.wrap = options.wrap.into();
    anchor.dist_t = oxoffice_docx_oxml::units::Emu(options.distance_top.to_emu());
    anchor.dist_b = oxoffice_docx_oxml::units::Emu(options.distance_bottom.to_emu());
    anchor.dist_l = oxoffice_docx_oxml::units::Emu(options.distance_left.to_emu());
    anchor.dist_r = oxoffice_docx_oxml::units::Emu(options.distance_right.to_emu());
    anchor.relative_height = options.relative_height;
}

// Locate only the unchanged API-owned alternate representation. Raw unrelated
// header content and any edits outside this exact owner remain outside the splice.
pub(super) fn drawingml_watermark_span(xml: &[u8]) -> Result<Option<(Range<usize>, u32)>> {
    let mut reader = oxoffice_core::xml_reader::NsReader::from_reader(xml);
    let mut depth = 0usize;
    struct WatermarkOwner {
        start: usize,
        depth: usize,
        marker: Option<(u32, String)>,
    }
    let mut alternate: Option<WatermarkOwner> = None;
    let mut selected = None;
    loop {
        let start = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(format!("watermark owner scan failed: {error}")))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name();
                let namespace = match namespace {
                    quick_xml::name::ResolveResult::Bound(namespace) => namespace.as_ref().to_vec(),
                    _ => Vec::new(),
                };
                if namespace == b"http://schemas.openxmlformats.org/markup-compatibility/2006"
                    && local.as_ref() == b"AlternateContent"
                    && alternate.is_none()
                {
                    alternate = Some(WatermarkOwner {
                        start,
                        depth,
                        marker: None,
                    });
                }
                if namespace == b"http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                    && local.as_ref() == b"drawing"
                    && alternate.is_none()
                {
                    alternate = Some(WatermarkOwner {
                        start,
                        depth,
                        marker: None,
                    });
                }
                if namespace
                    == b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                    && local.as_ref() == b"docPr"
                {
                    let mut name = None;
                    let mut id = None;
                    let mut description = None;
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(oxoffice_docx_oxml::OxmlError::from)?;
                        let value = attribute
                            .decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?
                            .into_owned();
                        match attribute.key.as_ref() {
                            b"name" => name = Some(value),
                            b"id" => id = value.parse::<u32>().ok(),
                            b"descr" => description = Some(value),
                            _ => {}
                        }
                    }
                    if name.as_deref() == Some("RDOCX DrawingML Watermark") {
                        let owner = alternate.as_mut().ok_or_else(|| {
                            Error::Other("watermark identity is outside its alternate owner".into())
                        })?;
                        if owner.marker.is_some() {
                            return Err(Error::Other(
                                "duplicate watermark identity is unsupported".into(),
                            ));
                        }
                        owner.marker = Some((
                            id.ok_or_else(|| {
                                Error::Other("watermark drawing identity is invalid".into())
                            })?,
                            description.ok_or_else(|| {
                                Error::Other("watermark source qualification is missing".into())
                            })?,
                        ));
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("watermark owner depth is invalid".into()))?;
                if alternate.as_ref().is_some_and(|owner| owner.depth == depth) {
                    let owner = alternate.take().unwrap();
                    let start = owner.start;
                    if let Some((id, description)) = owner.marker {
                        if selected.is_some() {
                            return Err(Error::Other(
                                "multiple API watermark owners are unsupported".into(),
                            ));
                        }
                        let end = reader.buffer_position() as usize;
                        let text = std::str::from_utf8(&xml[start..end])
                            .map_err(|error| Error::Other(error.to_string()))?;
                        let attribute = format!(" descr=\"{description}\"");
                        let checksum = description
                            .strip_prefix("rdocx-watermark-text-v1:")
                            .or_else(|| description.strip_prefix("rdocx-watermark-image-v1:"))
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| {
                                Error::Other("watermark source qualification is invalid".into())
                            })?;
                        if text.matches(&attribute).count() != 1
                            || structural_fingerprint(text.replacen(&attribute, "", 1).as_bytes())?
                                != checksum
                        {
                            return Err(Error::Other("API watermark has opaque or externally changed source, replacement refused".into()));
                        }
                        selected = Some((start..end, id));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(selected)
}

pub(super) fn authored_text_box_alternate_content(
    drawing_id: u32,
    text: &str,
    options: &TextBoxOptions,
) -> Result<Vec<u8>> {
    let horizontal = match options.anchor.horizontal_alignment {
        Some(alignment) => format!(
            "<wp:align>{}</wp:align>",
            AnchorAlignH::from(alignment).to_str()
        ),
        None => format!(
            "<wp:posOffset>{}</wp:posOffset>",
            options.anchor.horizontal_offset.to_emu()
        ),
    };
    let vertical = match options.anchor.vertical_alignment {
        Some(alignment) => format!(
            "<wp:align>{}</wp:align>",
            AnchorAlignV::from(alignment).to_str()
        ),
        None => format!(
            "<wp:posOffset>{}</wp:posOffset>",
            options.anchor.vertical_offset.to_emu()
        ),
    };
    let wrap = match options.anchor.wrap {
        DrawingWrap::None => "<wp:wrapNone/>".to_owned(),
        DrawingWrap::Square => "<wp:wrapSquare wrapText=\"bothSides\"/>".to_owned(),
        DrawingWrap::TopAndBottom => "<wp:wrapTopAndBottom/>".to_owned(),
        DrawingWrap::Tight => {
            "<wp:wrapTight wrapText=\"bothSides\"><wp:wrapPolygon edited=\"0\"><wp:start x=\"0\" y=\"0\"/><wp:lineTo x=\"0\" y=\"21600\"/><wp:lineTo x=\"21600\" y=\"21600\"/><wp:lineTo x=\"21600\" y=\"0\"/><wp:lineTo x=\"0\" y=\"0\"/></wp:wrapPolygon></wp:wrapTight>".to_owned()
        }
        DrawingWrap::Through => {
            "<wp:wrapThrough wrapText=\"bothSides\"><wp:wrapPolygon edited=\"0\"><wp:start x=\"0\" y=\"0\"/><wp:lineTo x=\"0\" y=\"21600\"/><wp:lineTo x=\"21600\" y=\"21600\"/><wp:lineTo x=\"21600\" y=\"0\"/><wp:lineTo x=\"0\" y=\"0\"/></wp:wrapPolygon></wp:wrapThrough>".to_owned()
        }
    };
    let direction = match options.text_direction {
        TextBoxDirection::Horizontal => "horz",
        TextBoxDirection::Vertical => "vert",
        TextBoxDirection::Vertical270 => "vert270",
    };
    let vml_direction = match options.text_direction {
        TextBoxDirection::Horizontal => "layout-flow:horizontal",
        TextBoxDirection::Vertical => "layout-flow:vertical;mso-layout-flow-alt:top-to-bottom",
        TextBoxDirection::Vertical270 => "layout-flow:vertical;mso-layout-flow-alt:bottom-to-top",
    };
    let fill = options.fill_color.as_deref().map_or_else(
        || "<a:noFill/>".to_owned(),
        |color| format!("<a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill>"),
    );
    let rotation = drawingml_rotation(options.rotation_degrees)?;
    let escaped = quick_xml::escape::escape(text);
    let width = options.width.to_emu();
    let height = options.height.to_emu();
    let anchor = options.anchor;
    let xml = format!(
        concat!(
            "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" ",
            "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
            "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" ",
            "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ",
            "xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" ",
            "xmlns:v=\"urn:schemas-microsoft-com:vml\" ",
            "xmlns:o=\"urn:schemas-microsoft-com:office:office\"><mc:Choice Requires=\"wps\"><w:drawing>",
            "<wp:anchor behindDoc=\"{behind}\" simplePos=\"0\" relativeHeight=\"{relative_height}\" ",
            "distT=\"{dist_t}\" distB=\"{dist_b}\" distL=\"{dist_l}\" distR=\"{dist_r}\" ",
            "locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>",
            "<wp:positionH relativeFrom=\"{relative_h}\">{horizontal}</wp:positionH>",
            "<wp:positionV relativeFrom=\"{relative_v}\">{vertical}</wp:positionV>",
            "<wp:extent cx=\"{width}\" cy=\"{height}\"/>{wrap}<wp:docPr id=\"{drawing_id}\" name=\"Text Box {drawing_id}\"/>",
            "<a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\">",
            "<wps:wsp><wps:cNvSpPr txBox=\"1\"/><wps:spPr><a:xfrm rot=\"{rotation}\"><a:off x=\"0\" y=\"0\"/>",
            "<a:ext cx=\"{width}\" cy=\"{height}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>{fill}</wps:spPr>",
            "<wps:txbx><w:txbxContent><w:p><w:r><w:t xml:space=\"preserve\">{escaped}</w:t></w:r></w:p></w:txbxContent></wps:txbx>",
            "<wps:bodyPr vert=\"{direction}\"/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice>",
            "<mc:Fallback><w:pict><v:shapetype id=\"rdocx-textbox-type-{drawing_id}\" coordsize=\"21600,21600\" ",
            "o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/>",
            "<v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype>",
            "<v:shape id=\"rdocx-textbox-{drawing_id}\" type=\"#rdocx-textbox-type-{drawing_id}\" ",
            "style=\"position:absolute;width:{width_pt}pt;height:{height_pt}pt;rotation:{rotation_degrees}\" ",
            "fillcolor=\"#{vml_fill}\"><v:textbox style=\"{vml_direction}\"><w:txbxContent><w:p><w:r>",
            "<w:t xml:space=\"preserve\">{escaped}</w:t></w:r></w:p>",
            "</w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent>"
        ),
        behind = u8::from(anchor.behind_text),
        horizontal = horizontal,
        vertical = vertical,
        width = width,
        height = height,
        wrap = wrap,
        drawing_id = drawing_id,
        rotation = rotation,
        fill = fill,
        direction = direction,
        vml_direction = vml_direction,
        escaped = escaped,
        relative_height = anchor.relative_height,
        dist_t = anchor.distance_top.to_emu(),
        dist_b = anchor.distance_bottom.to_emu(),
        dist_l = anchor.distance_left.to_emu(),
        dist_r = anchor.distance_right.to_emu(),
        relative_h = ST_RelativeFromH::from(anchor.horizontal_relative_from).to_str(),
        relative_v = ST_RelativeFromV::from(anchor.vertical_relative_from).to_str(),
        width_pt = options.width.to_pt(),
        height_pt = options.height.to_pt(),
        rotation_degrees = options.rotation_degrees,
        vml_fill = options.fill_color.as_deref().unwrap_or("FFFFFF"),
    );
    Ok(xml.into_bytes())
}
