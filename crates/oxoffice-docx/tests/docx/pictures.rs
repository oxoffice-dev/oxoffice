//! Pictures and other drawings: inserting a picture inline or floating (size,
//! alt text, crop, position) in the body, a table cell, a header or a footer;
//! reading pictures back, replacing, resizing and removing them; charts and
//! their legend; watermarks; text boxes and background images; the media
//! parts and relationships of the saved package; and the pictures, shapes and
//! charts of the corpus files.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use oxoffice_docx::{
    ChartData, ChartKind, ContentControlTypePayload, Document, DrawingHorizontalAlignment,
    DrawingHorizontalRelativeFrom, DrawingVerticalAlignment, DrawingVerticalRelativeFrom,
    DrawingWrap, Error, HdrFtrType, HeaderFooterKind, ImageWatermarkOptions, LegendPosition,
    LegendSettings, Length, PictureAnchor, PictureCrop, PictureOccurrence, PictureOptions,
    RunItemRef, StoryId, StoryItemKind, StoryKind, StoryRunPosition, TextBoxDirection,
    TextBoxOptions, TextWatermarkOptions,
};
use oxoffice_opc::{OpcPackage, Relationship};

use crate::support::{doc_with, doc_with_body, reopen, texts};

const EMU_PER_INCH: i64 = 914_400;
const IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

// ---- Builders ----

/// The CRC-32 a PNG chunk ends with, over its type and data.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A valid grey PNG of `width` by `height` pixels that declares no
/// resolution. `grey` is the pixel value, so pictures built with different
/// values differ in their bytes.
fn png(width: u32, height: u32, grey: u8) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let crc = crc32(&out[start..]);
        out.extend(crc.to_be_bytes());
    }
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    // 8-bit greyscale, deflate, no filter method, no interlace.
    header.extend([8, 0, 0, 0, 0]);
    let mut rows = Vec::new();
    for _ in 0..height {
        rows.push(0);
        rows.extend(std::iter::repeat_n(grey, width as usize));
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(
        &mut out,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6),
    );
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A JPEG of `width` by `height` pixels at `dpi` dots per inch, as far as a
/// reader measures it: a JFIF density, then a baseline frame header.
fn jpeg(width: u16, height: u16, dpi: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    out.extend(b"JFIF\0");
    // Version 1.1, density in dots per inch.
    out.extend([1, 1, 1]);
    out.extend(dpi.to_be_bytes());
    out.extend(dpi.to_be_bytes());
    out.extend([0, 0]);
    // Baseline frame: 8 bits, height, width, one component.
    out.extend([0xFF, 0xC0, 0x00, 0x0B, 8]);
    out.extend(height.to_be_bytes());
    out.extend(width.to_be_bytes());
    out.extend([1, 1, 0x11, 0]);
    out.extend([0xFF, 0xD9]);
    out
}

/// The body story of a document.
fn body(doc: &Document) -> StoryId {
    doc.stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .expect("a body story")
}

/// The default header of the first section.
fn header_story(doc: &Document) -> StoryId {
    doc.section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap()
        .expect("a default header")
        .story()
        .clone()
}

/// The texts of the paragraphs of a story, in order: a picture paragraph
/// reads as an empty line.
fn story_lines(doc: &Document, story: &StoryId) -> Vec<String> {
    doc.story_items(story)
        .unwrap()
        .iter()
        .filter(|item| item.kind() == StoryItemKind::Paragraph)
        .map(|item| item.text().unwrap().unwrap_or_default())
        .collect()
}

/// The pictures of the headers, in order.
fn header_pictures(doc: &Document) -> Vec<PictureOccurrence> {
    doc.pictures()
        .unwrap()
        .into_iter()
        .filter(|picture| picture.location.story().kind() == StoryKind::Header)
        .collect()
}

/// Picture options with only a size.
fn sized(width: Length, height: Length) -> PictureOptions {
    PictureOptions {
        width,
        height,
        crop: None,
        anchor: None,
        name: None,
        description: None,
        title: None,
        decorative: false,
    }
}

/// A floating placement at the top left of the column, in front of the
/// text, with no wrapping and no distances.
fn floating() -> PictureAnchor {
    PictureAnchor {
        horizontal_relative_from: DrawingHorizontalRelativeFrom::Column,
        horizontal_offset: Length::emu(0),
        horizontal_alignment: None,
        vertical_relative_from: DrawingVerticalRelativeFrom::Paragraph,
        vertical_offset: Length::emu(0),
        vertical_alignment: None,
        wrap: DrawingWrap::None,
        distance_top: Length::emu(0),
        distance_bottom: Length::emu(0),
        distance_left: Length::emu(0),
        distance_right: Length::emu(0),
        relative_height: 1,
        behind_text: false,
    }
}

/// What the runs of body paragraph `index` show, in order: each text, and
/// each drawing as `[kind inline|floating width x height]` in EMU.
fn shown(doc: &Document, index: usize) -> Vec<String> {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    let mut out = Vec::new();
    for run in paragraph.runs() {
        for item in run.items() {
            match item {
                RunItemRef::Text(text) => out.push(text.to_owned()),
                RunItemRef::Drawing(drawing) => out.push(format!(
                    "[{:?} {} {}x{}]",
                    drawing.kind(),
                    if drawing.is_inline() {
                        "inline"
                    } else {
                        "floating"
                    },
                    drawing.width().map_or(0, |width| width.to_emu()),
                    drawing.height().map_or(0, |height| height.to_emu()),
                )),
                _ => {}
            }
        }
    }
    out
}

/// The root of a raw header or footer part (`w:hdr` or `w:ftr`) holding
/// `children`, with the namespaces a picture paragraph needs.
fn raw_story(root: &str, children: &str) -> Vec<u8> {
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<w:{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#,
            r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
            r#" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
            r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#,
            r#" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
            "{children}</w:{root}>"
        ),
        root = root,
        children = children
    )
    .into_bytes()
}

/// A paragraph holding one inline picture of `cx` by `cy` EMU whose blip
/// names `relationship`, as a template's header holds it.
fn raw_picture_paragraph(relationship: &str, id: u32, cx: i64, cy: i64) -> String {
    format!(
        concat!(
            r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">"#,
            r#"<wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{id}" name="Picture {id}"/>"#,
            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
            r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="Picture {id}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
            r#"<pic:blipFill><a:blip r:embed="{relationship}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
            r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"#,
            r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        ),
        relationship = relationship,
        id = id,
        cx = cx,
        cy = cy
    )
}

/// Where the linked picture of `doc_with_linked_picture` points, as Word
/// writes a picture inserted with "Link to File".
const LINKED_ADDRESS: &str = "file:///C:/Images/logo.png";

/// A document whose first paragraph reads "Logos: ", then shows a one-inch
/// picture linked to `LINKED_ADDRESS` (no media part), then reads " end".
fn doc_with_linked_picture() -> Document {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Logos: </w:t></w:r><w:r><w:drawing>"#,
        r#"<wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
        r#" distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/>"#,
        r#"<wp:docPr id="1" name="Linked logo"/>"#,
        r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
        r#"<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
        r#"<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
        r#"<pic:nvPicPr><pic:cNvPr id="1" name="logo.png"/><pic:cNvPicPr/></pic:nvPicPr>"#,
        r#"<pic:blipFill><a:blip xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
        r#" r:link="rIdLinked"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
        r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>"#,
        r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"#,
        r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#,
        r#"<w:r><w:t xml:space="preserve"> end</w:t></w:r></w:p>"#
    ))
    .to_bytes()
    .expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(doc)).expect("package opens");
    package
        .get_or_create_part_rels("/word/document.xml")
        .items
        .push(Relationship {
            id: "rIdLinked".into(),
            rel_type: IMAGE.into(),
            target: LINKED_ADDRESS.into(),
            target_mode: Some("External".into()),
        });
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with a linked picture opens")
}

/// The values of one attribute in XML, in document order: `values(xml, "r:embed")`.
fn values<'a>(xml: &'a str, attribute: &str) -> Vec<&'a str> {
    let pattern = format!(" {attribute}=\"");
    xml.split(&pattern)
        .skip(1)
        .map(|tail| tail.split('"').next().expect("a closed attribute value"))
        .collect()
}

