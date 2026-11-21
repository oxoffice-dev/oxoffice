//! Pictures on slides: adding PNG, JPEG, GIF and SVG images with a position
//! and a size, reading back their image and alt text, replacing, cropping and
//! removing them, the media parts they share, picture placeholders, the
//! picture each corpus producer saved, and a video with its poster.

use std::collections::BTreeMap;
use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::{
    EmbeddedMediaInput, Emu, Error, Fill, FillOwnerLocation, MediaKind, MediaLocation,
    MediaPlaybackSettings, MediaPoster, MediaSourceInput, Percent1000, Presentation, ShapeKind,
    ShapeOwner, ShapeType,
};

use crate::support::{corpus_deck, new_deck, reopen, slide_texts};

/// One inch, in EMU.
const INCH: i64 = 914_400;

const IMAGE_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// A PNG of one colour, `width` by `height` pixels, with no declared DPI.
fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("non-empty PNG");
    pixmap.fill(tiny_skia::Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
    pixmap.encode_png().expect("PNG encodes")
}

/// A grey JPEG, `width` by `height` pixels, declaring `dpi` when given.
fn jpeg(width: u16, height: u16, dpi: Option<u16>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, 90);
    if let Some(dpi) = dpi {
        encoder.set_density(jpeg_encoder::Density::Inch { x: dpi, y: dpi });
    }
    let pixels = vec![128; usize::from(width) * usize::from(height) * 3];
    encoder
        .encode(&pixels, width, height, jpeg_encoder::ColorType::Rgb)
        .expect("JPEG encodes");
    bytes
}

/// A white GIF, `width` by `height` pixels.
fn gif(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut bytes, width, height, &[0, 0, 0, 255, 255, 255]).unwrap();
        let pixels = vec![1; usize::from(width) * usize::from(height)];
        let frame = gif::Frame::from_indexed_pixels(width, height, pixels, None);
        encoder.write_frame(&frame).expect("GIF encodes");
    }
    bytes
}

/// The 900 x 450 PNG every corpus deck shows on its "Revenue by quarter"
/// slide.
fn corpus_png() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files/powerpoint/deck.pptx");
    let package = OpcPackage::open(path).expect("corpus deck opens as a package");
    package
        .get_part("/ppt/media/image1.png")
        .expect("corpus picture part")
        .to_vec()
}

/// A new deck with `count` slides on the Blank layout.
fn blank_deck(count: usize) -> Presentation {
    let mut prs = new_deck();
    let blank = layout_named(&prs, "Blank");
    for _ in 0..count {
        prs.add_slide(blank).unwrap();
    }
    prs
}

fn layout_named(prs: &Presentation, name: &str) -> usize {
    (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some(name))
        .unwrap_or_else(|| panic!("no layout named {name}"))
}

/// Adds a picture at one inch from the top left corner and returns its id.
fn add_picture(
    prs: &mut Presentation,
    slide: usize,
    image: &[u8],
    size: (Option<i64>, Option<i64>),
) -> u32 {
    prs.add_picture(
        slide,
        image,
        "image.png",
        Emu(INCH),
        Emu(INCH),
        size.0.map(Emu),
        size.1.map(Emu),
    )
    .unwrap()
    .non_visual_id()
    .expect("a new picture has an id")
}

/// Adds a textbox to a slide and returns its id.
fn add_textbox(prs: &mut Presentation, slide: usize) -> u32 {
    prs.slide_mut(slide)
        .unwrap()
        .add_textbox(Emu(0), Emu(0), Emu(INCH), Emu(INCH))
        .unwrap();
    prs.slide(slide)
        .unwrap()
        .shapes()
        .next_back()
        .and_then(|shape| shape.non_visual_id())
        .unwrap()
}

/// The z-order index of the shape with this id among the slide's own shapes.
fn shape_index(prs: &Presentation, slide: usize, id: u32) -> usize {
    prs.slide(slide)
        .unwrap()
        .shapes()
        .position(|shape| shape.non_visual_id() == Some(id))
        .unwrap_or_else(|| panic!("no shape {id} on slide {slide}"))
}

/// The id of the only picture among the slide's own shapes.
fn only_picture(prs: &Presentation, slide: usize) -> u32 {
    let ids: Vec<u32> = prs
        .slide(slide)
        .unwrap()
        .shapes()
        .filter(|shape| shape.kind() == ShapeKind::Picture)
        .filter_map(|shape| shape.non_visual_id())
        .collect();
    assert_eq!(ids.len(), 1, "pictures on slide {slide}: {ids:?}");
    ids[0]
}

/// The media parts of the saved package, by part name.
fn media_parts(prs: &Presentation) -> BTreeMap<String, Vec<u8>> {
    let bytes = prs.to_bytes().expect("deck saves");
    OpcPackage::from_reader(Cursor::new(bytes))
        .expect("saved package opens")
        .parts
        .into_iter()
        .filter(|(name, _)| name.starts_with("/ppt/media/"))
        .map(|(name, bytes)| (name, bytes.to_vec()))
        .collect()
}

/// The bytes of every media part of the saved package, sorted.
fn stored_media(prs: &Presentation) -> Vec<Vec<u8>> {
    let mut stored: Vec<Vec<u8>> = media_parts(prs).into_values().collect();
    stored.sort();
    stored
}

fn sorted(mut images: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    images.sort();
    images
}

/// A picture's crop as left, top, right and bottom thousandths of a percent.
fn crop(prs: &Presentation, slide: usize, id: u32) -> Option<[i32; 4]> {
    let shape = prs
        .slide(slide)
        .unwrap()
        .shape(shape_index(prs, slide, id))?;
    shape
        .crop()
        .map(|(left, top, right, bottom)| [left.0, top.0, right.0, bottom.0])
}

