//! The hashes of a render, which say whether it changed: the pixels of each
//! page image and the parts of the PDF, independent of object numbers, and
//! the baseline files they are kept in.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;

use lopdf::{Object, ObjectId};
use sha2::{Digest, Sha256};

use crate::checks::baseline_path;
use crate::pdf::Pdf;

/// The hashes of a render: one entry per page, then the parts of the PDF
/// that belong to no page.
#[derive(Clone, Debug, PartialEq)]
pub struct Fingerprint {
    pub pages: Vec<PageHashes>,
    /// What the content streams name: fonts with their widths, ToUnicode
    /// maps, pictures, graphics states and shadings, metadata left out.
    pub resources: String,
    /// The outline and the structure tree.
    pub outline_and_tags: String,
    /// The catalog's own entries.
    pub catalog: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageHashes {
    /// The decoded pixels of the page image, with its size.
    pub pixels: String,
    /// The page's content stream, inflated.
    pub content: String,
    /// Its link annotations.
    pub annotations: String,
    /// The rest of its page dictionary.
    pub page: String,
}

/// The decoded pixels of a page image.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    pub fn decode(png_bytes: &[u8]) -> Self {
        let mut reader = png::Decoder::new(Cursor::new(png_bytes))
            .read_info()
            .expect("the page image is a PNG");
        let mut buffer = vec![0; reader.output_buffer_size().expect("the image fits")];
        let info = reader.next_frame(&mut buffer).expect("the PNG decodes");
        assert_eq!(
            (info.color_type, info.bit_depth),
            (png::ColorType::Rgba, png::BitDepth::Eight),
            "page images are 8-bit RGBA"
        );
        buffer.truncate(info.buffer_size());
        Self {
            width: info.width,
            height: info.height,
            rgba: buffer,
        }
    }

    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.width.to_be_bytes());
        hasher.update(self.height.to_be_bytes());
        hasher.update(&self.rgba);
        hex(&hasher.finalize())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn fingerprint(pdf: &Pdf, pixels: &[Pixels]) -> Fingerprint {
    let page_ids = pdf.page_ids();
    let pages = page_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let dict = pdf.doc.get_dictionary(*id).expect("page dictionary");
            let annotations = dict
                .get(b"Annots")
                .map(|annots| canonical(pdf, annots, &[b"P", b"Parent"]))
                .unwrap_or_default();
            let mut rest = dict.clone();
            for key in [&b"Contents"[..], b"Annots", b"Parent", b"Resources"] {
                rest.remove(key);
            }
            PageHashes {
                pixels: pixels.get(index).map_or_else(String::new, Pixels::hash),
                content: sha256(&pdf.doc.get_page_content(*id)),
                annotations: sha256(&annotations),
                page: sha256(&canonical(pdf, &Object::Dictionary(rest), &[])),
            }
        })
        .collect();
    let catalog = pdf.doc.catalog().expect("catalog");
    let outline_and_tags: Vec<u8> = [&b"Outlines"[..], b"StructTreeRoot"]
        .into_iter()
        .flat_map(|key| {
            catalog
                .get(key)
                .map(|object| canonical(pdf, object, &[b"P", b"Parent", b"Prev", b"Last"]))
                .unwrap_or_default()
        })
        .collect();
    let mut own = catalog.clone();
    for key in [&b"Pages"[..], b"Outlines", b"StructTreeRoot", b"Metadata"] {
        own.remove(key);
    }
    Fingerprint {
        pages,
        resources: resources_hash(pdf),
        outline_and_tags: sha256(&outline_and_tags),
        catalog: sha256(&canonical(pdf, &Object::Dictionary(own), &[])),
    }
}