/// The first `name` element of XML, from its start tag to its end tag.
/// `v:shape` is not `v:shapetype`.
fn element<'a>(xml: &'a str, name: &str) -> &'a str {
    let open = format!("<{name}");
    let start = xml
        .match_indices(&open)
        .map(|(at, _)| at)
        .find(|at| matches!(xml.as_bytes()[at + open.len()], b' ' | b'>' | b'/'))
        .unwrap_or_else(|| panic!("no <{name}> in {xml}"));
    let end_tag = format!("</{name}>");
    let end = start
        + xml[start..]
            .find(&end_tag)
            .unwrap_or_else(|| panic!("<{name}> is not closed"))
        + end_tag.len();
    &xml[start..end]
}

/// Every ZIP entry of a package, by name, as stored.
fn zip_entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).expect("ZIP entry reads");
            let mut data = Vec::new();
            entry.read_to_end(&mut data).expect("ZIP entry inflates");
            (entry.name().expect("ZIP entry name").into_owned(), data)
        })
        .collect()
}

/// A saved document as the next reader sees it: every ZIP entry as stored,
/// and the relationships and content types the package layer reads from
/// them.
struct Saved {
    entries: BTreeMap<String, Vec<u8>>,
    package: OpcPackage,
}

impl Saved {
    fn of(doc: &mut Document) -> Self {
        Self::read(&doc.to_bytes().expect("document saves"))
    }

    fn read(bytes: &[u8]) -> Self {
        Self {
            entries: zip_entries(bytes),
            package: OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens"),
        }
    }

    /// The bytes of a part, named with or without its leading slash.
    fn part(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .get(name.trim_start_matches('/'))
            .map(Vec::as_slice)
    }

    fn xml(&self, name: &str) -> String {
        let bytes = self
            .part(name)
            .unwrap_or_else(|| panic!("{name} is not saved"));
        String::from_utf8(bytes.to_vec()).expect("XML part is UTF-8")
    }

    /// The media entries, by name.
    fn media(&self) -> BTreeMap<&str, &[u8]> {
        self.entries
            .iter()
            .filter(|(name, _)| name.starts_with("word/media/"))
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect()
    }

    fn content_type(&self, part: &str) -> Option<&str> {
        let part = format!("/{}", part.trim_start_matches('/'));
        self.package.content_types.content_type_for(&part)
    }