fn set_crop(
    prs: &mut Presentation,
    slide: usize,
    index: usize,
    edges: [i32; 4],
) -> Result<(), Error> {
    let [left, top, right, bottom] = edges.map(Percent1000);
    prs.slide_mut(slide)
        .unwrap()
        .shape_mut(index)
        .unwrap()
        .set_crop(left, top, right, bottom)
}

// Adding

#[test]
fn a_picture_keeps_its_position_size_and_image_after_a_save() {
    let image = png(40, 20, [200, 30, 30]);
    let mut prs = blank_deck(1);
    let id = prs
        .add_picture(
            0,
            &image,
            "chart.png",
            Emu(457_200),
            Emu(914_400),
            Some(Emu(3 * INCH)),
            Some(Emu(INCH)),
        )
        .unwrap()
        .non_visual_id()
        .unwrap();

    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    let picture = prs
        .slide(0)
        .unwrap()
        .shape(shape_index(&prs, 0, id))
        .unwrap();
    assert_eq!(picture.kind(), ShapeKind::Picture);
    assert_eq!(picture.shape_type(), Some(ShapeType::Picture));
    assert_eq!(picture.non_visual_name(), Some(format!("Picture {id}")));
    assert_eq!(picture.position(), Some((Emu(457_200), Emu(914_400))));
    assert_eq!(picture.size(), Some((Emu(3 * INCH), Emu(INCH))));
    let read = prs.picture_image(0, id).unwrap();
    assert_eq!(read.bytes, image.as_slice());
    assert_eq!(read.content_type, "image/png");
    assert!(
        read.part_name.starts_with("/ppt/media/"),
        "{}",
        read.part_name
    );
}

#[test]
fn a_picture_without_a_size_takes_the_size_its_image_declares() {
    let mut prs = blank_deck(1);
    // No declared DPI counts as 72 dpi: a pixel is 12,700 EMU.
    let plain = add_picture(&mut prs, 0, &png(32, 16, [0, 0, 0]), (None, None));
    let at_144_dpi = add_picture(&mut prs, 0, &jpeg(288, 144, Some(144)), (None, None));
    let prs = reopen(&prs);
    let size = |id| {
        prs.slide(0)
            .unwrap()
            .shape(shape_index(&prs, 0, id))
            .unwrap()
            .size()
    };
    assert_eq!(size(plain), Some((Emu(32 * 12_700), Emu(16 * 12_700))));
    assert_eq!(size(at_144_dpi), Some((Emu(2 * INCH), Emu(INCH))));
}

#[test]
fn a_picture_given_one_side_keeps_the_image_aspect_ratio() {
    let image = png(300, 200, [10, 120, 10]);
    let mut prs = blank_deck(1);
    let by_width = add_picture(&mut prs, 0, &image, (Some(3 * INCH), None));
    let by_height = add_picture(&mut prs, 0, &image, (None, Some(INCH)));
    let prs = reopen(&prs);
    let size = |id| {
        prs.slide(0)
            .unwrap()
            .shape(shape_index(&prs, 0, id))
            .unwrap()
            .size()
    };
    assert_eq!(size(by_width), Some((Emu(3 * INCH), Emu(2 * INCH))));
    assert_eq!(size(by_height), Some((Emu(INCH * 3 / 2), Emu(INCH))));
}

#[test]
fn png_jpeg_and_gif_pictures_keep_their_bytes_and_content_type() {
    let images = [
        (png(4, 3, [1, 2, 3]), "image/png"),
        (jpeg(8, 6, None), "image/jpeg"),
        (gif(5, 5), "image/gif"),
    ];
    let mut prs = blank_deck(1);
    let ids: Vec<u32> = images
        .iter()
        .map(|(bytes, _)| add_picture(&mut prs, 0, bytes, (None, None)))
        .collect();

    let prs = reopen(&prs);
    for ((bytes, content_type), id) in images.iter().zip(ids) {
        let read = prs.picture_image(0, id).unwrap();
        assert_eq!(read.bytes, bytes.as_slice(), "{content_type}");
        assert_eq!(read.content_type, *content_type);
    }
    assert_eq!(media_parts(&prs).len(), 3);
}

#[test]
fn the_image_format_comes_from_the_bytes_not_the_file_name() {
    let mut prs = blank_deck(1);
    prs.add_picture(
        0,
        &png(4, 3, [9, 9, 9]),
        "photo.jpg",
        Emu(0),
        Emu(0),
        None,
        None,
    )
    .unwrap();
    let prs = reopen(&prs);
    let read = prs.picture_image(0, only_picture(&prs, 0)).unwrap();
    assert_eq!(read.content_type, "image/png");
    assert!(read.part_name.ends_with(".png"), "{}", read.part_name);
}

