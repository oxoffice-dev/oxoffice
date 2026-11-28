//! The shared strings part (`CT_Sst`), read at the first shared string a cell
//! reads: the plain text of each item, its runs and phonetics kept as
//! written. New items are appended before the end of the list, `count` and
//! `uniqueCount` updated; items are never removed or reordered, so the `v`
//! of every `t="s"` cell stays valid.

use std::ops::Range;
use std::sync::Arc;

use oxoffice_core::OxmlError;
use oxoffice_core::xml_element::ElementSpan;

use crate::namespaces::MAIN_NAMESPACES;
use crate::xml::{
    Child, Element, Node, Opened, Result, Splice, Tag, XmlSource, append_to, prefix_of, qualified,
};
use crate::xstring;

/// A string item (`si` of the shared strings, `is` of a cell): its text in
/// `t`, or in the `t` of its runs (`r`), and its phonetic runs.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_Rst(pub Element);

impl CT_Rst {
    /// The item `xml` holds.
    pub fn parse(xml: &[u8]) -> Result<Self> {
        Element::parse(xml).map(CT_Rst)
    }

    /// The text, runs joined, phonetic runs left out, escapes read.
    pub fn plain_text(&self) -> String {
        let mut stored = String::new();
        for child in self.0.elements() {
            if child.tag.is("t") {
                stored.push_str(&child.text());
            } else if child.tag.is("r") {
                for text in child.elements_named("t") {
                    stored.push_str(&text.text());
                }
            }
        }
        xstring::decode(&stored).into_owned()
    }

    /// Whether the text is in runs (`r`): rich text.
    pub fn is_rich(&self) -> bool {
        self.0.elements_named("r").next().is_some()
    }

    /// An item holding `text` in one `t`, its elements under `prefix`.
    pub fn plain(prefix: &str, local: &str, text: &str) -> Self {
        let stored = xstring::encode(text);
        let mut t = Element::new(&qualified(prefix, "t"));
        let edge = |c: Option<char>| c.is_some_and(|c| matches!(c, ' ' | '\t' | '\n'));
        if edge(stored.chars().next()) || edge(stored.chars().last()) {
            t.tag.set("xml:space", Some("preserve"));
        }
        if !stored.is_empty() {
            t.children.push(Node::Text(stored.into_owned()));
        }
        let mut item = Element::new(&qualified(prefix, local));
        item.children.push(Node::Element(t));
        CT_Rst(item)
    }
}

/// An item of the shared strings: where it was read, its plain text, and
/// whether it is rich text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedString {
    pub range: Range<usize>,
    pub text: String,
    pub rich: bool,
}

/// The shared strings part. See the [module](self).
#[derive(Clone, Debug)]
pub struct SharedStrings {
    source: Arc<Vec<u8>>,
    root: ElementSpan,
    root_tag: Tag,
    prefix: String,
    items: Vec<SharedString>,
    /// Where an appended item goes: after the last item, else before what
    /// follows the items.
    append_at: Option<usize>,
    added: Vec<CT_Rst>,
    /// Cells pointing at an item added, less those removed: what `count`
    /// moves by.
    references_added: i64,
}

impl SharedStrings {
    pub fn parse(source: Arc<Vec<u8>>) -> Result<Self> {
        let xml: &[u8] = &source;
        let mut reader = XmlSource::new(xml);
        let root = reader.root()?;
        if !root.is("sst") {
            return Err(OxmlError::UnexpectedElement(format!(
                "{} where sst was expected",
                String::from_utf8_lossy(root.start.name().as_ref())
            )));
        }
        let root_tag = Tag::read(&root.start);
        let mut span = ElementSpan {
            name: root_tag.name(),
            start_tag: root.tag.clone(),
            content: root.tag.end..root.tag.end,
            end_tag: root.tag.end..root.tag.end,
        };
        let (mut items, mut append_at) = (Vec::new(), None);
        if !root.empty {
            loop {
                match reader.next_child()? {
                    Child::Element(item) if item.is("si") => {
                        let (text, rich) = read_item(&mut reader, &item)?;
                        let range = item.tag.start..reader.position();
                        append_at = Some(range.end);
                        items.push(SharedString { range, text, rich });
                    }
                    Child::Element(other) => {
                        let range = reader.finish(&other)?.range();
                        append_at.get_or_insert(range.start);
                    }
                    Child::Raw(_) => {}
                    Child::End(end) => {
                        span.content = root.tag.end..end.start;
                        span.end_tag = end;
                        break;
                    }
                }
            }
        }
        Ok(SharedStrings {
            prefix: prefix_of(&root_tag, &MAIN_NAMESPACES).unwrap_or_default(),
            source,
            root: span,
            root_tag,
            items,
            append_at,
            added: Vec::new(),
            references_added: 0,
        })
    }