    /// The relationships of one type a part owns: id to target part name.
    fn relationships(&self, owner: &str, kind: &str) -> BTreeMap<String, String> {
        let owner = format!("/{}", owner.trim_start_matches('/'));
        self.package
            .get_part_rels(&owner)
            .map(|relationships| {
                relationships
                    .items
                    .iter()
                    .filter(|relationship| relationship.rel_type == kind)
                    .map(|relationship| {
                        (
                            relationship.id.clone(),
                            OpcPackage::resolve_rel_target(&owner, &relationship.target),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The bytes each `r:embed` of a part shows, in document order, each
    /// found through that part's own image relationships.
    fn shown_images(&self, owner: &str) -> Vec<Vec<u8>> {
        let relationships = self.relationships(owner, IMAGE);
        values(&self.xml(owner), "r:embed")
            .into_iter()
            .map(|id| {
                let target = relationships
                    .get(id)
                    .unwrap_or_else(|| panic!("{owner}: r:embed {id} names no image relationship"));
                self.part(target)
                    .unwrap_or_else(|| panic!("{owner}: {id} targets {target}, which is not saved"))
                    .to_vec()
            })
            .collect()
    }
}

// ---- Inserting and reading back ----

#[test]
fn a_picture_at_native_size_takes_the_resolution_the_image_declares_or_72_dpi() {
    let mut doc = Document::new();
    // 144 x 72 pixels that declare no resolution: 2 x 1 inches at 72 dpi.
    doc.add_picture_auto(&png(144, 72, 0), "plain.png").unwrap();
    // 288 x 144 pixels declared at 144 dpi: 2 x 1 inches too.
    let photo = jpeg(288, 144, 144);
    doc.add_picture_auto(&photo, "photo.jpg").unwrap();
    let mut doc = reopen(&mut doc);

    let pictures: Vec<_> = doc
        .pictures()
        .unwrap()
        .into_iter()
        .map(|picture| (picture.width, picture.height, picture.content_type))
        .collect();
    assert_eq!(
        pictures,
        [
            (2 * EMU_PER_INCH, EMU_PER_INCH, "image/png".to_owned()),
            (2 * EMU_PER_INCH, EMU_PER_INCH, "image/jpeg".to_owned()),
        ]
    );
    // The JPEG is stored as a JPEG, under its own extension and content type.
    let saved = Saved::of(&mut doc);
    let (name, _) = saved
        .media()
        .into_iter()
        .find(|(_, bytes)| *bytes == photo.as_slice())
        .expect("the JPEG is saved");
    assert!(name.ends_with(".jpeg") || name.ends_with(".jpg"), "{name}");
    assert_eq!(saved.content_type(name), Some("image/jpeg"));
}

#[test]
fn a_picture_that_cannot_be_written_is_refused_without_a_change() {
    let mut doc = doc_with(&["unchanged"]);
    let before = doc.to_bytes().unwrap();
    let image = png(2, 2, 0);
    let inch = Length::inches(1.0);

    match doc.add_picture_auto(b"not an image", "broken.bin") {
        Err(Error::UnavailableImageDimensions { filename }) => assert_eq!(filename, "broken.bin"),
        other => panic!(
            "expected unavailable dimensions, got {:?}",
            other.map(|_| ())
        ),
    }
    assert_eq!(doc.to_bytes().unwrap(), before, "add_picture_auto");

    let story = body(&doc);
    assert!(
        doc.add_picture_to_story(&story, b"not an image", "fake.png", inch, inch)
            .is_err()
    );
    let position = StoryRunPosition {
        location: doc.paragraph_story_location(0).unwrap().unwrap(),
        run_index: 0,
    };
    assert!(
        doc.insert_picture_at(&position, b"\x89PNG\r\n\x1a\ncut", "cut.png", None, None)
            .is_err()
    );
    // One side given and the other left to the image.
    assert!(
        doc.insert_picture_to_story(&story, None, &image, "p.png", Some(inch), None)
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before, "story picture calls");

    let crop = |left, top, right, bottom| PictureOptions {
        crop: Some(PictureCrop {
            left,
            top,
            right,
            bottom,
        }),
        ..sized(inch, inch)
    };
    let cases = [
        ("a zero width", sized(Length::emu(0), inch)),
        (
            "a control character in the title",
            PictureOptions {
                title: Some("bad\u{1}title".into()),
                ..sized(inch, inch)
            },
        ),
        ("a crop over the whole width", crop(60_000, 0, 40_000, 0)),
        ("a negative crop", crop(0, -1, 0, 0)),
        (
            "a negative wrap distance",
            PictureOptions {
                anchor: Some(PictureAnchor {
                    distance_left: Length::pt(-1.0),
                    ..floating()
                }),
                ..sized(inch, inch)
            },
        ),
    ];
    for (case, options) in cases {
        let story = body(&doc);
        assert!(
            doc.add_picture_with_options(&story, &image, "p.png", options)
                .is_err(),
            "{case} is accepted"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{case} changed the file");
    }
    assert!(doc.pictures().unwrap().is_empty());
}

// ---- Alt text and placement ----

#[test]
fn a_floating_picture_keeps_its_position_wrap_distances_and_crop() {
    let mut doc = doc_with(&["Text that flows around the picture."]);
    let story = body(&doc);
    let options = PictureOptions {
        crop: Some(PictureCrop {
            left: 10_000,
            top: 5_000,
            right: 20_000,
            bottom: 0,
        }),
        anchor: Some(PictureAnchor {
            horizontal_relative_from: DrawingHorizontalRelativeFrom::Margin,
            horizontal_offset: Length::pt(18.0),
            vertical_offset: Length::pt(6.0),
            wrap: DrawingWrap::Square,
            distance_top: Length::pt(2.0),
            distance_bottom: Length::pt(3.0),
            distance_left: Length::pt(4.0),
            distance_right: Length::pt(5.0),
            relative_height: 7,
            ..floating()
        }),
        ..sized(Length::pt(72.0), Length::pt(36.0))
    };
    doc.add_picture_with_options(&story, &png(4, 4, 30), "float.png", options)
        .unwrap();
    let mut doc = reopen(&mut doc);

    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert!(pictures[0].anchored);
    assert_eq!((pictures[0].width, pictures[0].height), (914_400, 457_200));
    assert!(doc.images()[0].is_anchor);
    assert_eq!(shown(&doc, 1), ["[Image floating 914400x457200]"]);
    // No reader gives the position back: the saved drawing carries it, in
    // EMU (12,700 a point) and thousandths of a percent.
    let xml = Saved::of(&mut doc).xml("word/document.xml");
    let anchor = element(&xml, "wp:anchor");
    for expected in [
        r#"<wp:positionH relativeFrom="margin"><wp:posOffset>228600</wp:posOffset></wp:positionH>"#,
        r#"<wp:positionV relativeFrom="paragraph"><wp:posOffset>76200</wp:posOffset></wp:positionV>"#,
        "<wp:wrapSquare ",
        r#"distT="25400""#,
        r#"distB="38100""#,
        r#"distL="50800""#,
        r#"distR="63500""#,
        r#"relativeHeight="7""#,
        r#"behindDoc="0""#,
        r#"<a:srcRect l="10000" t="5000" r="20000" b="0"/>"#,
    ] {
        assert!(anchor.contains(expected), "{expected} is not in {anchor}");
    }
}

/// Where the layout draws each text of `doc`, first occurrence, as (x, y).
fn text_origins(doc: &Document) -> BTreeMap<String, (f64, f64)> {
    let result = doc.layout().unwrap();
    let mut origins = BTreeMap::new();
    for page in &result.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element {
                origins
                    .entry(run.text.trim().to_owned())
                    .or_insert((run.origin.x, run.origin.y));
            }
        });
    }
    origins
}

#[test]
fn a_clearing_break_restarts_the_text_below_the_floats_on_its_side() {
    // A square-wrapped picture, an inch square at the left or the right of
    // the column, then a paragraph beside it whose break says where "after"
    // restarts: below the picture, or on the next line beside it.
    let margin_left = 72.0;
    let picture_bottom = 72.0 + 72.0;
    for (side, alignment) in [
        ("left", DrawingHorizontalAlignment::Left),
        ("right", DrawingHorizontalAlignment::Right),
    ] {
        let mut doc = Document::new();
        let story = body(&doc);
        let options = PictureOptions {
            anchor: Some(PictureAnchor {
                horizontal_alignment: Some(alignment),
                wrap: DrawingWrap::Square,
                ..floating()
            }),
            ..sized(Length::pt(72.0), Length::pt(72.0))
        };
        doc.add_picture_with_options(&story, &png(4, 4, 30), "float.png", options)
            .unwrap();
        let bytes = doc.to_bytes().unwrap();
        for clear in ["left", "right", "all", "none"] {
            let mut package = OpcPackage::from_reader(Cursor::new(bytes.clone())).unwrap();
            let part = package.main_document_part().unwrap();
            let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
            let at = xml.find("<w:sectPr").unwrap();
            let xml = format!(
                concat!(
                    r#"{}<w:p><w:r><w:t>beside</w:t>"#,
                    r#"<w:br w:type="textWrapping" w:clear="{}"/><w:t>after</w:t></w:r></w:p>{}"#,
                ),
                &xml[..at],
                clear,
                &xml[at..]
            );
            package.set_part(&part, xml.into_bytes());
            let mut out = Cursor::new(Vec::new());
            package.write_to(&mut out).unwrap();
            let doc = Document::from_bytes(&out.into_inner()).unwrap();

            let origins = text_origins(&doc);
            let (beside, after) = (origins["beside"], origins["after"]);
            let case = format!("picture on the {side}, clear {clear}: {origins:?}");
            assert!(beside.1 < picture_bottom, "{case}");
            if clear == side || clear == "all" {
                assert!(after.1 > picture_bottom, "below the picture: {case}");
                assert!((after.0 - margin_left).abs() < 0.01, "{case}");
            } else {
                assert!(after.1 < picture_bottom, "beside the picture: {case}");
                assert!(after.1 > beside.1, "on the next line: {case}");
            }
            if side == "left" {
                assert!(beside.0 >= margin_left + 72.0, "{case}");
            }
        }
    }
}

#[test]
fn an_anchor_without_side_distances_keeps_text_an_eighth_of_an_inch_away() {
    // Word reads an absent `distL` or `distR` as 114,300 EMU, 9 points, so
    // oxoffice writes every distance, zero included.
    let mut doc = Document::new();
    let story = body(&doc);
    let options = PictureOptions {
        anchor: Some(PictureAnchor {
            wrap: DrawingWrap::Square,
            ..floating()
        }),
        ..sized(Length::pt(72.0), Length::pt(72.0))
    };
    doc.add_picture_with_options(&story, &png(4, 4, 30), "float.png", options)
        .unwrap();
    doc.add_paragraph("beside");
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    assert!(
        xml.contains(r#"distT="0" distB="0" distL="0" distR="0""#),
        "{xml}"
    );
    let zero = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
    assert!((text_origins(&zero)["beside"].0 - 144.0).abs() < 0.01);

    // The same anchor without its distances, as another producer may write it.
    let xml = xml.replace(r#" distT="0" distB="0" distL="0" distR="0""#, "");
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    assert!((text_origins(&doc)["beside"].0 - 153.0).abs() < 0.01);
    // An edit elsewhere keeps the anchor as it was, so Word reads the same.
    assert_eq!(doc.replace_text("beside", "Beside"), 1);
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("distL="), "{xml}");
    assert!((text_origins(&doc)["Beside"].0 - 153.0).abs() < 0.01);
    // Even a resize of the picture keeps its anchor's source bytes.
    let mut doc = doc;
    let relationship = doc.pictures().unwrap().remove(0).relationship_id;
    assert_eq!(
        doc.set_picture_size(&relationship, Length::pt(72.0), Length::pt(72.0))
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("distL="), "{xml}");
    assert!((text_origins(&doc)["Beside"].0 - 153.0).abs() < 0.01);
}

#[test]
fn a_floating_picture_can_be_aligned_and_sent_behind_the_text() {
    let (aligned, front, back) = (png(2, 2, 1), png(2, 2, 2), png(2, 2, 3));
    let mut doc = doc_with(&["Text"]);
    let story = body(&doc);
    let options = PictureOptions {
        anchor: Some(PictureAnchor {
            horizontal_alignment: Some(DrawingHorizontalAlignment::Right),
            vertical_relative_from: DrawingVerticalRelativeFrom::Page,
            vertical_alignment: Some(DrawingVerticalAlignment::Bottom),
            wrap: DrawingWrap::Tight,
            behind_text: true,
            ..floating()
        }),
        ..sized(Length::inches(1.0), Length::inches(1.0))
    };
    doc.add_picture_with_options(&story, &aligned, "aligned.png", options)
        .unwrap();
    // The older call puts its picture first in the body.
    doc.add_anchored_image(
        &front,
        "front.png",
        Length::inches(1.0),
        Length::inches(0.5),
        false,
    );
    doc.add_anchored_image(
        &back,
        "back.png",
        Length::inches(1.0),
        Length::inches(0.5),
        true,
    );
    let mut doc = reopen(&mut doc);

    let pictures = doc.pictures().unwrap();
    let order: Vec<_> = pictures
        .iter()
        .map(|picture| picture.bytes.clone())
        .collect();
    assert_eq!(order, [back, front, aligned]);
    assert!(pictures.iter().all(|picture| picture.anchored));
    let sizes: Vec<_> = pictures
        .iter()
        .take(2)
        .map(|picture| (picture.width, picture.height))
        .collect();
    assert_eq!(sizes, [(EMU_PER_INCH, EMU_PER_INCH / 2); 2]);
    let xml = Saved::of(&mut doc).xml("word/document.xml");
    assert_eq!(values(&xml, "behindDoc"), ["1", "0", "1"]);
    let anchor = &xml[xml.rfind("<wp:anchor").unwrap()..];
    for expected in [
        r#"<wp:positionH relativeFrom="column"><wp:align>right</wp:align></wp:positionH>"#,
        r#"<wp:positionV relativeFrom="page"><wp:align>bottom</wp:align></wp:positionV>"#,
        "<wp:wrapTight ",
    ] {
        assert!(anchor.contains(expected), "{expected} is not in {anchor}");
    }
}

#[test]
fn a_picture_inserted_between_two_runs_sits_between_their_texts() {
    let mut doc = Document::new();
    doc.add_paragraph("left ").add_run("right");
    let position = StoryRunPosition {
        location: doc.paragraph_story_location(0).unwrap().unwrap(),
        run_index: 1,
    };
    doc.insert_picture_at(&position, &png(3, 2, 50), "mid.png", None, None)
        .unwrap();
    let doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["left right"]);
    // 3 x 2 pixels at 72 dpi: 1/24 x 1/36 inch.
    assert_eq!(
        shown(&doc, 0),
        ["left ", "[Image inline 38100x25400]", "right"]
    );
}

#[test]
fn a_picture_inserted_after_a_paragraph_lands_right_after_it() {
    let mut doc = doc_with(&["first", "second"]);
    let after = doc.paragraph_story_location(0).unwrap().unwrap();
    let story = body(&doc);
    doc.insert_picture_to_story(
        &story,
        Some(&after),
        &png(3, 2, 60),
        "p.png",
        Some(Length::inches(1.5)),
        Some(Length::inches(1.0)),
    )
    .unwrap();
    let doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["first", "", "second"]);
    assert_eq!(shown(&doc, 1), ["[Image inline 1371600x914400]"]);
}

#[test]
fn a_picture_inserted_after_a_linked_picture_is_the_occurrence_returned() {
    let seal = png(3, 3, 23);
    let mut doc = doc_with_linked_picture();
    let returned = doc
        .insert_picture_with_options_at(
            &StoryRunPosition {
                location: doc.paragraph_story_location(0).unwrap().unwrap(),
                run_index: 2,
            },
            &seal,
            "seal.png",
            Some(Length::inches(1.0)),
            Some(Length::inches(1.0)),
            Some(PictureOptions {
                description: Some("Seal".into()),
                ..sized(Length::inches(1.0), Length::inches(1.0))
            }),
        )
        .unwrap();
    assert!(
        returned.external_url.is_none() && returned.bytes == seal,
        "the occurrence returned is not the picture inserted: {:?} {:?}",
        returned.external_url,
        returned.description
    );
    let doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 2);
    assert_eq!(returned, pictures[1]);
    assert_eq!(returned.ordinal, 1);
    assert_eq!(returned.description.as_deref(), Some("Seal"));
    assert!(pictures[0].external_url.is_some(), "the link stays first");
}

#[test]
fn a_floating_picture_with_alt_text_inserted_after_a_header_paragraph_reads_back_where_returned() {
    let logo = png(4, 2, 33);
    let mut doc = doc_with(&["Body"]);
    doc.set_raw_header_with_images(
        raw_story(
            "hdr",
            "<w:p><w:r><w:t>Northwind</w:t></w:r></w:p><w:p><w:r><w:t>Second line</w:t></w:r></w:p>",
        ),
        &[],
        HdrFtrType::Default,
    );
    let header = header_story(&doc);
    let first = doc.story_items(&header).unwrap()[0].location().clone();
    let options = PictureOptions {
        description: Some("Northwind logo".into()),
        title: Some("Logo".into()),
        anchor: Some(PictureAnchor {
            horizontal_relative_from: DrawingHorizontalRelativeFrom::Margin,
            horizontal_alignment: Some(DrawingHorizontalAlignment::Right),
            behind_text: true,
            ..floating()
        }),
        ..sized(Length::inches(1.0), Length::inches(0.5))
    };
    let returned = doc
        .insert_picture_to_story_with_options(
            &header,
            Some(&first),
            &logo,
            "logo.png",
            Some(Length::inches(1.0)),
            Some(Length::inches(0.5)),
            Some(options),
        )
        .unwrap();
    let mut doc = reopen(&mut doc);

    let pictures = header_pictures(&doc);
    assert_eq!(pictures.len(), 1);
    let picture = &pictures[0];
    assert_eq!(
        picture.location, returned,
        "the location returned holds the picture"
    );
    assert_eq!(picture.bytes, logo);
    assert!(picture.anchored);
    assert_eq!(picture.description.as_deref(), Some("Northwind logo"));
    assert_eq!(picture.title.as_deref(), Some("Logo"));
    assert_eq!((picture.width, picture.height), (914_400, 457_200));
    let header = header_story(&doc);
    assert_eq!(story_lines(&doc, &header), ["Northwind", "", "Second line"]);
    let part = header.part_name().to_owned();
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images(&part), [logo]);
    let anchor = element(&saved.xml(&part), "wp:anchor").to_owned();
    for expected in [
        r#"<wp:positionH relativeFrom="margin"><wp:align>right</wp:align></wp:positionH>"#,
        r#"behindDoc="1""#,
    ] {
        assert!(anchor.contains(expected), "{expected} is not in {anchor}");
    }
    assert!(saved.relationships("word/document.xml", IMAGE).is_empty());
}

#[test]
fn a_footer_picture_is_related_from_the_footer_part() {
    let image = png(5, 1, 80);
    let mut doc = doc_with(&["Body"]);
    doc.set_footer_image(
        &image,
        "signature.png",
        Length::inches(2.5),
        Length::inches(0.5),
    );
    let mut doc = reopen(&mut doc);

    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    let picture = &pictures[0];
    let story = picture.location.story();
    assert_eq!(story.kind(), StoryKind::Footer);
    assert_eq!((picture.width, picture.height), (2_286_000, 457_200));
    assert_eq!(
        doc.image_data_for_story(story, &picture.relationship_id)
            .unwrap(),
        image
    );
    // The main part has no such relationship: the footer part owns it.
    assert_eq!(doc.image_data(&picture.relationship_id), None);
    let footer = story.part_name().to_owned();
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images(&footer), [image]);
    assert!(saved.relationships("word/document.xml", IMAGE).is_empty());
}