#[test]
fn bytes_that_are_not_an_image_are_refused_and_change_nothing() {
    let mut prs = blank_deck(1);
    let before = prs.to_bytes().unwrap();
    let error = prs
        .add_picture(0, b"not an image", "a.png", Emu(0), Emu(0), None, None)
        .err()
        .expect("refused");
    assert!(matches!(error, Error::UnsupportedPicture { .. }), "{error}");
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn bytes_that_are_not_an_image_are_refused_even_with_both_sides_given() {
    let mut prs = blank_deck(1);
    let size = Some(Emu(INCH));
    match prs.add_picture(0, b"not an image", "a.png", Emu(0), Emu(0), size, size) {
        Ok(picture) => {
            let id = picture.non_visual_id().unwrap();
            let read = prs.picture_image(0, id).unwrap();
            panic!(
                "bytes that are not an image were added as a picture, stored as {} ({})",
                read.part_name, read.content_type
            );
        }
        Err(error) => assert!(matches!(error, Error::UnsupportedPicture { .. }), "{error}"),
    }
}

#[test]
fn every_editors_deck_takes_an_image_with_both_sides_given_and_refuses_other_bytes() {
    let size = Some(Emu(INCH));
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
    for name in [
        "powerpoint/deck.pptx",
        "google/deck.pptx",
        "libreoffice/deck.pptx",
        "authored/deck.pptx",
    ] {
        let mut prs = corpus_deck(name);
        let before = prs.to_bytes().unwrap();
        let refused = match prs.add_picture(
            1,
            b"%PDF-1.7 not an image",
            "a.png",
            Emu(0),
            Emu(0),
            size,
            size,
        ) {
            Ok(_) => panic!("{name}: bytes that are not an image were added"),
            Err(error) => error,
        };
        assert!(
            matches!(refused, Error::UnsupportedPicture { .. }),
            "{name}: {refused}"
        );
        assert_eq!(prs.to_bytes().unwrap(), before, "{name}");

        prs.add_picture(
            1,
            &png(10, 4, [0, 120, 250]),
            "chart.png",
            Emu(0),
            Emu(0),
            size,
            size,
        )
        .unwrap();
        prs.add_picture(1, svg, "logo.svg", Emu(0), Emu(0), size, size)
            .unwrap();
        let prs = reopen(&prs);
        assert!(prs.validate().is_empty(), "{name}: {:?}", prs.validate());
    }
}

#[test]
fn tiff_and_wmf_go_through_with_both_sides_and_a_compressed_metafile_does_not() {
    let size = Some(Emu(INCH));
    let mut prs = blank_deck(1);
    let tiff = b"II*\0\x08\0\0\0\0\0\0\0";
    let mut wmf = b"\xd7\xcd\xc6\x9a".to_vec();
    wmf.extend_from_slice(&[0; 18]);
    for (bytes, name) in [(&tiff[..], "scan.tif"), (&wmf[..], "logo.wmf")] {
        prs.add_picture(0, bytes, name, Emu(0), Emu(0), size, size)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    // A .wmz is a gzip stream, not a metafile the sniffer knows.
    let refused =
        match prs.add_picture(0, b"\x1f\x8b\x08\0", "logo.wmz", Emu(0), Emu(0), size, size) {
            Ok(_) => panic!("a compressed metafile was added"),
            Err(error) => error,
        };
    assert!(
        matches!(refused, Error::UnsupportedPicture { .. }),
        "{refused}"
    );
}

#[test]
fn an_odp_picture_a_deck_cannot_hold_is_dropped_with_a_diagnostic() {
    use std::io::{Read, Write};
    let mut source = blank_deck(1);
    source
        .add_picture(
            0,
            &png(10, 4, [0, 120, 250]),
            "chart.png",
            Emu(0),
            Emu(0),
            None,
            None,
        )
        .unwrap();
    let odp = source.to_odp_bytes().unwrap().bytes;
    // Swap the picture for a LibreOffice metafile, which no sniffer knows.
    let mut archive = zip::ZipArchive::new(Cursor::new(odp)).unwrap();
    let mut rewritten = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut swapped = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name.starts_with("Pictures/") {
            bytes = b"VCLMTF\x01\x00 a StarView metafile".to_vec();
            swapped += 1;
        }
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        rewritten.start_file(name, options).unwrap();
        rewritten.write_all(&bytes).unwrap();
    }
    assert_eq!(swapped, 1, "the ODP holds the picture");
    let odp = rewritten.finish().unwrap().into_inner();

    let read = Presentation::from_odp_bytes(&odp).unwrap();

    assert_eq!(read.presentation.len(), 1);
    assert_eq!(read.presentation.slide(0).unwrap().shapes().count(), 0);
    let dropped: Vec<_> = read
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.path.as_str(), diagnostic.message.as_str()))
        .collect();
    assert_eq!(
        dropped,
        [(
            "slides/0/objects/0",
            "ODP image in a format a presentation cannot hold was dropped"
        )]
    );
}

#[test]
fn a_picture_on_a_layout_reads_back_and_takes_a_new_image_there() {
    let logo = png(10, 4, [250, 120, 0]);
    let mut prs = blank_deck(1);
    let layout = ShapeOwner::Layout(layout_named(&prs, "Blank"));
    prs.shapes_mut_at(layout, &[])
        .unwrap()
        .add_picture(&logo, "logo.png", Emu(0), Emu(0), Some(Emu(INCH)), None)
        .unwrap();
    let last = (0..)
        .take_while(|&index| prs.part_shape_at(layout, &[index]).is_ok())
        .last()
        .unwrap();
    let id = prs
        .part_shape_at(layout, &[last])
        .unwrap()
        .non_visual_id()
        .unwrap();

    let mut prs = reopen(&prs);
    assert_eq!(
        prs.picture_image_at(layout, id).unwrap().bytes,
        logo.as_slice()
    );
    let new_logo = png(10, 4, [0, 120, 250]);
    prs.replace_picture_image_at(layout, id, &new_logo).unwrap();
    let prs = reopen(&prs);
    assert_eq!(
        prs.picture_image_at(layout, id).unwrap().bytes,
        new_logo.as_slice()
    );
    assert_eq!(stored_media(&prs), vec![new_logo], "the old logo is gone");
}

// SVG

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#;

/// A Blank slide holding a picture as PowerPoint saves an SVG: the PNG
/// `fallback` in `a:blip` and [`SVG`] in its `asvg:svgBlip` extension,
/// described as "Logo". The picture has id 40.
fn svg_picture_deck(fallback: &[u8]) -> Presentation {
    let mut package =
        OpcPackage::from_reader(Cursor::new(blank_deck(1).to_bytes().unwrap())).unwrap();
    package.set_part("/ppt/media/image1.png", fallback.to_vec());
    package.set_part("/ppt/media/image2.svg", SVG.to_vec());
    package.content_types.add_default("png", "image/png");
    package.content_types.add_default("svg", "image/svg+xml");
    let slide = "/ppt/slides/slide1.xml";
    let picture = r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="Graphic 39" descr="Logo"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId20"><a:extLst><a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="rId21"/></a:ext></a:extLst></a:blip><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;
    let xml = String::from_utf8(package.get_part(slide).unwrap().to_vec())
        .unwrap()
        .replacen("</p:spTree>", &format!("{picture}</p:spTree>"), 1);
    package.set_part(slide, xml.into_bytes());
    let relationships = package.get_or_create_part_rels(slide);
    relationships.add_with_id("rId20", IMAGE_RELATIONSHIP, "../media/image1.png");
    relationships.add_with_id("rId21", IMAGE_RELATIONSHIP, "../media/image2.svg");
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Presentation::from_bytes(&bytes.into_inner()).expect("SVG picture deck opens")
}

