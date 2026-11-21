//! A PDF read back with lopdf: the text a page draws with where it lands,
//! the pictures it paints, its links, fonts, outline and tags.

use std::collections::{BTreeMap, HashMap};

use lopdf::content::Content;
use lopdf::{Dictionary, Object, ObjectId};

/// One text object of a content stream, or one shaped run read through its
/// ActualText: its text, where its first glyph starts and its last glyph
/// ends, in default user space (points from the bottom left corner).
#[derive(Debug, Clone)]
pub struct Drawn {
    pub text: String,
    /// What its glyphs map back to through the font's ToUnicode map: the
    /// text itself outside a shaped run, the drawn characters inside one.
    pub glyphs: String,
    pub x: f64,
    pub y: f64,
    pub end: f64,
    /// The largest font size it is drawn at, in points of user space.
    pub size: f64,
    /// The marked content identifier the structure tree refers to it by.
    pub mcid: Option<i64>,
    /// How many of its glyphs are the font's .notdef glyph, an empty box.
    pub notdef: usize,
}

/// A font of a page: its map back to text and its glyph widths.
pub struct PageFont<'a> {
    pub encoding: lopdf::Encoding<'a>,
    pub widths: HashMap<u16, f64>,
}

/// A picture a page paints.
#[derive(Debug)]
pub struct Picture {
    /// Left, bottom, width and height, in points.
    pub rect: [f64; 4],
    /// Width and height in pixels.
    pub pixels: (i64, i64),
    /// The image's filter, such as `FlateDecode` or `DCTDecode`.
    pub filter: String,
    /// The decoded samples, or the JPEG bytes as stored.
    pub data: Vec<u8>,
}

/// A structure element: its type, its depth under the root, its alternate
/// text, and the (page, MCID) of the marked content it owns directly.
pub type Element = (String, usize, Option<String>, Vec<(usize, i64)>);

/// A PDF read back.
pub struct Pdf {
    pub doc: lopdf::Document,
}

impl Pdf {
    pub fn read(bytes: &[u8]) -> Self {
        assert!(bytes.starts_with(b"%PDF-"), "the output is not a PDF");
        Self {
            doc: lopdf::Document::load_mem(bytes).expect("the PDF parses"),
        }
    }

    pub fn catalog(&self) -> &Dictionary {
        self.doc.catalog().expect("the PDF has a catalog")
    }

    /// The object behind a reference, or the object itself.
    pub fn resolve<'a>(&'a self, object: &'a Object) -> &'a Object {
        self.doc.dereference(object).expect("reference resolves").1
    }

    pub fn dict<'a>(&'a self, object: &'a Object) -> &'a Dictionary {
        self.resolve(object).as_dict().expect("a dictionary")
    }

    pub fn page_ids(&self) -> Vec<ObjectId> {
        self.doc.get_pages().into_values().collect()
    }

    pub fn page_count(&self) -> usize {
        self.page_ids().len()
    }

    pub fn page(&self, page: usize) -> &Dictionary {
        self.doc
            .get_dictionary(self.page_ids()[page])
            .expect("page dictionary")
    }

    /// Width and height of a page's media box, in points.
    pub fn page_size(&self, page: usize) -> (f64, f64) {
        let media_box = self.resolve(self.page(page).get(b"MediaBox").expect("MediaBox"));
        let [left, bottom, right, top] = numbers(media_box.as_array().expect("MediaBox array"))[..]
        else {
            panic!("a MediaBox has four numbers");
        };
        (right - left, top - bottom)
    }