#[test]
fn a_header_picture_is_replaced_through_its_header_story() {
    let (logo, new_logo) = (png(4, 2, 90), png(6, 3, 100));
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Northwind");
    let header = header_story(&doc);
    doc.add_picture_to_story(
        &header,
        &logo,
        "logo.png",
        Length::inches(1.0),
        Length::inches(0.5),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    let picture = header_pictures(&doc).remove(0);
    assert_eq!(picture.bytes, logo);

    doc.replace_image_for_story(
        picture.location.story(),
        &picture.relationship_id,
        &new_logo,
    )
    .unwrap();
    let mut doc = reopen(&mut doc);

    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].location.story().kind(), StoryKind::Header);
    assert_eq!(pictures[0].bytes, new_logo);
    assert_eq!((pictures[0].width, pictures[0].height), (914_400, 457_200));
    let header = header_story(&doc);
    let first = doc.story_items(&header).unwrap()[0].text().unwrap();
    assert_eq!(first.as_deref(), Some("Northwind"));
    let saved = Saved::of(&mut doc);
    let media: Vec<_> = saved.media().into_values().collect();
    assert_eq!(media, [new_logo.as_slice()], "the old logo is gone");
}

// ---- Header and footer image calls ----

#[test]
fn a_first_page_header_image_shows_on_the_first_page_only() {
    let cover = png(6, 3, 44);
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Every page");
    doc.set_first_page_header_image(
        &cover,
        "cover.png",
        Length::inches(2.0),
        Length::inches(1.0),
    );
    let mut doc = reopen(&mut doc);

    assert_eq!(doc.section(0).unwrap().different_first_page(), Some(true));
    let first = doc
        .section_story(0, HeaderFooterKind::Header, HdrFtrType::First)
        .unwrap()
        .expect("a first-page header");
    let pictures = header_pictures(&doc);
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].location.story(), first.story());
    assert_eq!(pictures[0].bytes, cover);
    assert_eq!(
        (pictures[0].width, pictures[0].height),
        (1_828_800, 914_400)
    );
    assert_eq!(doc.header_text().as_deref(), Some("Every page"));
    let first_part = first.story().part_name().to_owned();
    let default_part = header_story(&doc).part_name().to_owned();
    assert_ne!(first_part, default_part);
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images(&first_part), [cover]);
    assert!(saved.shown_images(&default_part).is_empty());
    assert!(saved.relationships("word/document.xml", IMAGE).is_empty());
}