#[test]
fn an_svg_picture_needs_an_explicit_size() {
    let mut prs = blank_deck(1);
    let before = prs.to_bytes().unwrap();
    let error = prs
        .add_picture(0, SVG, "logo.svg", Emu(0), Emu(0), Some(Emu(INCH)), None)
        .err()
        .expect("refused");
    assert!(
        matches!(error, Error::UnavailablePictureDimensions { .. }),
        "{error}"
    );
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn an_svg_picture_saved_by_powerpoint_reads_as_its_png_and_keeps_its_svg() {
    let fallback = png(2, 2, [255, 0, 0]);
    let prs = reopen(&svg_picture_deck(&fallback));
    let read = prs.picture_image(0, 40).unwrap();
    assert_eq!(read.bytes, fallback.as_slice());
    assert_eq!(read.content_type, "image/png");
    let picture = prs.slide(0).unwrap().shape(0).unwrap();
    assert_eq!(picture.alt_text().as_deref(), Some("Logo"));
    let xml = String::from_utf8(picture.xml().unwrap()).unwrap();
    assert!(xml.contains("svgBlip"), "{xml}");
    assert_eq!(stored_media(&prs), sorted(vec![fallback, SVG.to_vec()]));
}

#[test]
fn an_svg_picture_refuses_a_new_image_and_takes_both_its_images_when_removed() {
    let mut prs = svg_picture_deck(&png(2, 2, [255, 0, 0]));
    let before = prs.to_bytes().unwrap();
    // Replacing only the fallback would leave PowerPoint showing the SVG.
    let error = prs
        .replace_picture_image(0, 40, &png(2, 2, [0, 0, 255]))
        .unwrap_err();
    assert!(error.to_string().contains("second image"), "{error}");
    assert_eq!(prs.to_bytes().unwrap(), before);

    prs.remove_shape(0, 0).unwrap();
    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    assert!(media_parts(&prs).is_empty());
}

// Alt text, title, decorative

#[test]
fn alt_text_on_a_picture_survives_a_save_in_any_script() {
    let texts = [
        "Revenue by quarter, 2025",
        "Chiffre d'affaires <T1> & \"T2\"",
        "Two lines\nand a\ttab",
        "مخطط الإيرادات حسب الربع",
        "季度收入图表",
    ];
    let mut prs = blank_deck(1);
    let image = png(2, 2, [0, 0, 0]);
    for text in texts {
        let id = add_picture(&mut prs, 0, &image, (None, None));
        let index = shape_index(&prs, 0, id);
        let mut slide = prs.slide_mut(0).unwrap();
        slide
            .shape_mut(index)
            .unwrap()
            .set_alt_text(Some(text))
            .unwrap();
    }

    let prs = reopen(&prs);
    let read: Vec<Option<String>> = prs
        .slide(0)
        .unwrap()
        .shapes()
        .map(|shape| shape.alt_text())
        .collect();
    assert_eq!(read, texts.map(|text| Some(text.to_owned())));
}

#[test]
fn alt_text_can_be_cleared_and_refuses_characters_xml_cannot_carry() {
    let mut prs = blank_deck(1);
    add_picture(&mut prs, 0, &png(2, 2, [0, 0, 0]), (None, None));
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut picture = slide.shape_mut(0).unwrap();
        picture.set_alt_text(Some("A chart")).unwrap();
        let error = picture.set_alt_text(Some("A\u{1}chart")).unwrap_err();
        assert!(error.to_string().contains("U+0001"), "{error}");
    }
    let mut prs = reopen(&prs);
    assert_eq!(
        prs.slide(0)
            .unwrap()
            .shape(0)
            .unwrap()
            .alt_text()
            .as_deref(),
        Some("A chart")
    );

    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_alt_text(None)
        .unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.slide(0).unwrap().shape(0).unwrap().alt_text(), None);
}

#[test]
fn a_picture_title_and_decorative_flag_survive_a_save() {
    let mut prs = blank_deck(1);
    add_picture(&mut prs, 0, &png(2, 2, [0, 0, 0]), (None, None));
    assert!(!prs.slide(0).unwrap().shape(0).unwrap().decorative());
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut picture = slide.shape_mut(0).unwrap();
        picture.set_title(Some("Quarterly revenue")).unwrap();
        picture.set_decorative(true).unwrap();
    }
    let prs = reopen(&prs);
    let picture = prs.slide(0).unwrap().shape(0).unwrap();
    assert_eq!(picture.title().as_deref(), Some("Quarterly revenue"));
    assert!(picture.decorative());
}

// Replacing

