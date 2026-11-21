//! Synthetic layouts and a PDF reader for the writer's tests.

use std::sync::Arc;

use lopdf::{Dictionary, Document, Object, ObjectId};
use oxoffice_layout::bundled_fonts::LIBERATION_SANS_REGULAR;
use oxoffice_layout::{
    Color, FontData, FontId, GlyphRun, LayoutResult, PageFrame, Point, PositionedElement,
};

pub const PAGE_WIDTH: f64 = 612.0;
pub const PAGE_HEIGHT: f64 = 792.0;
pub const FONT: FontId = FontId(1);

/// A run of `text` in Liberation Sans at `size`, its baseline at `origin`.
pub fn run(text: &str, origin: Point, size: f64) -> PositionedElement {
    let face = ttf_parser::Face::parse(LIBERATION_SANS_REGULAR, 0).unwrap();
    let scale = size / f64::from(face.units_per_em());
    let glyph_ids: Vec<u16> = text
        .chars()
        .map(|ch| face.glyph_index(ch).map_or(0, |glyph| glyph.0))
        .collect();
    let advances = glyph_ids
        .iter()
        .map(|glyph| {
            f64::from(
                face.glyph_hor_advance(ttf_parser::GlyphId(*glyph))
                    .unwrap_or(0),
            ) * scale
        })
        .collect();
    PositionedElement::Text(GlyphRun {
        origin,
        font_id: FONT,
        font_size: size,
        glyph_ids,
        advances,
        text: text.to_owned(),
        source: None,
        color: Color::BLACK,
        bold: false,
        italic: false,
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
        tab_aligned: None,
    })
}

/// A layout of one page per element list, with the font the runs use.
pub fn layout(pages: Vec<Vec<PositionedElement>>) -> LayoutResult {
    let pages = pages
        .into_iter()
        .enumerate()
        .map(|(index, elements)| {
            Arc::new(PageFrame::new(index + 1, PAGE_WIDTH, PAGE_HEIGHT, elements))
        })
        .collect();
    let font = FontData {
        id: FONT,
        family: "Liberation Sans".to_owned(),
        data: Arc::from(LIBERATION_SANS_REGULAR),
        face_index: 0,
        bold: false,
        italic: false,
        variations: Vec::new(),
        synthetic_bold: false,
        synthetic_italic: false,
    };
    LayoutResult::new(pages, vec![font], None, Vec::new())
}

/// A PDF read back.
pub struct Pdf {
    pub doc: Document,
}

impl Pdf {
    pub fn read(bytes: &[u8]) -> Self {
        Self {
            doc: Document::load_mem(bytes).expect("the writer's PDF parses"),
        }
    }

    pub fn catalog(&self) -> &Dictionary {
        self.doc.catalog().unwrap()
    }

    /// The object behind a reference, or the object itself.
    pub fn resolve<'a>(&'a self, object: &'a Object) -> &'a Object {
        self.doc.dereference(object).unwrap().1
    }

    pub fn dict<'a>(&'a self, dict: &'a Dictionary, key: &[u8]) -> &'a Dictionary {
        self.resolve(dict.get(key).unwrap()).as_dict().unwrap()
    }