#[test]
fn a_header_image_with_a_background_shades_its_paragraph() {
    let banner = png(8, 1, 55);
    let mut doc = doc_with(&["Body"]);
    doc.set_header_image_with_background(
        &banner,
        "banner.png",
        Length::inches(6.0),
        Length::inches(0.75),
        "1F3864",
    );
    let mut doc = reopen(&mut doc);

    let pictures = header_pictures(&doc);
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].bytes, banner);
    assert_eq!(
        (pictures[0].width, pictures[0].height),
        (5_486_400, 685_800)
    );
    let part = header_story(&doc).part_name().to_owned();
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images(&part), [banner]);
    // No reader gives a header paragraph's shading back.
    let xml = saved.xml(&part);
    let shading = element(&xml, "w:pPr");
    assert!(shading.contains(r#"w:fill="1F3864""#), "{shading}");
    assert_eq!(values(&xml, "w:fill"), ["1F3864"]);
}

#[test]
fn a_raw_footer_shows_each_given_image_through_its_own_relationships() {
    let (signature, stamp) = (png(5, 1, 99), png(2, 2, 111));
    let footer = raw_story(
        "ftr",
        &format!(
            "<w:p><w:r><w:t>Signed</w:t></w:r></w:p>{}{}",
            raw_picture_paragraph("rIdSignature", 1, 2_286_000, 457_200),
            raw_picture_paragraph("rId7", 2, 457_200, 457_200)
        ),
    );
    let mut doc = doc_with(&["Body"]);
    doc.set_raw_footer_with_images(
        footer,
        &[
            ("rId7", &stamp, "stamp.png"),
            ("rIdSignature", &signature, "signature.png"),
        ],
        HdrFtrType::Default,
    );
    let mut doc = reopen(&mut doc);

    let footer = doc
        .section_story(0, HeaderFooterKind::Footer, HdrFtrType::Default)
        .unwrap()
        .expect("a default footer")
        .story()
        .clone();
    assert_eq!(story_lines(&doc, &footer), ["Signed", "", ""]);
    let pictures = doc.pictures().unwrap();
    let in_footer: Vec<_> = pictures
        .iter()
        .filter(|picture| picture.location.story().kind() == StoryKind::Footer)
        .map(|picture| (picture.bytes.clone(), picture.width))
        .collect();
    assert_eq!(
        in_footer,
        [(signature.clone(), 2_286_000), (stamp.clone(), 457_200)]
    );
    assert_eq!(pictures.len(), 2, "no picture elsewhere");
    assert_eq!(pictures[0].location.story(), &footer);
    let part = footer.part_name().to_owned();
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images(&part), [signature, stamp]);
    assert!(saved.relationships("word/document.xml", IMAGE).is_empty());
}

// ---- Replacing and resizing ----

#[test]
fn replacing_a_png_with_a_jpeg_stores_it_as_a_jpeg() {
    let photo = jpeg(40, 30, 96);
    let mut doc = doc_with(&["Photo"]);
    doc.add_picture(
        &png(4, 3, 5),
        "draft.png",
        Length::inches(4.0),
        Length::inches(3.0),
    );
    let mut doc = reopen(&mut doc);
    let relationship = doc.pictures().unwrap()[0].relationship_id.clone();

    doc.replace_image(&relationship, &photo).unwrap();
    let mut doc = reopen(&mut doc);

    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures[0].bytes, photo);
    assert_eq!(pictures[0].content_type, "image/jpeg");
    let saved = Saved::of(&mut doc);
    assert_eq!(saved.shown_images("word/document.xml"), [photo]);
    let media = saved.media();
    assert_eq!(media.len(), 1, "{:?}", media.keys());
    let (name, _) = media.first_key_value().unwrap();
    assert!(name.ends_with(".jpeg") || name.ends_with(".jpg"), "{name}");
    assert_eq!(saved.content_type(name), Some("image/jpeg"));
}

#[test]
fn resizing_one_occurrence_leaves_the_pictures_that_share_its_media_alone() {
    let image = png(2, 2, 42);
    let mut doc = doc_with(&["Pictures"]);
    let relationship = doc.embed_image(&image, "shared.png");
    for _ in 0..2 {
        doc.add_paragraph("").add_run("").add_picture(
            &relationship,
            Length::inches(1.0),
            Length::inches(1.0),
        );
    }
    let mut doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();

    doc.set_picture_size_at(&pictures[1], Length::inches(2.0), Length::inches(0.5))
        .unwrap();
    // The occurrence read before the change no longer matches the document.
    let changed = doc.to_bytes().unwrap();
    assert!(
        doc.set_picture_size_at(&pictures[1], Length::inches(3.0), Length::inches(0.5))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), changed);
    let doc = reopen(&mut doc);

    let after: Vec<_> = doc
        .pictures()
        .unwrap()
        .iter()
        .map(|picture| (picture.width, picture.height, picture.bytes == image))
        .collect();
    assert_eq!(
        after,
        [(914_400, 914_400, true), (1_828_800, 457_200, true)]
    );
}