#[test]
fn replacing_the_image_keeps_the_picture_geometry_name_and_alt_text() {
    let mut prs = blank_deck(1);
    let id = add_picture(
        &mut prs,
        0,
        &png(30, 20, [200, 0, 0]),
        (Some(3 * INCH), None),
    );
    let index = shape_index(&prs, 0, id);
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(index)
        .unwrap()
        .set_alt_text(Some("Sales"))
        .unwrap();
    // A wider image still fills the picture's box: the box does not change.
    let replacement = jpeg(60, 20, None);
    prs.replace_picture_image(0, id, &replacement).unwrap();

    let prs = reopen(&prs);
    let read = prs.picture_image(0, id).unwrap();
    assert_eq!(read.bytes, replacement.as_slice());
    assert_eq!(read.content_type, "image/jpeg");
    let picture = prs.slide(0).unwrap().shape(index).unwrap();
    assert_eq!(picture.position(), Some((Emu(INCH), Emu(INCH))));
    assert_eq!(picture.size(), Some((Emu(3 * INCH), Emu(2 * INCH))));
    assert_eq!(picture.alt_text().as_deref(), Some("Sales"));
    assert_eq!(picture.non_visual_name(), Some(format!("Picture {id}")));
}

#[test]
fn replacing_one_of_two_pictures_sharing_an_image_leaves_the_other_alone() {
    let shared = png(4, 4, [0, 100, 0]);
    let mut prs = blank_deck(2);
    let first = add_picture(&mut prs, 0, &shared, (None, None));
    let second = add_picture(&mut prs, 0, &shared, (None, None));
    let elsewhere = add_picture(&mut prs, 1, &shared, (None, None));
    let replacement = png(4, 4, [0, 0, 100]);
    prs.replace_picture_image(0, first, &replacement).unwrap();

    let prs = reopen(&prs);
    assert_eq!(
        prs.picture_image(0, first).unwrap().bytes,
        replacement.as_slice()
    );
    assert_eq!(
        prs.picture_image(0, second).unwrap().bytes,
        shared.as_slice()
    );
    assert_eq!(
        prs.picture_image(1, elsewhere).unwrap().bytes,
        shared.as_slice()
    );
    assert_eq!(stored_media(&prs), sorted(vec![shared, replacement]));
}

#[test]
fn replacing_the_image_drops_the_part_nobody_uses_any_more() {
    let mut prs = blank_deck(1);
    let id = add_picture(&mut prs, 0, &png(4, 4, [1, 1, 1]), (None, None));
    let new = gif(4, 4);
    prs.replace_picture_image(0, id, &new).unwrap();
    let prs = reopen(&prs);
    assert_eq!(stored_media(&prs), vec![new]);
    assert_eq!(prs.picture_image(0, id).unwrap().content_type, "image/gif");
}

#[test]
fn replacing_is_refused_for_bytes_that_are_not_an_image_or_a_shape_that_is_not_a_picture() {
    let mut prs = blank_deck(1);
    let picture = add_picture(&mut prs, 0, &png(4, 4, [1, 1, 1]), (None, None));
    let textbox = add_textbox(&mut prs, 0);
    let before = prs.to_bytes().unwrap();

    let error = prs
        .replace_picture_image(0, picture, b"not an image")
        .unwrap_err();
    assert!(
        error.to_string().contains("not a supported image"),
        "{error}"
    );
    let error = prs
        .replace_picture_image(0, textbox, &png(2, 2, [0, 0, 0]))
        .unwrap_err();
    assert!(error.to_string().contains("is not a picture"), "{error}");
    let error = prs.picture_image(0, textbox).unwrap_err();
    assert!(error.to_string().contains("is not a picture"), "{error}");
    assert!(prs.picture_image(0, 999).is_err(), "no shape has id 999");
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn a_duplicated_slide_shares_the_image_until_one_copy_is_replaced() {
    let image = png(8, 8, [50, 50, 50]);
    let mut prs = blank_deck(1);
    let id = add_picture(&mut prs, 0, &image, (None, None));
    prs.duplicate_slide(0).unwrap();
    let copy = only_picture(&prs, 1);
    assert_eq!(media_parts(&prs).len(), 1, "the copy shares the image part");

    let replacement = png(8, 8, [250, 250, 0]);
    prs.replace_picture_image(1, copy, &replacement).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.picture_image(0, id).unwrap().bytes, image.as_slice());
    assert_eq!(
        prs.picture_image(1, copy).unwrap().bytes,
        replacement.as_slice()
    );
}

#[test]
fn a_cloned_deck_keeps_its_pictures_when_the_clone_is_edited() {
    let image = png(4, 4, [9, 9, 9]);
    let mut original = blank_deck(1);
    let id = add_picture(&mut original, 0, &image, (None, None));
    let original = reopen(&original);

    let mut clone = original.clone();
    clone
        .replace_picture_image(0, id, &png(4, 4, [8, 8, 8]))
        .unwrap();
    add_picture(&mut clone, 0, &gif(3, 3), (None, None));

    let original = reopen(&original);
    assert_eq!(
        original.picture_image(0, id).unwrap().bytes,
        image.as_slice()
    );
    assert_eq!(stored_media(&original), vec![image]);
    assert_eq!(media_parts(&clone).len(), 2);
}

// Cropping

#[test]
fn a_crop_reads_back_after_a_save_and_a_zero_crop_shows_the_whole_image() {
    let mut prs = blank_deck(1);
    let id = add_picture(&mut prs, 0, &png(4, 4, [0, 0, 0]), (None, None));
    assert_eq!(
        crop(&prs, 0, id),
        Some([0; 4]),
        "a new picture is not cropped"
    );
    // A quarter off the left, an eighth off the bottom, and the right edge
    // pushed out by a tenth.
    set_crop(&mut prs, 0, 0, [25_000, 0, -10_000, 12_500]).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(crop(&prs, 0, id), Some([25_000, 0, -10_000, 12_500]));

    set_crop(&mut prs, 0, 0, [0; 4]).unwrap();
    let prs = reopen(&prs);
    assert_eq!(crop(&prs, 0, id), Some([0; 4]));
}