/// What the content streams name and the pixels do not show: for each page,
/// its resource dictionary with every font (widths included), picture,
/// graphics state and shading it refers to, then every other stream
/// (annotation appearances, colour profiles), metadata left out. Streams are
/// inflated, and nothing depends on object numbers.
pub fn resources_hash(pdf: &Pdf) -> String {
    let mut streams = HashMap::new();
    let mut parts: Vec<String> = pdf
        .page_ids()
        .into_iter()
        .map(|id| {
            let page = pdf.doc.get_dictionary(id).expect("page dictionary");
            page.get(b"Resources")
                .map(|resources| sha256(&canonical_with(pdf, resources, &[], &mut streams)))
                .unwrap_or_default()
        })
        .collect();
    let contents: HashSet<ObjectId> = pdf
        .page_ids()
        .into_iter()
        .flat_map(|page| pdf.doc.get_page_contents(page))
        .collect();
    let mut others: Vec<String> = pdf
        .doc
        .objects
        .iter()
        .filter(|(id, _)| !contents.contains(id) && !streams.contains_key(*id))
        .filter(|(_, object)| {
            object.as_stream().is_ok_and(|stream| {
                let kind = stream.dict.get(b"Type").and_then(Object::as_name).ok();
                !matches!(kind, Some(b"Metadata" | b"XRef" | b"ObjStm"))
            })
        })
        .map(|(_, object)| sha256(&canonical(pdf, object, &[])))
        .collect();
    others.sort();
    parts.push(String::new());
    parts.extend(others);
    sha256(parts.join("\n").as_bytes())
}

/// A PDF object written out with what it refers to, independent of object
/// numbers: a page is written as its index, an object met before as the
/// order it was first met in, a stream as its dictionary and the hash of its
/// inflated bytes (how it is compressed left out), and `skip`ped keys are
/// left out (back references that would loop).
pub fn canonical(pdf: &Pdf, object: &Object, skip: &[&[u8]]) -> Vec<u8> {
    canonical_with(pdf, object, skip, &mut HashMap::new())
}

/// [`canonical`], with the hashes of the stream bytes already inflated, so
/// that the pages sharing a font inflate it once.
pub fn canonical_with(
    pdf: &Pdf,
    object: &Object,
    skip: &[&[u8]],
    streams: &mut HashMap<ObjectId, String>,
) -> Vec<u8> {
    struct Writer<'a> {
        pdf: &'a Pdf,
        pages: HashMap<ObjectId, usize>,
        met: HashMap<ObjectId, usize>,
        skip: &'a [&'a [u8]],
        streams: &'a mut HashMap<ObjectId, String>,
        out: Vec<u8>,
    }
    impl Writer<'_> {
        fn write(&mut self, object: &Object, depth: usize) {
            // An outline is a chain of `Next` items, one level each.
            assert!(depth < 4096, "a PDF object nests too deep to hash");
            match object {
                Object::Reference(id) => {
                    if let Some(page) = self.pages.get(id) {
                        self.out.extend(format!("page {page}").as_bytes());
                    } else if let Some(order) = self.met.get(id) {
                        self.out.extend(format!("@{order}").as_bytes());
                    } else {
                        self.met.insert(*id, self.met.len());
                        match self.pdf.doc.get_object(*id) {
                            Ok(Object::Stream(stream)) => {
                                let bytes = self
                                    .streams
                                    .entry(*id)
                                    .or_insert_with(|| stream_bytes_hash(stream))
                                    .clone();
                                self.write_stream(stream, &bytes, depth + 1);
                            }
                            Ok(target) => self.write(target, depth + 1),
                            Err(_) => self.out.extend(b"null"),
                        }
                    }
                }
                Object::Dictionary(dict) => {
                    let mut entries: Vec<_> = dict
                        .iter()
                        .filter(|(key, _)| !self.skip.contains(&key.as_slice()))
                        .collect();
                    entries.sort_by(|a, b| a.0.cmp(b.0));
                    self.out.push(b'<');
                    for (key, value) in entries {
                        self.out.push(b'/');
                        self.out.extend(key.as_slice());
                        self.out.push(b' ');
                        self.write(value, depth + 1);
                        self.out.push(b' ');
                    }
                    self.out.push(b'>');
                }
                Object::Array(items) => {
                    self.out.push(b'[');
                    for item in items {
                        self.write(item, depth + 1);
                        self.out.push(b' ');
                    }
                    self.out.push(b']');
                }
                Object::Stream(stream) => {
                    self.write_stream(stream, &stream_bytes_hash(stream), depth + 1)
                }
                Object::String(bytes, _) => {
                    self.out.push(b'(');
                    self.out.extend(bytes);
                    self.out.push(b')');
                }
                Object::Name(name) => {
                    self.out.push(b'/');
                    self.out.extend(name);
                }
                other => self.out.extend(format!("{other:?}").as_bytes()),
            }
        }

        fn write_stream(&mut self, stream: &lopdf::Stream, bytes: &str, depth: usize) {
            let mut dict = stream.dict.clone();
            for key in [&b"Length"[..], b"Filter", b"DecodeParms"] {
                dict.remove(key);
            }
            self.write(&Object::Dictionary(dict), depth);
            self.out.extend(bytes.as_bytes());
        }
    }
    let pages = pdf
        .page_ids()
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let mut writer = Writer {
        pdf,
        pages,
        met: HashMap::new(),
        skip,
        streams,
        out: Vec::new(),
    };
    writer.write(object, 0);
    writer.out
}