    /// The AcroForm's fields, in order.
    pub fn fields(&self) -> Vec<&Dictionary> {
        let form = self.dict(self.catalog(), b"AcroForm");
        form.get(b"Fields")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|field| self.resolve(field).as_dict().unwrap())
            .collect()
    }

    pub fn field(&self, name: &str) -> &Dictionary {
        self.fields()
            .into_iter()
            .find(|field| text(field.get(b"T").unwrap()) == name)
            .unwrap_or_else(|| panic!("no field named {name}"))
    }

    pub fn page_ids(&self) -> Vec<ObjectId> {
        self.doc.get_pages().into_values().collect()
    }

    pub fn page_content(&self, page: usize) -> Vec<u8> {
        self.doc.get_page_content(self.page_ids()[page])
    }

    /// The widget references of a page's `/Annots`.
    pub fn page_widgets(&self, page: usize) -> Vec<ObjectId> {
        let page = self
            .doc
            .get_object(self.page_ids()[page])
            .unwrap()
            .as_dict()
            .unwrap();
        page.get(b"Annots")
            .map(|annots| {
                self.resolve(annots)
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|annot| annot.as_reference().ok())
                    .filter(|id| {
                        self.doc
                            .get_object(*id)
                            .unwrap()
                            .as_dict()
                            .unwrap()
                            .get(b"Subtype")
                            .unwrap()
                            .as_name()
                            .unwrap()
                            == b"Widget"
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A stream's decoded bytes.
    pub fn stream(&self, object: &Object) -> Vec<u8> {
        let stream = self.resolve(object).as_stream().unwrap();
        stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone())
    }

    /// A field's normal appearance, or one state of it.
    pub fn appearance(&self, field: &Dictionary, state: Option<&[u8]>) -> Vec<u8> {
        let normal = self.dict(field, b"AP").get(b"N").unwrap();
        match state {
            None => self.stream(normal),
            Some(state) => self.stream(self.resolve(normal).as_dict().unwrap().get(state).unwrap()),
        }
    }
}

impl Pdf {
    /// The text a content stream draws with the page's one font, read back
    /// through the font's ToUnicode map.
    pub fn drawn_text(&self, content: &[u8]) -> String {
        let page = self
            .doc
            .get_object(self.page_ids()[0])
            .unwrap()
            .as_dict()
            .unwrap();
        let fonts = self.dict(self.dict(page, b"Resources"), b"Font");
        let font = self
            .resolve(fonts.iter().next().unwrap().1)
            .as_dict()
            .unwrap();
        let cmap = to_unicode(&self.stream(font.get(b"ToUnicode").unwrap()));
        shown_strings(content)
            .iter()
            .flat_map(|bytes| bytes.chunks(2))
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .map(|glyph| cmap.get(&glyph).cloned().unwrap_or_else(|| "?".to_owned()))
            .collect()
    }
}

/// Glyph to text pairs of a ToUnicode CMap's `bfchar` entries.
fn to_unicode(cmap: &[u8]) -> std::collections::HashMap<u16, String> {
    let text = String::from_utf8_lossy(cmap);
    let mut map = std::collections::HashMap::new();
    for block in text.split("beginbfchar").skip(1) {
        let block = block.split("endbfchar").next().unwrap();
        let hexes: Vec<&str> = block
            .split(['<', '>'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        for pair in hexes.chunks(2) {
            let glyph = u16::from_str_radix(pair[0], 16).unwrap();
            let units: Vec<u16> = (0..pair[1].len())
                .step_by(4)
                .map(|start| u16::from_str_radix(&pair[1][start..start + 4], 16).unwrap())
                .collect();
            map.insert(glyph, String::from_utf16(&units).unwrap());
        }
    }
    map
}

/// The bytes of every literal string a content stream holds, escapes decoded.
fn shown_strings(content: &[u8]) -> Vec<Vec<u8>> {
    let mut strings = Vec::new();
    let mut index = 0;
    while index < content.len() {
        if content[index] != b'(' {
            index += 1;
            continue;
        }
        index += 1;
        let mut bytes = Vec::new();
        let mut depth = 0;
        while index < content.len() {
            match content[index] {
                b'\\' => {
                    index += 1;
                    let escaped = content[index];
                    if escaped.is_ascii_digit() {
                        let digits: Vec<u8> = content[index..]
                            .iter()
                            .take(3)
                            .take_while(|byte| (b'0'..=b'7').contains(byte))
                            .copied()
                            .collect();
                        let value = digits
                            .iter()
                            .fold(0u32, |sum, digit| sum * 8 + u32::from(digit - b'0'));
                        bytes.push(value as u8);
                        index += digits.len();
                        continue;
                    }
                    bytes.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 0x08,
                        b'f' => 0x0C,
                        other => other,
                    });
                }
                b'(' => {
                    depth += 1;
                    bytes.push(b'(');
                }
                b')' if depth > 0 => {
                    depth -= 1;
                    bytes.push(b')');
                }
                b')' => break,
                byte => bytes.push(byte),
            }
            index += 1;
        }
        strings.push(bytes);
        index += 1;
    }
    strings
}

/// A PDF text string as text: UTF-16BE after its byte order mark, else bytes.
pub fn text(object: &Object) -> String {
    let bytes = object.as_str().unwrap();
    match bytes {
        [0xFE, 0xFF, rest @ ..] => String::from_utf16(
            &rest
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        _ => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

/// A rectangle as four numbers.
pub fn rect(object: &Object) -> [f32; 4] {
    let values: Vec<f32> = object
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_float().unwrap())
        .collect();
    [values[0], values[1], values[2], values[3]]
}

/// Whether `haystack` holds `needle`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