#[test]
fn only_a_picture_can_be_cropped() {
    let mut prs = blank_deck(1);
    let textbox = add_textbox(&mut prs, 0);
    assert_eq!(crop(&prs, 0, textbox), None);
    let before = prs.to_bytes().unwrap();
    let error = set_crop(&mut prs, 0, 0, [10_000, 0, 0, 0]).unwrap_err();
    assert!(
        matches!(error, Error::UnsupportedShapeMutation { .. }),
        "{error}"
    );
    assert_eq!(prs.to_bytes().unwrap(), before);
}

// Removing, and the media parts pictures share

#[test]
fn a_media_part_shared_by_two_pictures_stays_while_one_still_uses_it() {
    let image = png(4, 4, [4, 4, 4]);
    let mut prs = blank_deck(1);
    let first = add_picture(&mut prs, 0, &image, (None, None));
    let second = add_picture(&mut prs, 0, &image, (None, None));

    prs.remove_shape(0, shape_index(&prs, 0, first)).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(stored_media(&prs), vec![image.clone()]);
    assert_eq!(
        prs.picture_image(0, second).unwrap().bytes,
        image.as_slice()
    );

    prs.remove_shape(0, shape_index(&prs, 0, second)).unwrap();
    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    assert!(media_parts(&prs).is_empty());
    assert_eq!(prs.slide(0).unwrap().shapes().len(), 0);
}

#[test]
fn an_image_shared_across_slides_stays_until_its_last_slide_goes() {
    let image = png(4, 4, [5, 5, 5]);
    let mut prs = blank_deck(2);
    add_picture(&mut prs, 0, &image, (None, None));
    let survivor = add_picture(&mut prs, 1, &image, (None, None));

    prs.remove_slide(0).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(stored_media(&prs), vec![image.clone()]);
    assert_eq!(
        prs.picture_image(0, survivor).unwrap().bytes,
        image.as_slice()
    );

    prs.remove_slide(0).unwrap();
    assert!(media_parts(&reopen(&prs)).is_empty());
}

#[test]
fn an_image_the_background_and_the_layout_also_show_stays_when_the_picture_changes_or_goes() {
    let image = png(4, 4, [1, 2, 3]);
    let mut prs = blank_deck(1);
    let layout = ShapeOwner::Layout(layout_named(&prs, "Blank"));
    prs.shapes_mut_at(layout, &[])
        .unwrap()
        .add_picture(&image, "logo.png", Emu(0), Emu(0), Some(Emu(INCH)), None)
        .unwrap();
    let id = add_picture(&mut prs, 0, &image, (None, None));
    prs.set_picture_fill(
        &FillOwnerLocation::SlideBackground { slide_index: 0 },
        &image,
        "bg.png",
    )
    .unwrap();
    assert_eq!(media_parts(&prs).len(), 1, "one image, three users");

    let other = gif(2, 2);
    prs.replace_picture_image(0, id, &other).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(stored_media(&prs), sorted(vec![image.clone(), other]));

    prs.remove_shape(0, shape_index(&prs, 0, id)).unwrap();
    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    assert_eq!(stored_media(&prs), vec![image]);
    assert!(matches!(
        prs.slide(0).unwrap().background_fill(),
        Some(Fill::Blip(_))
    ));
}

#[test]
fn a_picture_inside_a_group_reads_back_and_goes_with_its_group() {
    let images = [png(6, 6, [0, 0, 200]), png(6, 6, [0, 200, 0])];
    let mut prs = blank_deck(1);
    prs.shapes_mut(0, &[]).unwrap().add_group_shape().unwrap();
    for image in &images {
        prs.shapes_mut(0, &[0])
            .unwrap()
            .add_picture(image, "a.png", Emu(0), Emu(0), Some(Emu(INCH)), None)
            .unwrap();
    }

    let mut prs = reopen(&prs);
    let group = prs.slide(0).unwrap().shape(0).unwrap();
    assert_eq!(group.kind(), ShapeKind::Group);
    let ids: Vec<u32> = group
        .children()
        .filter_map(|child| child.non_visual_id())
        .collect();
    assert_eq!(ids.len(), 2);
    for (image, id) in images.iter().zip(&ids) {
        assert_eq!(prs.picture_image(0, *id).unwrap().bytes, image.as_slice());
    }

    prs.remove_shape(0, 0).unwrap();
    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    assert!(media_parts(&prs).is_empty());
}

#[test]
fn the_same_image_added_twice_is_stored_once() {
    let image = png(4, 4, [6, 6, 6]);
    let other = png(4, 4, [7, 7, 7]);
    let mut prs = blank_deck(2);
    let a = add_picture(&mut prs, 0, &image, (None, None));
    let b = add_picture(&mut prs, 0, &image, (Some(INCH), None));
    let c = add_picture(&mut prs, 1, &image, (None, None));
    let d = add_picture(&mut prs, 1, &other, (None, None));

    let prs = reopen(&prs);
    let part = |slide, id| prs.picture_image(slide, id).unwrap().part_name;
    assert_eq!(part(0, a), part(0, b));
    assert_eq!(part(0, a), part(1, c));
    assert_ne!(part(0, a), part(1, d));
    assert_eq!(media_parts(&prs).len(), 2);
}

// Picture placeholders

/// The deck after a new slide on `layout`, the new slide's index, and the
/// z-order index of its picture placeholder.
fn with_picture_placeholder_slide(
    mut prs: Presentation,
    layout: &str,
) -> (Presentation, usize, usize) {
    let layout = layout_named(&prs, layout);
    prs.add_slide(layout).unwrap();
    let slide = prs.len() - 1;
    let index = prs
        .slide(slide)
        .unwrap()
        .shapes()
        .position(|shape| shape.placeholder_type() == Some("pic"))
        .expect("the layout gives the slide a picture placeholder");
    (prs, slide, index)
}