#[test]
fn replacing_or_resizing_with_invalid_input_is_refused_without_a_change() {
    let mut doc = doc_with(&["Figure"]);
    doc.add_picture(
        &png(2, 2, 7),
        "p.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let mut doc = reopen(&mut doc);
    let picture = doc.pictures().unwrap().remove(0);
    let relationship = picture.relationship_id.as_str();
    let before = doc.to_bytes().unwrap();
    let inch = Length::inches(1.0);

    assert!(doc.replace_image(relationship, b"not an image").is_err());
    assert!(doc.replace_image("rId404", &png(1, 1, 1)).is_err());
    assert!(
        doc.set_picture_size(relationship, Length::emu(0), inch)
            .is_err()
    );
    assert!(doc.set_picture_size("rId404", inch, inch).is_err());
    assert!(
        doc.set_picture_size_at(&picture, inch, Length::inches(-1.0))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
}

// ---- Removing ----

// ---- Background images and text boxes ----

#[test]
fn a_text_box_shows_its_text_after_its_anchor_paragraph() {
    let mut doc = doc_with(&["Body"]);
    let story = body(&doc);
    let options = TextBoxOptions {
        width: Length::inches(2.0),
        height: Length::inches(1.0),
        anchor: PictureAnchor {
            wrap: DrawingWrap::TopAndBottom,
            ..floating()
        },
        rotation_degrees: 15.0,
        text_direction: TextBoxDirection::Vertical,
        fill_color: Some("D9EAF7".into()),
    };
    let before = doc.to_bytes().unwrap();
    let refused = TextBoxOptions {
        fill_color: Some("blue".into()),
        ..options.clone()
    };
    assert!(doc.add_text_box_to_story(&story, "x", refused).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.add_text_box_to_story(&story, "Boxed quote", options)
        .unwrap();
    let mut doc = reopen(&mut doc);

    let paragraphs: Vec<_> = doc
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .map(|paragraph| (paragraph.location.story().kind(), paragraph.text))
        .collect();
    assert_eq!(
        paragraphs,
        [
            (StoryKind::Body, "Body".to_owned()),
            (StoryKind::Body, String::new()),
            (StoryKind::TextBox, "Boxed quote".to_owned()),
        ]
    );
    // No reader gives the shape back: the saved drawing carries its size,
    // fill, rotation (60,000ths of a degree) and text direction, and the
    // fallback for older readers its text.
    let xml = Saved::of(&mut doc).xml("word/document.xml");
    let shape = element(&xml, "wps:wsp");
    for expected in [
        r#"<a:xfrm rot="900000">"#,
        r#"<a:ext cx="1828800" cy="914400"/>"#,
        r#"<a:srgbClr val="D9EAF7"/>"#,
        r#"vert="vert""#,
    ] {
        assert!(shape.contains(expected), "{expected} is not in {shape}");
    }
    assert!(xml.contains("<wp:wrapTopAndBottom"), "{xml}");
    assert_eq!(xml.matches(">Boxed quote</w:t>").count(), 2, "{xml}");
}

// ---- Charts ----

#[test]
fn each_chart_kind_reads_back_with_its_kind_and_data() {
    let data = ChartData {
        categories: vec!["Été".into(), "Hiver".into(), "冬".into()],
        series: vec![
            ("North".into(), vec![1.5, -2.0, 3.25]),
            ("South".into(), vec![0.0, 4.0, 1e6]),
        ],
        ..ChartData::default()
    };
    let shares = ChartData {
        series: vec![("Share".into(), vec![50.0, 30.0, 20.0])],
        ..data.clone()
    };
    let points = ChartData {
        categories: vec!["1".into(), "2.5".into(), "4".into()],
        ..data.clone()
    };
    // Excel's XlChartType values: a bar chart is drawn as clustered columns,
    // lines and radar with markers, scatter as lines with markers.
    let cases = [
        (ChartKind::Bar, 51, &data),
        (ChartKind::Line, 65, &data),
        (ChartKind::Pie, 5, &shares),
        (ChartKind::Doughnut, -4120, &shares),
        (ChartKind::Area, 1, &data),
        (ChartKind::Radar, 81, &data),
        (ChartKind::Scatter, 74, &points),
    ];
    let mut doc = doc_with(&["Charts"]);
    for (kind, _, data) in &cases {
        doc.add_chart(*kind, Length::inches(4.0), Length::inches(3.0), data)
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
    }
    let doc = reopen(&mut doc);

    let expected: Vec<_> = cases
        .iter()
        .map(|(_, code, data)| (Some(*code), (*data).clone()))
        .collect();
    assert_eq!(doc.charts().unwrap(), expected);
}

#[test]
fn a_chart_inserted_after_a_paragraph_lands_right_after_it() {
    let series = |name: &str| ChartData {
        categories: vec!["a".into(), "b".into()],
        series: vec![(name.into(), vec![1.0, 2.0])],
        ..ChartData::default()
    };
    let (width, height) = (Length::inches(4.0), Length::inches(3.0));
    let mut doc = doc_with(&["Intro", "Outro"]);
    doc.add_chart(ChartKind::Line, width, height, &series("Late"))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.add_chart_after(9, ChartKind::Pie, width, height, &series("Nowhere"))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.add_chart_after(0, ChartKind::Pie, width, height, &series("Early"))
        .unwrap();
    let mut doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["Intro", "", "Outro", ""]);
    let charts: Vec<_> = doc
        .charts()
        .unwrap()
        .into_iter()
        .map(|(kind, data)| (kind, data.series[0].0.clone()))
        .collect();
    assert_eq!(
        charts,
        [(Some(5), "Early".to_owned()), (Some(65), "Late".to_owned())]
    );
    // Each chart has a part of its own.
    let saved = Saved::of(&mut doc);
    let targets: Vec<_> = saved
        .relationships("word/document.xml", CHART)
        .into_values()
        .collect();
    assert_eq!(targets.len(), 2);
    assert_ne!(targets[0], targets[1]);
}

#[test]
fn the_chart_legend_can_be_moved_or_shown_and_survives_a_save() {
    let two = ChartData {
        categories: vec!["a".into(), "b".into()],
        series: vec![("s".into(), vec![1.0, 2.0]), ("t".into(), vec![3.0, 4.0])],
        ..ChartData::default()
    };
    let one = ChartData {
        series: vec![two.series[0].clone()],
        ..two.clone()
    };
    let mut doc = doc_with(&["Intro"]);
    doc.add_chart(
        ChartKind::Bar,
        Length::inches(4.0),
        Length::inches(3.0),
        &two,
    )
    .unwrap();
    doc.add_chart(
        ChartKind::Line,
        Length::inches(4.0),
        Length::inches(3.0),
        &one,
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    // A new chart shows its legend at the right when it has several series.
    let at_right = |visible| LegendSettings {
        visible,
        position: LegendPosition::Right,
        overlay: false,
    };
    assert_eq!(doc.chart_legend(1).unwrap(), at_right(true));
    assert_eq!(doc.chart_legend(2).unwrap(), at_right(false));
    // A paragraph without a chart has no legend.
    assert!(doc.chart_legend(0).is_err());

    doc.set_chart_legend(1, None, Some(LegendPosition::Bottom), Some(true))
        .unwrap();
    doc.set_chart_legend(2, Some(true), Some(LegendPosition::Top), None)
        .unwrap();
    let doc = reopen(&mut doc);

    assert_eq!(
        doc.chart_legend(1).unwrap(),
        LegendSettings {
            visible: true,
            position: LegendPosition::Bottom,
            overlay: true,
        }
    );
    assert_eq!(
        doc.chart_legend(2).unwrap(),
        LegendSettings {
            visible: true,
            position: LegendPosition::Top,
            overlay: false,
        }
    );
    let kept: Vec<_> = doc.charts().unwrap().into_iter().map(|(_, d)| d).collect();
    assert_eq!(kept, [two, one]);
}

#[test]
fn chart_data_that_cannot_be_drawn_is_refused_without_a_change() {
    let valid = ChartData {
        categories: vec!["a".into(), "b".into()],
        series: vec![("s".into(), vec![1.0, 2.0])],
        ..ChartData::default()
    };
    let cases = [
        (
            "no category",
            ChartKind::Bar,
            Length::inches(4.0),
            ChartData {
                categories: Vec::new(),
                series: vec![("s".into(), Vec::new())],
                ..ChartData::default()
            },
        ),
        (
            "more values than categories",
            ChartKind::Bar,
            Length::inches(4.0),
            ChartData {
                series: vec![("s".into(), vec![1.0, 2.0, 3.0])],
                ..valid.clone()
            },
        ),
        (
            "a text category on a scatter chart",
            ChartKind::Scatter,
            Length::inches(4.0),
            valid.clone(),
        ),
        (
            "a zero width",
            ChartKind::Bar,
            Length::emu(0),
            valid.clone(),
        ),
    ];
    let mut doc = doc_with(&["Intro"]);
    let before = doc.to_bytes().unwrap();
    for (case, kind, width, data) in cases {
        assert!(
            doc.add_chart(kind, width, Length::inches(3.0), &data)
                .is_err(),
            "{case} is accepted"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{case} changed the file");
    }
    assert!(doc.charts().unwrap().is_empty());
}

#[test]
fn the_image_list_leaves_out_charts() {
    let data = ChartData {
        categories: vec!["a".into()],
        series: vec![("s".into(), vec![1.0])],
        ..ChartData::default()
    };
    let mut doc = doc_with(&["Intro"]);
    doc.add_picture(
        &png(2, 2, 9),
        "p.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    doc.add_chart(
        ChartKind::Bar,
        Length::inches(4.0),
        Length::inches(3.0),
        &data,
    )
    .unwrap();
    let doc = reopen(&mut doc);

    let pictures: Vec<_> = doc
        .pictures()
        .unwrap()
        .into_iter()
        .map(|picture| picture.relationship_id)
        .collect();
    assert_eq!(pictures.len(), 1);
    let images: Vec<_> = doc
        .images()
        .into_iter()
        .map(|image| image.embed_id)
        .collect();
    assert!(images.contains(&pictures[0]), "{images:?}");
    assert!(
        images.iter().all(|id| !id.is_empty()),
        "images() lists a drawing that shows no picture: {images:?}"
    );
    // A chart listed under an id of its own is still not a picture.
    assert_eq!(images, pictures);
}

// ---- Watermarks ----

#[test]
fn an_image_watermark_shows_its_picture_from_the_header() {
    let image = png(6, 4, 200);
    let mut doc = doc_with(&["Body"]);
    doc.set_image_watermark(&image, "mark.png", Length::inches(3.0), Length::inches(2.0))
        .unwrap();
    let mut doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["Body"]);
    let header = header_story(&doc).part_name().to_owned();
    let saved = Saved::of(&mut doc);
    let xml = saved.xml(&header);
    // A VML shape whose image comes through the header's own relationship.
    let shape = element(&xml, "v:shape");
    let ids = values(shape, "r:id");
    assert_eq!(ids.len(), 1, "{shape}");
    let relationships = saved.relationships(&header, IMAGE);
    let target = relationships
        .get(ids[0])
        .unwrap_or_else(|| panic!("{} is not an image relationship of {header}", ids[0]));
    assert_eq!(saved.part(target), Some(image.as_slice()));
    assert!(shape.contains("width:216pt;height:144pt"), "{shape}");
}

#[test]
fn a_drawingml_image_watermark_is_replaced_then_removed_with_its_media() {
    let (first, second) = (png(4, 4, 10), png(5, 5, 20));
    let options = ImageWatermarkOptions {
        width: Length::inches(4.0),
        height: Length::inches(2.0),
        rotation_degrees: 0.0,
        opacity: 0.3,
    };
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Header text");
    doc.set_drawingml_image_watermark_for(
        0,
        HdrFtrType::Default,
        &first,
        "first.png",
        options.clone(),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    let marks = header_pictures(&doc);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].bytes, first);
    assert!(marks[0].anchored);
    assert_eq!(
        (marks[0].width, marks[0].height),
        (4 * EMU_PER_INCH, 2 * EMU_PER_INCH)
    );
    // An opacity of 0.3 is a 30 % alpha on the picture.
    let saved = Saved::of(&mut doc);
    let headers: String = saved
        .entries
        .iter()
        .filter(|(name, _)| name.starts_with("word/header"))
        .map(|(name, _)| saved.xml(name))
        .collect();
    assert!(
        headers.contains(r#"<a:alphaModFix amt="30000"/>"#),
        "{headers}"
    );

    doc.set_drawingml_image_watermark_for(0, HdrFtrType::Default, &second, "second.png", options)
        .unwrap();
    let mut doc = reopen(&mut doc);
    let marks: Vec<_> = header_pictures(&doc).into_iter().map(|m| m.bytes).collect();
    assert_eq!(marks, std::slice::from_ref(&second));
    let saved = Saved::of(&mut doc);
    let media: Vec<_> = saved.media().into_values().collect();
    assert_eq!(
        media,
        [second.as_slice()],
        "the first watermark's media is gone"
    );

    doc.remove_drawingml_watermark_for(0, HdrFtrType::Default)
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert!(doc.pictures().unwrap().is_empty());
    assert!(Saved::of(&mut doc).media().is_empty());
    let header = header_story(&doc);
    let first_line = doc.story_items(&header).unwrap()[0].text().unwrap();
    assert_eq!(first_line.as_deref(), Some("Header text"));
}

#[test]
fn a_drawingml_text_watermark_carries_its_text_then_goes() {
    let mut doc = doc_with(&["Body"]);
    doc.set_drawingml_text_watermark_for(
        0,
        HdrFtrType::Default,
        "DRAFT ONLY",
        TextWatermarkOptions::default(),
        Length::pt(48.0),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);

    let header = header_story(&doc).part_name().to_owned();
    let xml = Saved::of(&mut doc).xml(&header);
    let shape = element(&xml, "wps:wsp");
    for expected in [
        ">DRAFT ONLY</w:t>",
        // 48 points in half-points, the default colour and the default
        // rotation of 315 degrees in 60,000ths of a degree.
        r#"<w:sz w:val="96"/>"#,
        r#"<w:color w:val="D9D9D9"/>"#,
        r#"<a:xfrm rot="18900000">"#,
    ] {
        assert!(shape.contains(expected), "{expected} is not in {shape}");
    }

    doc.remove_drawingml_watermark_for(0, HdrFtrType::Default)
        .unwrap();
    let mut doc = reopen(&mut doc);
    let xml = Saved::of(&mut doc).xml(&header);
    assert!(!xml.contains("DRAFT ONLY"), "{xml}");
    assert_eq!(texts(&doc), ["Body"]);
}

#[test]
fn watermark_settings_out_of_range_are_refused_without_a_change() {
    let mut doc = doc_with(&["Body"]);
    let before = doc.to_bytes().unwrap();
    let image = png(2, 2, 1);
    let text = |options: TextWatermarkOptions| options;
    let picture = ImageWatermarkOptions {
        width: Length::inches(4.0),
        height: Length::inches(2.0),
        rotation_degrees: 0.0,
        opacity: 0.5,
    };
    let text_cases = [
        (
            "an opacity over one",
            text(TextWatermarkOptions {
                opacity: 1.5,
                ..TextWatermarkOptions::default()
            }),
        ),
        (
            "a colour that is not hexadecimal",
            text(TextWatermarkOptions {
                color: "grey".into(),
                ..TextWatermarkOptions::default()
            }),
        ),
        (
            "a zero height",
            text(TextWatermarkOptions {
                height: Length::emu(0),
                ..TextWatermarkOptions::default()
            }),
        ),
    ];
    for (case, options) in text_cases {
        assert!(
            doc.set_text_watermark_for(0, HdrFtrType::Default, "DRAFT", options.clone())
                .is_err(),
            "text watermark with {case} is accepted"
        );
        assert!(
            doc.set_drawingml_text_watermark_for(
                0,
                HdrFtrType::Default,
                "DRAFT",
                options,
                Length::pt(40.0)
            )
            .is_err(),
            "DrawingML text watermark with {case} is accepted"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{case} changed the file");
    }
    let picture_cases = [
        (
            "a negative opacity",
            &image[..],
            ImageWatermarkOptions {
                opacity: -0.1,
                ..picture.clone()
            },
        ),
        (
            "a zero width",
            &image[..],
            ImageWatermarkOptions {
                width: Length::emu(0),
                ..picture.clone()
            },
        ),
        (
            "bytes that are not an image",
            b"not an image".as_slice(),
            picture.clone(),
        ),
    ];
    for (case, bytes, options) in picture_cases {
        assert!(
            doc.set_drawingml_image_watermark_for(
                0,
                HdrFtrType::Default,
                bytes,
                "mark.png",
                options
            )
            .is_err(),
            "image watermark with {case} is accepted"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{case} changed the file");
    }
    assert!(
        doc.set_image_watermark(&image, "mark.png", Length::inches(1.0), Length::emu(0))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before, "set_image_watermark");
}

// ---- The corpus ----

const LINKED_IMAGE: &str = "file:///C:/Images/logo.png";

/// `bytes` with the first picture of `part` linked ("Link to File"): its
/// blip names an External image relationship with `r:link`, and its media
/// part is gone. Returns the bytes and the linked relationship id.
fn link_first_picture(bytes: Vec<u8>, part: &str) -> (Vec<u8>, String) {
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let start = xml.find("r:embed=\"").unwrap() + "r:embed=\"".len();
    let embedded = xml[start..start + xml[start..].find('"').unwrap()].to_owned();
    let relationships = package.get_part_rels_mut(part).unwrap();
    let media = relationships.get_by_id(&embedded).unwrap().target.clone();
    relationships
        .items
        .retain(|relationship| relationship.id != embedded);
    let linked = relationships.add_external(
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
        LINKED_IMAGE,
    );
    package.remove_part(&OpcPackage::resolve_rel_target(part, &media));
    package.set_part(
        part,
        xml.replace(
            &format!("r:embed=\"{embedded}\""),
            &format!("r:link=\"{linked}\""),
        )
        .into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    (out.into_inner(), linked)
}

/// A document whose only picture is linked.
fn linked_picture_document() -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("Logo ");
    doc.add_picture(
        &png(2, 2, 9),
        "logo.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let (bytes, _) = link_first_picture(doc.to_bytes().unwrap(), "/word/document.xml");
    Document::from_bytes(&bytes).unwrap()
}

#[test]
fn a_picture_control_takes_the_linked_picture_it_holds() {
    let mut doc = Document::new();
    doc.add_picture(
        &png(2, 2, 9),
        "logo.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let start = main.rfind("<w:p>").or_else(|| main.rfind("<w:p ")).unwrap();
    let end = main[start..].find("</w:p>").unwrap() + start + "</w:p>".len();
    let wrapped = format!(
        "{}<w:sdt><w:sdtPr><w:tag w:val=\"logo\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>{}",
        &main[..start],
        &main[start..end],
        &main[end..]
    );
    package.set_part("/word/document.xml", wrapped.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let (bytes, linked) = link_first_picture(out.into_inner(), "/word/document.xml");
    let mut doc = Document::from_bytes(&bytes).unwrap();
    let location = doc
        .story_item_snapshots()
        .unwrap()
        .into_iter()
        .find(|item| {
            item.location().item_kind() == StoryItemKind::ContentControl && item.is_direct_child()
        })
        .map(|item| item.location().clone())
        .expect("the control");
    doc.set_content_control_type(
        &location,
        ContentControlTypePayload::Picture {
            relationship_id: linked,
        },
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.pictures().unwrap()[0].external_url.as_deref(),
        Some(LINKED_IMAGE)
    );
}

#[test]
fn a_removed_picture_leaves_a_document_saved_as_a_template() {
    let mut doc = doc_with(&["Body"]);
    doc.add_picture(
        &png(2, 2, 66),
        "gone.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let mut doc = reopen(&mut doc);
    let picture = doc.pictures().unwrap().remove(0);
    doc.remove_content_at(&picture.location).unwrap();
    let path = std::env::temp_dir().join(format!(
        "oxoffice-removed-picture-{}.dotx",
        std::process::id()
    ));
    doc.save(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    assert!(
        !package
            .parts
            .keys()
            .any(|part| part.starts_with("/word/media/"))
    );
    let images = package
        .get_part_rels("/word/document.xml")
        .map_or(0, |relationships| {
            relationships.get_all_by_type(IMAGE).len()
        });
    assert_eq!(images, 0);
}

// ---- Linked pictures in a comparison ----

/// `original` redlined into `edited`, saved and reopened.
fn compared(mut original: Document, edited: &Document) -> Document {
    let diagnostics = original
        .compare(edited, "Reviewer", "2026-11-21T09:00:00Z")
        .unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    reopen(&mut original)
}

/// The External addresses of the pictures of `doc`, saved with every
/// revision accepted, or rejected.
fn resolved_picture_addresses(doc: &mut Document, accept: bool) -> Vec<Option<String>> {
    let mut copy = reopen(doc);
    if accept {
        copy.accept_all().unwrap();
    } else {
        copy.reject_all().unwrap();
    }
    reopen(&mut copy)
        .pictures()
        .unwrap()
        .into_iter()
        .map(|picture| picture.external_url)
        .collect()
}

#[test]
fn a_linked_picture_added_in_a_comparison_keeps_its_address() {
    // The original holds hyperlinks, one under the relationship id of the
    // linked picture of the edited version.
    let edited = linked_picture_document();
    let linked = edited.pictures().unwrap()[0].relationship_id.clone();
    let mut original = Document::new();
    let mut links = Vec::new();
    while !links.contains(&linked) {
        assert!(links.len() < 10, "{linked} is a hyperlink id: {links:?}");
        links.push(
            original.add_hyperlink_relationship(&format!("https://example.com/{}", links.len())),
        );
    }
    let mut paragraph = original.add_paragraph("Logo ");
    for (index, link) in links.iter().enumerate() {
        paragraph.add_hyperlink(&format!("site {index} "), link);
    }
    let mut doc = compared(original, &edited);
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].external_url.as_deref(), Some(LINKED_IMAGE));
    assert_eq!(
        resolved_picture_addresses(&mut doc, true),
        [Some(LINKED_IMAGE.to_owned())]
    );
    assert_eq!(resolved_picture_addresses(&mut doc, false), []);
}

/// The parts and relationship targets of the saved `doc`, by ZIP entry name.
fn saved_entries(doc: &mut Document) -> BTreeMap<String, String> {
    crate::support::package_parts(&doc.to_bytes().unwrap())
        .into_iter()
        .map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned()))
        .collect()
}

#[test]
fn removed_content_leaves_no_chart_link_or_note_link_behind() {
    // A chart (and its embedded workbook) cut out of the body.
    let mut doc = doc_with(&["kept"]);
    let data = ChartData {
        categories: vec!["Q1".into(), "Q2".into()],
        series: vec![("s".into(), vec![1.0, 2.0])],
        ..ChartData::default()
    };
    doc.add_chart(
        ChartKind::Bar,
        Length::inches(4.0),
        Length::inches(3.0),
        &data,
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|s| s.kind() == StoryKind::Body)
        .unwrap();
    let chart = doc.story_items(&body).unwrap()[1].location().clone();
    doc.remove_content_at(&chart).unwrap();
    let entries = saved_entries(&mut doc);
    assert!(
        !entries
            .keys()
            .any(|name| name.contains("chart") || name.contains("embeddings")),
        "{:?}",
        entries.keys().collect::<Vec<_>>()
    );
    // A link cut out of the body.
    let mut doc = doc_with(&["kept", ""]);
    doc.append_hyperlink("site", "https://gone.example/");
    let mut doc = reopen(&mut doc);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|s| s.kind() == StoryKind::Body)
        .unwrap();
    let linked = doc.story_items(&body).unwrap()[1].location().clone();
    doc.remove_content_at(&linked).unwrap();
    assert!(!saved_entries(&mut doc)["word/_rels/document.xml.rels"].contains("gone.example"));
    // A footnote holding a link, removed.
    let mut doc = doc_with(&["noted"]);
    let id = doc.add_footnote("note");
    let mut doc = reopen(&mut doc);
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let notes =
        String::from_utf8(package.get_part("/word/footnotes.xml").unwrap().to_vec()).unwrap();
    let at = notes.rfind("</w:p>").unwrap();
    package.set_part(
        "/word/footnotes.xml",
        format!(
            r#"{}<w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rIdNoteLink"><w:r><w:t>link</w:t></w:r></w:hyperlink>{}"#,
            &notes[..at],
            &notes[at..]
        )
        .into_bytes(),
    );
    package
        .get_or_create_part_rels("/word/footnotes.xml")
        .items
        .push(Relationship {
            id: "rIdNoteLink".into(),
            rel_type:
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
                    .into(),
            target: "https://note.example/".into(),
            target_mode: Some("External".into()),
        });
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.remove_footnote(id).unwrap();
    let entries = saved_entries(&mut doc);
    assert!(
        !entries
            .get("word/_rels/footnotes.xml.rels")
            .is_some_and(|rels| rels.contains("note.example"))
    );
}

#[test]
fn a_raw_header_replaced_on_a_reopened_file_leaves_no_old_picture() {
    let header = |rid: &str| {
        format!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:v="urn:schemas-microsoft-com:vml"><w:p><w:r><w:pict><v:shape style="width:10pt;height:10pt"><v:imagedata r:id="{rid}"/></v:shape></w:pict></w:r></w:p></w:hdr>"#
        )
        .into_bytes()
    };
    let mut doc = doc_with(&["body"]);
    doc.set_raw_header_with_images(
        header("rIdA"),
        &[("rIdA", &png(2, 2, 10), "first.png")],
        HdrFtrType::Default,
    );
    let mut doc = reopen(&mut doc);
    doc.set_raw_header_with_images(
        header("rIdB"),
        &[("rIdB", &png(2, 2, 20), "second.png")],
        HdrFtrType::Default,
    );
    let media: Vec<_> = saved_entries(&mut doc)
        .into_keys()
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media.len(), 1, "{media:?}");
}

#[test]
fn a_picture_or_link_removed_by_another_edit_leaves_the_saved_file() {
    // A picture whose run goes.
    let mut doc = doc_with(&["kept"]);
    doc.add_picture(
        &png(2, 2, 30),
        "gone.png",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let mut doc = reopen(&mut doc);
    {
        let mut paragraph = doc.paragraph_mut(1).expect("the picture paragraph");
        let runs = paragraph.runs().count();
        for run in (0..runs).rev() {
            paragraph.remove_run(run).unwrap();
        }
    }
    let media: Vec<_> = saved_entries(&mut doc)
        .into_keys()
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert!(media.is_empty(), "{media:?}");
    // A link unwrapped by `remove_hyperlink`.
    let mut doc = doc_with(&["kept", ""]);
    doc.append_hyperlink("site", "https://gone.example/");
    let mut doc = reopen(&mut doc);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|s| s.kind() == StoryKind::Body)
        .unwrap();
    doc.remove_hyperlink(&body, 0).unwrap();
    assert!(!saved_entries(&mut doc)["word/_rels/document.xml.rels"].contains("gone.example"));
}

#[test]
fn a_cut_picture_comes_back_after_its_other_occurrence_goes() {
    // Two pictures share one relationship, as Word writes a duplicated one.
    let picture = |name: &str| {
        format!(
            concat!(
                r#"<w:p>"#,
                r#"<w:r><w:drawing><wp:inline><wp:extent cx="91440" cy="91440"/><wp:docPr id="{id}" name="{name}"/>"#,
                r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
                r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
                r#"<pic:blipFill><a:blip r:embed="rIdShared"/></pic:blipFill><pic:spPr/></pic:pic>"#,
                r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
            ),
            id = if name == "first" { 1 } else { 2 },
            name = name
        )
    };
    let mut doc = doc_with_body(&format!(
        "{}{}<w:p><w:r><w:t>end</w:t></w:r></w:p>",
        picture("first"),
        picture("second")
    ));
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    package.set_part("/word/media/shared.png", png(2, 2, 40));
    package.content_types.add_default("png", "image/png");
    package
        .get_or_create_part_rels("/word/document.xml")
        .add_with_id("rIdShared", IMAGE, "media/shared.png");
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let story = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|s| s.kind() == StoryKind::Body)
        .unwrap();
    let first = doc.story_items(&story).unwrap()[0].location().clone();
    let fragment = doc.remove_content_at(&first).unwrap();
    {
        let mut paragraph = doc.paragraph_mut(0).expect("the second picture");
        let runs = paragraph.runs().count();
        for run in (0..runs).rev() {
            paragraph.remove_run(run).unwrap();
        }
    }
    for _ in 0..2 {
        doc.to_bytes().unwrap();
    }
    let story = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|s| s.kind() == StoryKind::Body)
        .unwrap();
    doc.insert_content(&oxoffice_docx::ContentLocation::end(story), fragment)
        .expect("the cut picture comes back");
    assert!(saved_entries(&mut doc).contains_key("word/media/shared.png"));
}