/// The hash of a stream's bytes, inflated when they are compressed.
pub fn stream_bytes_hash(stream: &lopdf::Stream) -> String {
    sha256(
        &stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone()),
    )
}

/// A baseline file as JSON, one page a line, so that a diff shows which
/// pages moved.
pub fn baseline_json(path: &str, reason: &str, fingerprint: &Fingerprint) -> String {
    let quote = |text: &str| serde_json::to_string(text).expect("a JSON string");
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"file\": {},\n", quote(path)));
    out.push_str(&format!("  \"reason\": {},\n", quote(reason)));
    out.push_str(&format!(
        "  \"resources\": \"{}\",\n",
        fingerprint.resources
    ));
    out.push_str(&format!(
        "  \"outline_and_tags\": \"{}\",\n",
        fingerprint.outline_and_tags
    ));
    out.push_str(&format!("  \"catalog\": \"{}\",\n", fingerprint.catalog));
    out.push_str("  \"pages\": [\n");
    let last = fingerprint.pages.len().saturating_sub(1);
    for (index, page) in fingerprint.pages.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"page\": {}, \"pixels\": \"{}\", \"content\": \"{}\", \"annotations\": \"{}\", \"dictionary\": \"{}\"}}{}\n",
            index + 1,
            page.pixels,
            page.content,
            page.annotations,
            page.page,
            if index == last { "" } else { "," }
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

/// A baseline file read back: its reason and fingerprint.
pub fn read_baseline(path: &str) -> Option<(String, Fingerprint)> {
    let text = fs::read_to_string(baseline_path(path)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).expect("a baseline file is JSON");
    let field = |value: &serde_json::Value, key: &str| {
        value[key]
            .as_str()
            .unwrap_or_else(|| panic!("{path}: baseline field {key}"))
            .to_owned()
    };
    let pages = json["pages"]
        .as_array()
        .expect("baseline pages")
        .iter()
        .map(|page| PageHashes {
            pixels: field(page, "pixels"),
            content: field(page, "content"),
            annotations: field(page, "annotations"),
            page: field(page, "dictionary"),
        })
        .collect();
    Some((
        field(&json, "reason"),
        Fingerprint {
            pages,
            resources: field(&json, "resources"),
            outline_and_tags: field(&json, "outline_and_tags"),
            catalog: field(&json, "catalog"),
        },
    ))
}

/// What moved between a baseline and a render, one line per page and
/// entry; empty when nothing did.
pub fn moved(expected: &Fingerprint, found: &Fingerprint) -> Vec<String> {
    let mut lines = Vec::new();
    if expected.pages.len() != found.pages.len() {
        lines.push(format!(
            "{} pages, the baseline has {}",
            found.pages.len(),
            expected.pages.len()
        ));
    }
    for (index, (was, now)) in expected.pages.iter().zip(&found.pages).enumerate() {
        let entries = [
            ("pixels", &was.pixels, &now.pixels),
            ("content", &was.content, &now.content),
            ("annotations", &was.annotations, &now.annotations),
            ("dictionary", &was.page, &now.page),
        ];
        let changed: Vec<&str> = entries
            .iter()
            .filter(|(_, was, now)| was != now)
            .map(|(name, ..)| *name)
            .collect();
        if !changed.is_empty() {
            lines.push(format!("page {}: {}", index + 1, changed.join(", ")));
        }
    }
    for (name, was, now) in [
        ("resources", &expected.resources, &found.resources),
        (
            "outline and tags",
            &expected.outline_and_tags,
            &found.outline_and_tags,
        ),
        ("catalog", &expected.catalog, &found.catalog),
    ] {
        if was != now {
            lines.push(name.to_owned());
        }
    }
    lines
}