#[test]
fn filling_a_picture_placeholder_puts_the_picture_in_its_place() {
    let (mut prs, _, index) = with_picture_placeholder_slide(new_deck(), "Picture with Caption");
    let slide = prs.slide(0).unwrap();
    let placeholder = slide.shape(index).unwrap();
    let id = placeholder.non_visual_id().unwrap();
    let name = placeholder.non_visual_name();
    let count = slide.shapes().len();
    let placeholder_box = prs.effective_geometry(0, &[index]).unwrap();
    assert!(placeholder_box.is_some(), "the placeholder has a box");
    let image = png(120, 60, [0, 90, 200]);
    prs.fill_picture_placeholder(0, &[index], &image, "photo.png")
        .unwrap();

    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    let slide = prs.slide(0).unwrap();
    assert_eq!(slide.shapes().len(), count);
    let picture = slide.shape(index).unwrap();
    assert_eq!(picture.kind(), ShapeKind::Picture);
    assert_eq!(picture.placeholder_type(), Some("pic"));
    assert_eq!(picture.non_visual_id(), Some(id));
    assert_eq!(picture.non_visual_name(), name);
    assert_eq!(
        prs.effective_geometry(0, &[index]).unwrap(),
        placeholder_box,
        "the picture takes the placeholder's box"
    );
    assert_eq!(prs.picture_image(0, id).unwrap().bytes, image.as_slice());
}

#[test]
fn a_placeholder_crops_the_picture_to_its_box_around_the_centre() {
    for (width, height) in [(400, 50), (50, 400)] {
        let (mut prs, _, index) =
            with_picture_placeholder_slide(new_deck(), "Picture with Caption");
        let id = prs
            .slide(0)
            .unwrap()
            .shape(index)
            .unwrap()
            .non_visual_id()
            .unwrap();
        prs.fill_picture_placeholder(0, &[index], &png(width, height, [9, 9, 9]), "p.png")
            .unwrap();
        let prs = reopen(&prs);
        let [left, top, right, bottom] = crop(&prs, 0, id).unwrap();
        if width > height {
            assert!(
                left > 0 && left == right && top == 0 && bottom == 0,
                "{width}x{height}"
            );
        } else {
            assert!(
                top > 0 && top == bottom && left == 0 && right == 0,
                "{width}x{height}"
            );
        }
    }
}

#[test]
fn only_an_empty_picture_placeholder_takes_a_picture() {
    let (mut prs, _, index) = with_picture_placeholder_slide(new_deck(), "Picture with Caption");
    let title = prs
        .slide(0)
        .unwrap()
        .shapes()
        .position(|shape| shape.placeholder_type() == Some("title"))
        .unwrap();
    let image = png(4, 4, [1, 2, 3]);
    let before = prs.to_bytes().unwrap();
    assert!(
        prs.fill_picture_placeholder(0, &[title], &image, "a.png")
            .is_err()
    );
    let error = prs
        .fill_picture_placeholder(0, &[index], b"not an image", "a.png")
        .unwrap_err();
    assert!(matches!(error, Error::UnsupportedPicture { .. }), "{error}");
    assert_eq!(prs.to_bytes().unwrap(), before);

    prs.fill_picture_placeholder(0, &[index], &image, "a.png")
        .unwrap();
    let filled = prs.to_bytes().unwrap();
    assert!(
        prs.fill_picture_placeholder(0, &[index], &image, "a.png")
            .is_err()
    );
    assert_eq!(prs.to_bytes().unwrap(), filled);
}

// Corpus decks

const PRODUCERS: [&str; 4] = ["powerpoint", "google", "libreoffice", "authored"];

/// The "Revenue by quarter" slide of a corpus deck.
const PICTURE_SLIDE: usize = 3;

#[test]
fn the_picture_of_every_corpus_deck_reads_back_with_its_image_and_alt_text() {
    let expected = corpus_png();
    for producer in PRODUCERS {
        let prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let id = only_picture(&prs, PICTURE_SLIDE);
        let read = prs.picture_image(PICTURE_SLIDE, id).unwrap();
        assert_eq!(read.bytes, expected.as_slice(), "{producer}");
        assert_eq!(read.content_type, "image/png", "{producer}");
        let index = shape_index(&prs, PICTURE_SLIDE, id);
        let picture = prs.slide(PICTURE_SLIDE).unwrap().shape(index).unwrap();
        assert_eq!(
            picture.alt_text().as_deref(),
            Some("image.png"),
            "{producer}"
        );
        // Google writes four explicit zero edges, the others none.
        assert_eq!(crop(&prs, PICTURE_SLIDE, id), Some([0; 4]), "{producer}");
        let (width, height) = picture.size().unwrap();
        // LibreOffice rounds the 10 x 5 inch box to its 1/100 mm grid.
        assert!((width.0 - 10 * INCH).abs() < 1_000, "{producer}: {width:?}");
        assert!(
            (height.0 - 5 * INCH).abs() < 1_000,
            "{producer}: {height:?}"
        );
    }
}

#[test]
fn replacing_the_picture_of_a_corpus_deck_keeps_the_rest_of_the_deck() {
    let replacement = png(90, 45, [20, 160, 60]);
    for producer in PRODUCERS {
        let mut prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let texts = slide_texts(&prs);
        let id = only_picture(&prs, PICTURE_SLIDE);
        let index = shape_index(&prs, PICTURE_SLIDE, id);
        let geometry = |prs: &Presentation| {
            let picture = prs.slide(PICTURE_SLIDE).unwrap().shape(index).unwrap();
            (picture.position(), picture.size(), picture.alt_text())
        };
        let before = geometry(&prs);
        prs.replace_picture_image(PICTURE_SLIDE, id, &replacement)
            .unwrap_or_else(|e| panic!("{producer}: {e}"));

        let prs = reopen(&prs);
        assert_eq!(slide_texts(&prs), texts, "{producer}");
        assert_eq!(geometry(&prs), before, "{producer}");
        let read = prs.picture_image(PICTURE_SLIDE, id).unwrap();
        assert_eq!(read.bytes, replacement.as_slice(), "{producer}");
        assert_eq!(
            stored_media(&prs),
            vec![replacement.clone()],
            "{producer}: the old image had no other user"
        );
    }
}