    /// A new, empty part in `namespace`, for a workbook that has none.
    pub fn empty(namespace: &str) -> Self {
        let namespace = oxoffice_core::xml::escape_attribute_value(namespace);
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"{namespace}\" count=\"0\" uniqueCount=\"0\"/>"
        );
        Self::parse(Arc::new(xml.into_bytes())).expect("the empty part reads")
    }

    pub fn len(&self) -> usize {
        self.items.len() + self.added.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The plain text of item `index`.
    pub fn text(&self, index: usize) -> Option<String> {
        match self.items.get(index) {
            Some(item) => Some(item.text.clone()),
            None => self
                .added
                .get(index - self.items.len())
                .map(CT_Rst::plain_text),
        }
    }

    /// The items read, with where they were read.
    pub fn items(&self) -> &[SharedString] {
        &self.items
    }

    /// Item `index` read whole: its runs and phonetic runs.
    pub fn item(&self, index: usize) -> Option<Result<CT_Rst>> {
        match self.items.get(index) {
            Some(item) => Some(CT_Rst::parse(&self.source[item.range.clone()])),
            None => self.added.get(index - self.items.len()).cloned().map(Ok),
        }
    }

    /// Appends an item holding `text` and gives its index.
    pub fn push_plain(&mut self, text: &str) -> usize {
        self.added.push(CT_Rst::plain(&self.prefix, "si", text));
        self.len() - 1
    }

    /// Counts `references` more cells pointing at an item (fewer when
    /// negative: cells that no longer do), for `count`.
    pub fn add_references(&mut self, references: i64) {
        self.references_added += references;
    }

    pub fn is_changed(&self) -> bool {
        !self.added.is_empty() || self.references_added != 0
    }

    /// The part as it stands: its source bytes when nothing changed, else
    /// the source with the new items and the counts spliced in.
    pub fn to_xml(&self) -> Vec<u8> {
        if !self.is_changed() {
            return self.source.to_vec();
        }
        let mut tag = self.root_tag.clone();
        if tag.get("uniqueCount").is_some() {
            tag.set("uniqueCount", Some(&self.len().to_string()));
        }
        if let Some(count) = tag.u32("count") {
            let count = (i64::from(count) + self.references_added).max(0);
            tag.set("count", Some(&count.to_string()));
        }
        let mut content = Vec::new();
        for item in &self.added {
            item.0.write(&mut content);
        }
        let mut splice = Splice::default();
        match self.append_at.filter(|_| !self.root.is_self_closing()) {
            Some(at) => {
                if tag != self.root_tag {
                    let mut start = Vec::new();
                    tag.write_start(&mut start);
                    splice.replace(self.root.start_tag.clone(), start);
                }
                splice.insert(at, content);
            }
            None => append_to(&mut splice, &self.root, &self.root_tag, Some(&tag), content),
        }
        splice.apply(&self.source)
    }

    /// The part with every item written again by its writer, in place: what
    /// a test compares with the source.
    #[doc(hidden)]
    pub fn to_xml_through_writers(&self) -> Result<Vec<u8>> {
        let mut splice = Splice::default();
        for item in &self.items {
            let written = CT_Rst::parse(&self.source[item.range.clone()])?.0.to_xml();
            splice.replace(item.range.clone(), written);
        }
        Ok(splice.apply(&self.source))
    }
}

/// Reads an `si` to its end: its plain text and whether it has runs.
fn read_item(reader: &mut XmlSource<'_>, item: &Opened<'_>) -> Result<(String, bool)> {
    let (mut stored, mut rich) = (String::new(), false);
    if item.empty {
        return Ok((stored, rich));
    }
    loop {
        match reader.next_child()? {
            Child::Element(text) if text.is("t") => stored.push_str(&reader.text(&text)?),
            Child::Element(run) if run.is("r") => {
                rich = true;
                if run.empty {
                    continue;
                }
                loop {
                    match reader.next_child()? {
                        Child::Element(text) if text.is("t") => {
                            stored.push_str(&reader.text(&text)?);
                        }
                        Child::Element(other) => {
                            reader.finish(&other)?;
                        }
                        Child::Raw(_) => {}
                        Child::End(_) => break,
                    }
                }
            }
            Child::Element(other) => {
                reader.finish(&other)?;
            }
            Child::Raw(_) => {}
            Child::End(_) => return Ok((xstring::decode(&stored).into_owned(), rich)),
        }
    }
}