    pub fn fonts(&self, page: usize) -> BTreeMap<Vec<u8>, PageFont<'_>> {
        let fonts = self
            .doc
            .get_page_fonts(self.page_ids()[page])
            .expect("page fonts");
        fonts
            .into_iter()
            .map(|(name, font)| {
                let descendants = font.get(b"DescendantFonts").expect("DescendantFonts");
                let descendant =
                    self.dict(&self.resolve(descendants).as_array().expect("array")[0]);
                let mut widths = HashMap::new();
                if let Ok(entries) = descendant.get(b"W") {
                    let entries = self.resolve(entries).as_array().expect("W array");
                    let mut index = 0;
                    while index < entries.len() {
                        let first = entries[index].as_i64().expect("W start") as u16;
                        if let Object::Array(list) = self.resolve(&entries[index + 1]) {
                            for (offset, width) in numbers(list).into_iter().enumerate() {
                                widths.insert(first + offset as u16, width);
                            }
                            index += 2;
                        } else {
                            let last = entries[index + 1].as_i64().expect("W end") as u16;
                            let width = f64::from(entries[index + 2].as_float().expect("W"));
                            widths.extend((first..=last).map(|glyph| (glyph, width)));
                            index += 3;
                        }
                    }
                }
                let encoding = font.get_font_encoding(&self.doc).expect("font encoding");
                (name, PageFont { encoding, widths })
            })
            .collect()
    }

    /// Every text object a page draws, in content stream order. A shaped
    /// run (right-to-left and complex scripts) reads as its ActualText, its
    /// logical text, once.
    pub fn drawn(&self, page: usize) -> Vec<Drawn> {
        let fonts = self.fonts(page);
        let content = self.doc.get_page_content(self.page_ids()[page]);
        let operations = Content::decode(&content)
            .expect("content stream decodes")
            .operations;

        // An open marked content sequence: whether it is a span with
        // ActualText, and its marked content identifier.
        struct Mark {
            span: bool,
            mcid: Option<i64>,
        }
        let mut ctm = IDENTITY;
        let mut saved = Vec::new();
        let mut font: Option<(&PageFont, f64)> = None;
        let mut text_matrix = IDENTITY;
        let mut marks: Vec<Mark> = Vec::new();
        let mut drawn = Vec::new();
        let mut current: Option<Drawn> = None;
        // Along the baseline, in points from the current text matrix: the
        // start of the text line, the pen, and the furthest glyph edge.
        let (mut line_start, mut pen, mut reach) = (0.0, 0.0, f64::NEG_INFINITY);
        let finish = |piece: Option<Drawn>, reach: f64, drawn: &mut Vec<Drawn>| {
            drawn.extend(piece.map(|mut piece| {
                piece.end = piece.x + reach.max(0.0);
                piece
            }));
        };
        for operation in &operations {
            let operands = &operation.operands;
            let in_span = marks.iter().any(|mark| mark.span);
            let mcid = marks.iter().rev().find_map(|mark| mark.mcid);
            let new_piece = |text: String, mcid| Drawn {
                text,
                glyphs: String::new(),
                x: f64::NAN,
                y: f64::NAN,
                end: f64::NAN,
                size: 0.0,
                mcid,
                notdef: 0,
            };
            // Text space to user space, along the baseline.
            let along = text_matrix[0] * ctm[0] + text_matrix[1] * ctm[2];
            match operation.operator.as_str() {
                "q" => saved.push(ctm),
                "Q" => ctm = saved.pop().expect("balanced q and Q"),
                "cm" => ctm = multiply(matrix(operands), ctm),
                "BDC" => {
                    let properties = operands.get(1).and_then(|object| object.as_dict().ok());
                    let actual = properties
                        .and_then(|properties| properties.get(b"ActualText").ok())
                        .map(|text| lopdf::decode_text_string(text).expect("ActualText"));
                    let own = properties
                        .and_then(|properties| properties.get(b"MCID").ok())
                        .map(|id| id.as_i64().expect("MCID is an integer"));
                    if let Some(text) = &actual
                        && !in_span
                    {
                        current = Some(new_piece(text.clone(), own.or(mcid)));
                        reach = f64::NEG_INFINITY;
                    }
                    marks.push(Mark {
                        span: actual.is_some(),
                        mcid: own,
                    });
                }
                "BMC" => marks.push(Mark {
                    span: false,
                    mcid: None,
                }),
                "EMC" => {
                    let mark = marks.pop().expect("balanced BDC and EMC");
                    if mark.span && !marks.iter().any(|mark| mark.span) {
                        finish(current.take(), reach, &mut drawn);
                    }
                }
                "BT" if !in_span => {
                    current = Some(new_piece(String::new(), mcid));
                    reach = f64::NEG_INFINITY;
                }
                "ET" if !in_span => finish(current.take(), reach, &mut drawn),
                "Tf" => {
                    let name = operands[0].as_name().expect("font name");
                    let size = f64::from(operands[1].as_float().expect("font size"));
                    font = Some((fonts.get(name).expect("a font of the page"), size));
                }
                "Tm" => {
                    text_matrix = matrix(operands);
                    (line_start, pen) = (0.0, 0.0);
                    if let Some(piece) = current.as_mut().filter(|piece| piece.x.is_nan()) {
                        let [.., x, y] = multiply(text_matrix, ctm);
                        (piece.x, piece.y) = (x, y);
                    }
                }
                // The next line starts from the start of this one, not the pen.
                "Td" => {
                    line_start += f64::from(operands[0].as_float().expect("Td x")) * along;
                    pen = line_start;
                }
                "Tj" | "TJ" => {
                    let (font, size) = font.expect("a font is set before text");
                    let scale = along * size / 1000.0;
                    let piece = current.as_mut().expect("text is shown inside BT and ET");
                    piece.size = piece.size.max(along.abs() * size);
                    let items: Vec<&Object> = match &operands[0] {
                        Object::Array(items) => items.iter().collect(),
                        string => vec![string],
                    };
                    for item in items {
                        let Ok(string) = item.as_str() else {
                            pen -= f64::from(item.as_float().expect("TJ number")) * scale;
                            continue;
                        };
                        // Two-byte glyph identifiers (Identity-H); 0 is .notdef.
                        for glyph in string.chunks(2) {
                            let glyph = u16::from_be_bytes([glyph[0], glyph[1]]);
                            piece.notdef += usize::from(glyph == 0);
                            pen += font.widths.get(&glyph).copied().unwrap_or(0.0) * scale;
                            reach = reach.max(pen);
                        }
                        let text = lopdf::Document::decode_text(&font.encoding, string)
                            .expect("text decodes");
                        piece.glyphs.push_str(&text);
                        if !in_span {
                            piece.text.push_str(&text);
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(
            current.is_none() && marks.is_empty() && saved.is_empty(),
            "page {page} ends inside text, marked content or a saved state"
        );
        drawn
    }

    /// Every picture a page paints, in content stream order.
    pub fn pictures(&self, page: usize) -> Vec<Picture> {
        let resources = self.dict(self.page(page).get(b"Resources").expect("Resources"));
        let xobjects = resources
            .get(b"XObject")
            .ok()
            .map(|xobjects| self.dict(xobjects));
        let content = self.doc.get_page_content(self.page_ids()[page]);
        let operations = Content::decode(&content)
            .expect("content stream decodes")
            .operations;
        let mut ctm = IDENTITY;
        let mut saved = Vec::new();
        let mut pictures = Vec::new();
        for operation in &operations {
            match operation.operator.as_str() {
                "q" => saved.push(ctm),
                "Q" => ctm = saved.pop().expect("balanced q and Q"),
                "cm" => ctm = multiply(matrix(&operation.operands), ctm),
                "Do" => {
                    let name = operation.operands[0].as_name().expect("XObject name");
                    let xobject = xobjects.expect("XObjects").get(name).expect("XObject");
                    let stream = self.resolve(xobject).as_stream().expect("XObject stream");
                    let entry = |key: &[u8]| stream.dict.get(key).expect("image entry");
                    if entry(b"Subtype").as_name().ok() != Some(b"Image") {
                        continue;
                    }
                    let filter = entry(b"Filter").as_name().expect("filter name");
                    let filter = String::from_utf8_lossy(filter).into_owned();
                    let data = if filter == "FlateDecode" {
                        stream.decompressed_content().expect("image inflates")
                    } else {
                        stream.content.clone()
                    };
                    // The unit square through the current matrix.
                    let xs = [0.0, ctm[0], ctm[2], ctm[0] + ctm[2]].map(|x| x + ctm[4]);
                    let ys = [0.0, ctm[1], ctm[3], ctm[1] + ctm[3]].map(|y| y + ctm[5]);
                    let (left, bottom) = (min(&xs), min(&ys));
                    pictures.push(Picture {
                        rect: [left, bottom, max(&xs) - left, max(&ys) - bottom],
                        pixels: (
                            entry(b"Width").as_i64().expect("width"),
                            entry(b"Height").as_i64().expect("height"),
                        ),
                        filter,
                        data,
                    });
                }
                _ => {}
            }
        }
        pictures
    }

    /// The URI and rectangle (left, bottom, right, top) of each link
    /// annotation of a page.
    pub fn links(&self, page: usize) -> Vec<(String, [f64; 4])> {
        let annotations = self
            .doc
            .get_page_annotations(self.page_ids()[page])
            .expect("page annotations");
        annotations
            .into_iter()
            .filter(|annot| annot.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Link"))
            .map(|annot| {
                let action = self.dict(annot.get(b"A").expect("link action"));
                let kind = action
                    .get(b"S")
                    .and_then(Object::as_name)
                    .expect("action kind");
                assert_eq!(kind, b"URI", "a link opens a URI");
                let uri = action.get(b"URI").and_then(Object::as_str).expect("URI");
                let rect = self.resolve(annot.get(b"Rect").expect("Rect"));
                let rect = numbers(rect.as_array().expect("Rect array"));
                let uri = String::from_utf8(uri.to_vec()).expect("URI is UTF-8");
                (uri, [rect[0], rect[1], rect[2], rect[3]])
            })
            .collect()
    }

    /// The outline in order, depth first: title, depth from 0, and the
    /// index of the page it opens.
    pub fn outline(&self) -> Vec<(String, usize, usize)> {
        let Ok(root) = self.catalog().get(b"Outlines") else {
            return Vec::new();
        };
        let pages = self.page_ids();
        let mut entries = Vec::new();
        let mut stack = Vec::new();
        if let Ok(first) = self.dict(root).get(b"First") {
            stack.push((first.as_reference().expect("First"), 0));
        }
        while let Some((id, depth)) = stack.pop() {
            let item = self.doc.get_dictionary(id).expect("outline item");
            let title = lopdf::decode_text_string(item.get(b"Title").expect("Title"));
            let destination = self.resolve(item.get(b"Dest").expect("Dest"));
            let target = destination.as_array().expect("Dest array")[0]
                .as_reference()
                .expect("Dest page");
            let page = pages.iter().position(|page| *page == target);
            entries.push((
                title.expect("title text"),
                depth,
                page.expect("Dest is a page"),
            ));
            if let Ok(next) = item.get(b"Next") {
                stack.push((next.as_reference().expect("Next"), depth));
            }
            if let Ok(child) = item.get(b"First") {
                stack.push((child.as_reference().expect("First"), depth + 1));
            }
        }
        entries
    }

    /// An entry of the document information dictionary.
    pub fn info(&self, key: &[u8]) -> Option<String> {
        let info = self.dict(self.doc.trailer.get(b"Info").ok()?);
        let value = info.get(key).ok()?;
        Some(lopdf::decode_text_string(value).expect("Info text"))
    }

    /// The XMP metadata packet of the catalog, if any.
    pub fn xmp(&self) -> Option<String> {
        let metadata = self.resolve(self.catalog().get(b"Metadata").ok()?);
        let stream = metadata.as_stream().expect("Metadata stream");
        let bytes = stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone());
        Some(String::from_utf8(bytes).expect("XMP is UTF-8"))
    }

    /// Every composite font of the file: its name, whether its font program
    /// is embedded, and whether it maps its glyphs back to text.
    pub fn embedded_fonts(&self) -> Vec<(String, bool, bool)> {
        let mut fonts = Vec::new();
        for object in self.doc.objects.values() {
            let Ok(font) = object.as_dict() else { continue };
            if font.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Type0") {
                continue;
            }
            let name = font
                .get(b"BaseFont")
                .and_then(Object::as_name)
                .expect("BaseFont");
            let descendants = font.get(b"DescendantFonts").expect("DescendantFonts");
            let descendant = self.dict(&self.resolve(descendants).as_array().expect("array")[0]);
            let descriptor = self.dict(descendant.get(b"FontDescriptor").expect("descriptor"));
            let embedded = descriptor.get(b"FontFile2").is_ok_and(|file| {
                let program = self.resolve(file).as_stream().expect("font program");
                !program
                    .decompressed_content()
                    .expect("font inflates")
                    .is_empty()
            });
            // A subset may carry a tag of six capital letters and a plus.
            let name = match name.split_at_checked(7) {
                Some((tag, rest))
                    if tag[6] == b'+' && tag[..6].iter().all(u8::is_ascii_uppercase) =>
                {
                    rest
                }
                _ => name,
            };
            let name = String::from_utf8_lossy(name).into_owned();
            fonts.push((name, embedded, font.get(b"ToUnicode").is_ok()));
        }
        fonts.sort();
        fonts
    }

    /// The structure tree, depth first.
    pub fn structure(&self) -> Vec<Element> {
        let pages = self.page_ids();
        let root = self.dict(
            self.catalog()
                .get(b"StructTreeRoot")
                .expect("StructTreeRoot"),
        );
        let mut elements = Vec::new();
        let mut stack = vec![(root.get(b"K").expect("root kids"), 0)];
        while let Some((kid, depth)) = stack.pop() {
            match self.resolve(kid) {
                Object::Array(kids) => stack.extend(kids.iter().rev().map(|kid| (kid, depth))),
                Object::Dictionary(element) if element.get(b"S").is_ok() => {
                    let role = element.get(b"S").and_then(Object::as_name).expect("S");
                    let alt = element
                        .get(b"Alt")
                        .ok()
                        .map(|alt| lopdf::decode_text_string(alt).expect("Alt text"));
                    let page = element.get(b"Pg").and_then(Object::as_reference).ok();
                    let kids: Vec<&Object> = match element.get(b"K") {
                        Ok(Object::Array(kids)) => kids.iter().collect(),
                        Ok(kid) => vec![kid],
                        Err(_) => Vec::new(),
                    };
                    let (mut marks, mut children) = (Vec::new(), Vec::new());
                    for kid in kids {
                        match self.resolve(kid) {
                            Object::Integer(mcid) => marks.push((page.expect("Pg"), *mcid)),
                            Object::Dictionary(mark) if mark.get(b"MCID").is_ok() => {
                                let own = mark.get(b"Pg").and_then(Object::as_reference).ok();
                                let mcid = mark.get(b"MCID").and_then(Object::as_i64);
                                marks.push((own.or(page).expect("Pg"), mcid.expect("MCID")));
                            }
                            _ => children.push((kid, depth + 1)),
                        }
                    }
                    let marks = marks
                        .into_iter()
                        .map(|(page, mcid)| {
                            let index = pages.iter().position(|id| *id == page);
                            (index.expect("Pg is a page"), mcid)
                        })
                        .collect();
                    let role = String::from_utf8_lossy(role).into_owned();
                    elements.push((role, depth, alt, marks));
                    stack.extend(children.into_iter().rev());
                }
                _ => {}
            }
        }
        elements
    }

    /// The text a page draws, its pieces joined in stream order.
    pub fn page_text(&self, page: usize) -> String {
        self.drawn(page)
            .into_iter()
            .map(|piece| piece.text)
            .collect()
    }

    /// The text a page draws between its top and bottom margins, without
    /// its header and footer.
    pub fn body_text(&self, page: usize, margin: f64) -> String {
        let (_, height) = self.page_size(page);
        self.drawn(page)
            .into_iter()
            .filter(|piece| piece.y > margin && piece.y < height - margin)
            .map(|piece| piece.text)
            .collect()
    }
}

pub const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub fn numbers(objects: &[Object]) -> Vec<f64> {
    objects
        .iter()
        .map(|object| f64::from(object.as_float().expect("a number")))
        .collect()
}

/// The six numbers of a matrix operator.
pub fn matrix(operands: &[Object]) -> [f64; 6] {
    numbers(operands).try_into().expect("six matrix numbers")
}

/// `first` then `second`, as PDF concatenates matrices.
pub fn multiply(first: [f64; 6], second: [f64; 6]) -> [f64; 6] {
    [
        first[0] * second[0] + first[1] * second[2],
        first[0] * second[1] + first[1] * second[3],
        first[2] * second[0] + first[3] * second[2],
        first[2] * second[1] + first[3] * second[3],
        first[4] * second[0] + first[5] * second[2] + second[4],
        first[4] * second[1] + first[5] * second[3] + second[5],
    ]
}

pub fn min(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

pub fn max(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}