#[test]
fn cropping_the_picture_of_a_corpus_deck_survives_a_save() {
    for producer in ["powerpoint", "google"] {
        let mut prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let id = only_picture(&prs, PICTURE_SLIDE);
        let index = shape_index(&prs, PICTURE_SLIDE, id);
        set_crop(&mut prs, PICTURE_SLIDE, index, [0, 10_000, 0, 20_000]).unwrap();
        let prs = reopen(&prs);
        assert_eq!(
            crop(&prs, PICTURE_SLIDE, id),
            Some([0, 10_000, 0, 20_000]),
            "{producer}"
        );
        assert_eq!(
            prs.picture_image(PICTURE_SLIDE, id).unwrap().bytes,
            corpus_png().as_slice(),
            "{producer}"
        );
    }
}

#[test]
fn removing_the_picture_of_a_corpus_deck_drops_its_image() {
    for producer in PRODUCERS {
        let mut prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let texts = slide_texts(&prs);
        let id = only_picture(&prs, PICTURE_SLIDE);
        prs.remove_shape(PICTURE_SLIDE, shape_index(&prs, PICTURE_SLIDE, id))
            .unwrap_or_else(|e| panic!("{producer}: {e}"));
        let prs = reopen(&prs);
        assert!(media_parts(&prs).is_empty(), "{producer}");
        assert_eq!(slide_texts(&prs), texts, "{producer}");
    }
}

#[test]
fn adding_the_image_a_corpus_deck_already_holds_reuses_its_part() {
    let image = corpus_png();
    for producer in ["powerpoint", "google"] {
        let mut prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let existing = prs
            .picture_image(PICTURE_SLIDE, only_picture(&prs, PICTURE_SLIDE))
            .unwrap()
            .part_name;
        let id = add_picture(&mut prs, 0, &image, (Some(INCH), None));
        let prs = reopen(&prs);
        assert_eq!(
            prs.picture_image(0, id).unwrap().part_name,
            existing,
            "{producer}"
        );
        assert_eq!(media_parts(&prs).len(), 1, "{producer}");
    }
}

#[test]
fn the_picture_placeholder_of_every_corpus_deck_takes_a_picture() {
    let image = png(120, 60, [200, 200, 0]);
    // Google Slides names its layouts by their kind.
    for (producer, layout) in [
        ("powerpoint", "Picture with Caption"),
        ("google", "PICTURE_WITH_CAPTION_TEXT"),
        ("libreoffice", "Picture with Caption"),
        ("authored", "Picture with Caption"),
    ] {
        let deck = corpus_deck(&format!("{producer}/deck.pptx"));
        let (mut prs, slide, index) = with_picture_placeholder_slide(deck, layout);
        let placeholder_box = prs.effective_geometry(slide, &[index]).unwrap();
        prs.fill_picture_placeholder(slide, &[index], &image, "photo.png")
            .unwrap_or_else(|e| panic!("{producer}: {e}"));

        let prs = reopen(&prs);
        let picture = prs.slide(slide).unwrap().shape(index).unwrap();
        assert_eq!(picture.kind(), ShapeKind::Picture, "{producer}");
        assert_eq!(picture.placeholder_type(), Some("pic"), "{producer}");
        assert_eq!(
            prs.effective_geometry(slide, &[index]).unwrap(),
            placeholder_box,
            "{producer}: the picture takes the placeholder's box"
        );
        let id = picture.non_visual_id().unwrap();
        assert_eq!(
            prs.picture_image(slide, id).unwrap().bytes,
            image.as_slice(),
            "{producer}"
        );
    }
}

// Media

#[test]
fn a_video_keeps_its_bytes_and_poster_and_takes_them_away_when_removed() {
    let video = b"\0\0\0\x18ftypisom\0\0\0\0isomiso2 a short clip";
    let poster = png(16, 9, [0, 0, 0]);
    let mut prs = blank_deck(1);
    let id = prs
        .add_media(
            0,
            MediaKind::Video,
            MediaSourceInput::Embedded(EmbeddedMediaInput {
                bytes: video,
                filename: "clip.mp4",
                content_type: "video/mp4",
            }),
            MediaPoster {
                bytes: &poster,
                filename: "poster.png",
            },
            Emu(0),
            Emu(0),
            Emu(4 * INCH),
            Emu(9 * INCH / 4),
            MediaPlaybackSettings::default(),
        )
        .unwrap()
        .non_visual_id()
        .unwrap();

    let prs = reopen(&prs);
    let media = prs.media(0).unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!((media[0].shape_id, media[0].kind), (id, MediaKind::Video));
    assert!(
        matches!(&media[0].source, MediaLocation::Embedded { content_type, .. } if content_type == "video/mp4"),
        "{:?}",
        media[0].source
    );
    assert_eq!(
        prs.extract_media(0, id).unwrap().as_deref(),
        Some(&video[..])
    );
    assert_eq!(prs.picture_image(0, id).unwrap().bytes, poster.as_slice());
    let shape = prs
        .slide(0)
        .unwrap()
        .shape(shape_index(&prs, 0, id))
        .unwrap();
    assert_eq!(shape.shape_type(), Some(ShapeType::Media));

    // Through the media call or as any other shape.
    let mut by_media = prs.clone();
    by_media.remove_media(0, id).unwrap();
    let mut by_shape = prs;
    by_shape
        .remove_shape(0, shape_index(&by_shape, 0, id))
        .unwrap();
    for prs in [reopen(&by_media), reopen(&by_shape)] {
        assert!(prs.media(0).unwrap().is_empty());
        assert!(
            media_parts(&prs).is_empty(),
            "{:?}",
            media_parts(&prs).keys()
        );
    }
}
