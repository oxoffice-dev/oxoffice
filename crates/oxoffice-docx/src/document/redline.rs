//! The redline of one paragraph: the paragraph before an edit and after it,
//! compared word by word, written as the deletions and insertions of the
//! tracked changes session under way (spec of 2026-11-27, writing revisions
//! and comments).
//!
//! Both paragraphs are read as leaves (words, marks, opaque content and
//! markers), each under the path of the elements that hold it: links and
//! controls, revisions, the run. The words the edit kept keep their element;
//! a removed word goes into a new deletion just above its run, nested in
//! another author's insertion and dropped from the session's author's own; the
//! new words go into a new insertion after what the edit removed. The result
//! must resolve as the edit and as the document before it.

use super::tracked::{ReplacePattern, Tracking};
use super::*;
use crate::revision::{Resolution, RevisionScope, resolve_story_xml};

/// An element of the paragraph that holds leaves.
#[derive(Clone, Debug)]
struct Container {
    /// The start tag as written (a control: also its properties and the
    /// start tag of its content). Empty for a new revision until its id is.
    open: String,
    /// The end tag (a control: the end tags of its content and of itself).
    end: String,
    kind: ContainerKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContainerKind {
    /// A run, with its properties written after its start tag.
    Run { properties: String, props: Props },
    /// `w:ins` or `w:moveTo`, whose content the accepted view keeps.
    Inserted { author: String, id: Option<i32> },
    /// `w:del` (`w:delText`) or `w:moveFrom` (`w:t`), whose content it removes.
    Removed { deletion: bool },
    /// A link, an inline control, a smart tag, custom XML, `w:dir`, `w:bdo`:
    /// an element that holds text without changing it, known by its start.
    Holder { identity: String },
    /// A revision of this session; its id is given when the output is written.
    New { deletion: bool },
    /// A later part of the insertion `of` that new text split, which takes
    /// an id of its own when the output is written.
    Split { of: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    /// A word, or one other character, of a `w:t` or `w:delText`.
    Text(String),
    /// A run child the text reads as characters: a tab, a break, a symbol.
    Mark(String),
    /// Content an edit may keep but, in this version, not add or remove: a
    /// drawing, a field character or code, a reference, an equation.
    Opaque(String),
    /// What has no width and stays where it stood: a range marker, a last
    /// rendered page break; empty for an element that held nothing.
    Marker(String),
}

#[derive(Clone, Debug)]
struct Leaf {
    path: Vec<usize>,
    item: Item,
}

/// A paragraph read as leaves.
#[derive(Clone, Debug)]
struct Flat {
    start: String,
    end: String,
    /// The `w:` prefix of the paragraph, which new elements take.
    prefix: String,
    properties: String,
    /// The paragraph's properties read for a property change.
    props: Props,
    containers: Vec<Container>,
    leaves: Vec<Leaf>,
}

impl Flat {
    fn in_view(&self, leaf: &Leaf) -> bool {
        !leaf.path.iter().any(|&container| {
            matches!(
                self.containers[container].kind,
                ContainerKind::Removed { .. } | ContainerKind::New { deletion: true }
            )
        })
    }

    fn token(&self, leaf: &Leaf) -> bool {
        !matches!(leaf.item, Item::Marker(_)) && self.in_view(leaf)
    }

    fn run_properties(&self, leaf: &Leaf) -> &str {
        leaf.path
            .iter()
            .rev()
            .find_map(|&container| match &self.containers[container].kind {
                ContainerKind::Run { props, .. } => Some(props.base.as_str()),
                _ => None,
            })
            .unwrap_or("")
    }

    fn holders(&self, leaf: &Leaf) -> Vec<&str> {
        leaf.path
            .iter()
            .filter_map(|&container| match &self.containers[container].kind {
                ContainerKind::Holder { identity } => Some(identity.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The range markers (bookmarks, comments, permissions), in order.
    fn range_markers(&self) -> Vec<&str> {
        self.leaves
            .iter()
            .filter_map(|leaf| match &leaf.item {
                Item::Marker(xml)
                    if !xml.is_empty()
                        && !xml.contains("lastRenderedPageBreak")
                        && !xml.contains("proofErr") =>
                {
                    Some(xml.as_str())
                }
                _ => None,
            })
            .collect()
    }
}

/// A property element (`w:rPr`, `w:pPr`) read for a property change: its
/// tags, its properties, the children it keeps apart from them (a
/// paragraph's mark properties and section), and the old value an earlier
/// change of it holds, `Some("")` for one that holds none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Props {
    open: String,
    end: String,
    base: String,
    kept: String,
    prior: Option<String>,
}

/// Where a new property change's id goes, until the ids are given.
const ID_MARK: &str = "\u{1}";

fn props(xml: &str, node: &Node, change: &str, kept: &[&str]) -> Result<Props> {
    let mut read = Props {
        open: node.open(xml)?,
        end: node.end_tag(xml),
        ..Props::default()
    };
    for child in &node.children {
        let Child::Element(element) = child else {
            continue;
        };
        let raw = &xml[element.span()];
        if element.word && element.local == change {
            read.prior = Some(
                element
                    .children
                    .iter()
                    .find_map(|old| match old {
                        Child::Element(old) => Some(xml[old.span()].to_owned()),
                        Child::Text(_) => None,
                    })
                    .unwrap_or_default(),
            );
        } else if element.word && kept.contains(&element.local.as_str()) {
            read.kept.push_str(raw);
        } else {
            read.base.push_str(raw);
        }
    }
    Ok(read)
}

/// Properties `new` as a property element `local` whose change `change`
/// holds the old ones, from `old`: the value an earlier change holds, if
/// any, so that one change holds the properties from before the first, as
/// Word keeps it (Word check W4). Its id is [`ID_MARK`] until the ids are
/// given.
fn changed_props(
    new: &Props,
    old: &Props,
    prefix: &str,
    local: &str,
    change: &str,
    tracking: &Tracking,
) -> String {
    let prior = match &old.prior {
        Some(prior) if !prior.is_empty() => prior.clone(),
        Some(_) => format!("<{prefix}{local}/>"),
        None if old.base.is_empty() => format!("<{prefix}{local}/>"),
        None => format!("<{prefix}{local}>{}</{prefix}{local}>", old.base),
    };
    let author = quick_xml::escape::escape(tracking.changes.author());
    let date = tracking.changes.timestamp().map_or(String::new(), |date| {
        format!(" {prefix}date=\"{}\"", quick_xml::escape::escape(date))
    });
    let (open, end) = match new.open.is_empty() {
        true => (format!("<{prefix}{local}>"), format!("</{prefix}{local}>")),
        false => (new.open.clone(), new.end.clone()),
    };
    format!(
        "{open}{}{}<{prefix}{change} {prefix}id=\"{ID_MARK}\" {prefix}author=\"{author}\"{date}>{prior}</{prefix}{change}>{end}",
        new.base, new.kept
    )
}

/// An edit a tracked session cannot write yet, said in plain words.
pub(super) fn refused(what: &str) -> Error {
    Error::Other(format!("{what} is not recorded as a tracked change yet"))
}

fn xml_error(error: impl std::fmt::Display) -> Error {
    Error::Other(format!("tracked change XML: {error}"))
}

/// Words (letters, digits and `_`, as the comparison's word granularity) and
/// single other characters.
fn words(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut in_word = false;
    for character in text.chars() {
        let word = character.is_alphanumeric() || character == '_';
        match tokens.last_mut() {
            Some(last) if word && in_word => last.push(character),
            _ => tokens.push(character.to_string()),
        }
        in_word = word;
    }
    tokens
}

const RUN_MARKS: [&str; 7] = [
    "tab",
    "br",
    "cr",
    "noBreakHyphen",
    "softHyphen",
    "sym",
    "ptab",
];
const HOLDERS: [&str; 5] = ["hyperlink", "smartTag", "customXml", "dir", "bdo"];
const RANGE_MARKERS: [&str; 7] = [
    "bookmarkStart",
    "bookmarkEnd",
    "commentRangeStart",
    "commentRangeEnd",
    "permStart",
    "permEnd",
    "proofErr",
];

/// One element of the source: its start tag, its end tag when it has one,
/// its children.
struct Node {
    name: String,
    local: String,
    word: bool,
    start: Range<usize>,
    end: Option<Range<usize>>,
    children: Vec<Child>,
    author: String,
    id: Option<i32>,
}

enum Child {
    Element(Node),
    Text(Range<usize>),
}

impl Node {
    fn span(&self) -> Range<usize> {
        self.start.start..self.end.as_ref().map_or(self.start.end, |end| end.end)
    }

    fn content(&self) -> Range<usize> {
        self.start.end..self.end.as_ref().map_or(self.start.end, |end| end.start)
    }

    /// The start tag as one that has content: `<x a="1"/>` as `<x a="1">`.
    fn open(&self, xml: &str) -> Result<String> {
        let tag =
            oxoffice_core::xml_element::start_tag(&xml[self.start.clone()]).map_err(xml_error)?;
        Ok(if tag.self_closing {
            format!(
                "{}>",
                &xml[self.start.start..self.start.start + tag.attributes_end]
            )
        } else {
            xml[self.start.clone()].to_owned()
        })
    }

    fn end_tag(&self, xml: &str) -> String {
        match &self.end {
            Some(end) => xml[end.clone()].to_owned(),
            None => format!("</{}>", self.name),
        }
    }
}

/// Read the element whose start tag `start` the reader just read.
fn element(
    reader: &mut NsReader<&[u8]>,
    xml: &str,
    start: &BytesStart<'_>,
    word: bool,
    span: Range<usize>,
    empty: bool,
) -> Result<Node> {
    let tag = oxoffice_core::xml_element::start_tag(&xml[span.clone()]).map_err(xml_error)?;
    let local = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
    let mut node = Node {
        name: tag.name.clone(),
        word,
        start: span.start..span.start + tag.range.end,
        end: None,
        children: Vec::new(),
        author: String::new(),
        id: None,
        local,
    };
    if word && matches!(node.local.as_str(), "ins" | "moveTo") {
        for attribute in start.attributes() {
            let attribute = attribute.map_err(xml_error)?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(xml_error)?;
            match local.as_ref() {
                b"author" => node.author = value.into_owned(),
                b"id" => node.id = value.parse().ok(),
                _ => {}
            }
        }
    }
    if empty {
        // An element written with an end tag and nothing inside is read as
        // one empty event, the reader after its end tag.
        if span.end > node.start.end {
            node.end = Some(node.start.end..span.end);
        }
        return Ok(node);
    }
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let is_word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(child) => {
                let child = child.into_owned();
                node.children.push(Child::Element(element(
                    reader,
                    xml,
                    &child,
                    is_word,
                    before..after,
                    false,
                )?));
            }
            Event::Empty(child) => {
                let child = child.into_owned();
                node.children.push(Child::Element(element(
                    reader,
                    xml,
                    &child,
                    is_word,
                    before..after,
                    true,
                )?));
            }
            Event::End(_) => {
                node.end = Some(before..after);
                return Ok(node);
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                node.children.push(Child::Text(before..after));
            }
            Event::Eof => return Err(xml_error("an element is not closed")),
            _ => {}
        }
    }
}

/// The paragraphs of `xml`: the root when it is one, else the paragraphs
/// directly in a `w:body`.
fn paragraphs(xml: &str) -> Result<Vec<Node>> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut body = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        let (start, empty) = match event {
            Event::Start(start) => (start.into_owned(), false),
            Event::Empty(start) => (start.into_owned(), true),
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            Event::Eof => return Ok(found),
            _ => continue,
        };
        let local = start.local_name();
        if word && local.as_ref() == b"p" && (depth == 0 || body == Some(depth)) {
            found.push(element(
                &mut reader,
                xml,
                &start,
                true,
                before..after,
                empty,
            )?);
            if depth == 0 {
                return Ok(found);
            }
            continue;
        }
        if word && local.as_ref() == b"body" && !empty {
            body = Some(depth + 1);
        }
        if !empty {
            depth += 1;
        }
    }
}

/// Read a paragraph as leaves.
fn flatten(xml: &str, paragraph: &Node) -> Result<Flat> {
    let prefix = paragraph
        .name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    let mut flat = Flat {
        start: paragraph.open(xml)?,
        end: paragraph.end_tag(xml),
        prefix,
        properties: String::new(),
        props: Props::default(),
        containers: Vec::new(),
        leaves: Vec::new(),
    };
    for child in &paragraph.children {
        match child {
            Child::Element(node) if node.word && node.local == "pPr" => {
                flat.properties = xml[node.span()].to_owned();
                flat.props = props(xml, node, "pPrChange", &["rPr", "sectPr"])?;
            }
            _ => flatten_child(xml, child, &mut Vec::new(), &mut flat)?,
        }
    }
    Ok(flat)
}

fn flatten_child(xml: &str, child: &Child, path: &mut Vec<usize>, flat: &mut Flat) -> Result<()> {
    let node = match child {
        Child::Text(span) if xml[span.clone()].trim().is_empty() => return Ok(()),
        Child::Text(_) => return Err(refused("an edit of text outside a run")),
        Child::Element(node) => node,
    };
    let leaf = |flat: &mut Flat, path: &[usize], item| {
        flat.leaves.push(Leaf {
            path: path.to_vec(),
            item,
        })
    };
    let whole = xml[node.span()].to_owned();
    if !node.word {
        leaf(flat, path, Item::Opaque(whole));
        return Ok(());
    }
    let local = node.local.as_str();
    let (open, end) = match local {
        "sdt" => {
            // An inline control holds its content's leaves; its properties
            // are part of the start written before them.
            let Some(position) = node.children.iter().position(
                |child| matches!(child, Child::Element(content) if content.word && content.local == "sdtContent"),
            ) else {
                leaf(flat, path, Item::Opaque(whole));
                return Ok(());
            };
            let Child::Element(content) = &node.children[position] else {
                unreachable!("found as an element")
            };
            let open = format!(
                "{}{}",
                &xml[node.start.start..content.start.start],
                content.open(xml)?
            );
            let end = format!(
                "{}{}",
                content.end_tag(xml),
                node.end
                    .as_ref()
                    .map_or(String::new(), |_| xml[content.span().end..node.span().end]
                        .to_owned())
            );
            flat.containers.push(Container {
                open: open.clone(),
                end,
                kind: ContainerKind::Holder { identity: open },
            });
            return enter(
                xml,
                &content.children,
                flat.containers.len() - 1,
                path,
                flat,
            );
        }
        _ => (node.open(xml)?, node.end_tag(xml)),
    };
    let kind = match local {
        "r" => ContainerKind::Run {
            properties: String::new(),
            props: Props::default(),
        },
        "ins" | "moveTo" => ContainerKind::Inserted {
            author: node.author.clone(),
            id: node.id,
        },
        "del" | "moveFrom" => ContainerKind::Removed {
            deletion: local == "del",
        },
        _ if HOLDERS.contains(&local) => ContainerKind::Holder {
            identity: open.clone(),
        },
        _ if RANGE_MARKERS.contains(&local)
            || local.ends_with("RangeStart")
            || local.ends_with("RangeEnd") =>
        {
            leaf(flat, path, Item::Marker(whole));
            return Ok(());
        }
        _ => {
            leaf(flat, path, Item::Opaque(whole));
            return Ok(());
        }
    };
    let run = matches!(kind, ContainerKind::Run { .. });
    flat.containers.push(Container { open, end, kind });
    let container = flat.containers.len() - 1;
    if !run {
        return enter(xml, &node.children, container, path, flat);
    }
    path.push(container);
    let count = flat.leaves.len();
    for child in &node.children {
        let Child::Element(item) = child else {
            continue;
        };
        let raw = xml[item.span()].to_owned();
        match (item.word, item.local.as_str()) {
            (true, "rPr") => {
                flat.containers[container].kind = ContainerKind::Run {
                    properties: raw,
                    props: props(xml, item, "rPrChange", &[])?,
                };
            }
            (true, "t" | "delText") => {
                let text = quick_xml::escape::unescape(&xml[item.content()]).map_err(xml_error)?;
                for word in words(&text) {
                    leaf(flat, path, Item::Text(word));
                }
            }
            (true, local) if RUN_MARKS.contains(&local) => leaf(flat, path, Item::Mark(raw)),
            (true, "lastRenderedPageBreak") => leaf(flat, path, Item::Marker(raw)),
            _ => leaf(flat, path, Item::Opaque(raw)),
        }
    }
    if flat.leaves.len() == count {
        leaf(flat, path, Item::Marker(String::new()));
    }
    path.pop();
    Ok(())
}

/// Read `children` under `container`, keeping the container when it holds
/// nothing.
fn enter(
    xml: &str,
    children: &[Child],
    container: usize,
    path: &mut Vec<usize>,
    flat: &mut Flat,
) -> Result<()> {
    path.push(container);
    let count = flat.leaves.len();
    for child in children {
        flatten_child(xml, child, path, flat)?;
    }
    if flat.leaves.len() == count {
        flat.leaves.push(Leaf {
            path: path.clone(),
            item: Item::Marker(String::new()),
        });
    }
    path.pop();
    Ok(())
}

fn flatten_one(xml: &str) -> Result<Flat> {
    let paragraph = paragraphs(xml)?
        .into_iter()
        .next()
        .ok_or_else(|| xml_error("no paragraph"))?;
    flatten(xml, &paragraph)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Keep(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// The longest common subsequence of `before` and `after`, as operations in
/// order, each run of changes written as its deletions then its insertions.
fn align<T: PartialEq>(before: &[T], after: &[T]) -> Vec<Op> {
    let prefix = before
        .iter()
        .zip(after)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let (left, right) = (
        &before[prefix..before.len() - suffix],
        &after[prefix..after.len() - suffix],
    );
    let mut ops = (0..prefix).map(|i| Op::Keep(i, i)).collect::<Vec<_>>();
    // A middle too large to align is replaced whole.
    if left.len().saturating_mul(right.len()) > 4_000_000 {
        ops.extend((0..left.len()).map(|i| Op::Delete(prefix + i)));
        ops.extend((0..right.len()).map(|j| Op::Insert(prefix + j)));
    } else {
        let width = right.len() + 1;
        let mut lcs = vec![0u32; (left.len() + 1) * width];
        for i in (0..left.len()).rev() {
            for j in (0..right.len()).rev() {
                lcs[i * width + j] = if left[i] == right[j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < left.len() || j < right.len() {
            if i < left.len() && j < right.len() && left[i] == right[j] {
                ops.push(Op::Keep(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if j == right.len()
                || (i < left.len() && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                ops.push(Op::Delete(prefix + i));
                i += 1;
            } else {
                ops.push(Op::Insert(prefix + j));
                j += 1;
            }
        }
    }
    let (before_end, after_end) = (before.len() - suffix, after.len() - suffix);
    ops.extend((0..suffix).map(|k| Op::Keep(before_end + k, after_end + k)));
    ops
}

/// Each run of changes as its deletions then its insertions.
fn grouped(ops: Vec<Op>) -> Vec<Op> {
    let mut output = Vec::with_capacity(ops.len());
    let mut deletions = Vec::new();
    let mut insertions = Vec::new();
    for op in ops {
        match op {
            Op::Delete(_) => deletions.push(op),
            Op::Insert(_) => insertions.push(op),
            Op::Keep(..) => {
                output.append(&mut deletions);
                output.append(&mut insertions);
                output.push(op);
            }
        }
    }
    output.append(&mut deletions);
    output.append(&mut insertions);
    output
}

/// A kept run of white space between two changes changes with them, as the
/// comparison's word granularity writes a rewritten passage.
fn absorb_spaces(ops: Vec<Op>, before: &[&Item]) -> Vec<Op> {
    let is_space = |op: &Op| match op {
        Op::Keep(i, _) => matches!(before[*i], Item::Text(text) if text.trim().is_empty()),
        _ => false,
    };
    let mut ops = ops;
    let mut index = 0;
    while index < ops.len() {
        if !matches!(ops[index], Op::Keep(..)) {
            index += 1;
            continue;
        }
        let end = index
            + ops[index..]
                .iter()
                .take_while(|op| matches!(op, Op::Keep(..)))
                .count();
        if index > 0 && end < ops.len() && ops[index..end].iter().all(is_space) {
            let replaced = ops[index..end]
                .iter()
                .flat_map(|op| match *op {
                    Op::Keep(i, j) => [Op::Delete(i), Op::Insert(j)],
                    _ => unreachable!("a kept stretch"),
                })
                .collect::<Vec<_>>();
            ops.splice(index..end, replaced);
        }
        index = end.max(index + 1);
    }
    grouped(ops)
}

impl Flat {
    /// The leaves the accepted view reads as content, in order.
    fn tokens(&self) -> Vec<usize> {
        (0..self.leaves.len())
            .filter(|&leaf| self.token(&self.leaves[leaf]))
            .collect()
    }
}

/// A token is the same before and after an edit when its text and its
/// holders are: text moved into a link is removed and inserted again, and
/// text kept with other run properties takes a property change.
fn key(flat: &Flat, leaf: usize) -> (&Item, Vec<&str>) {
    let leaf = &flat.leaves[leaf];
    (&leaf.item, flat.holders(leaf))
}

/// `flat` with each word split into its characters, so that a match may
/// start or end inside a word.
fn by_character(flat: &Flat) -> Flat {
    let mut split = flat.clone();
    split.leaves = flat
        .leaves
        .iter()
        .flat_map(|leaf| match &leaf.item {
            Item::Text(text) => text
                .chars()
                .map(|character| Leaf {
                    path: leaf.path.clone(),
                    item: Item::Text(character.to_string()),
                })
                .collect::<Vec<_>>(),
            _ => vec![leaf.clone()],
        })
        .collect();
    split
}

/// The text a token reads as for replacement: a tab as `\t`, a break as
/// `\n`, a non-breaking hyphen as `-`, other content as nothing.
fn token_text(item: &Item) -> String {
    match item {
        Item::Text(text) => text.clone(),
        Item::Mark(xml) => {
            let name = oxoffice_core::xml_element::start_tag(xml)
                .map(|tag| tag.name)
                .unwrap_or_default();
            match name.rsplit(':').next().unwrap_or_default() {
                "tab" | "ptab" => "\t".to_owned(),
                "br" | "cr" => "\n".to_owned(),
                "noBreakHyphen" => "-".to_owned(),
                _ => String::new(),
            }
        }
        Item::Opaque(_) | Item::Marker(_) => String::new(),
    }
}

/// The operations of a replacement: the tokens of each whole match the
/// replacement wrote deleted, those of its value inserted, every other token
/// kept, a match it could not reach included. `None` when the matches found
/// in the text before do not explain the text after.
fn replace_ops(
    old: &Flat,
    new: &Flat,
    old_tokens: &[usize],
    new_tokens: &[usize],
    pattern: &ReplacePattern,
) -> Option<Vec<Op>> {
    let read = |flat: &Flat, tokens: &[usize]| {
        let mut text = String::new();
        let mut spans = Vec::with_capacity(tokens.len());
        for &leaf in tokens {
            let piece = token_text(&flat.leaves[leaf].item);
            spans.push((text.len(), piece.len()));
            text.push_str(&piece);
        }
        (text, spans)
    };
    let (old_text, old_spans) = read(old, old_tokens);
    let (new_text, new_spans) = read(new, new_tokens);
    let mut matches = Vec::new();
    match pattern {
        ReplacePattern::Literal {
            placeholder,
            written,
        } => {
            if placeholder.is_empty() {
                return None;
            }
            for (at, _) in old_text.match_indices(placeholder.as_str()) {
                matches.push((at..at + placeholder.len(), written.clone()));
            }
        }
        ReplacePattern::Regex { regex, replacement } => {
            for captures in regex.captures_iter(&old_text) {
                let found = captures.get(0)?;
                if found.is_empty() {
                    return None;
                }
                let mut value = String::new();
                captures.expand(replacement, &mut value);
                matches.push((found.range(), value));
            }
        }
    }
    // The matches the replacement wrote: one it could not reach reads as
    // before, and so does one whose value is its own text. A match is taken
    // as written where the text after reads its value; a wrong guess fails
    // the comparison with the text after, and the paragraph is aligned word
    // by word.
    let mut old_ranges = Vec::with_capacity(matches.len());
    let mut new_ranges = Vec::with_capacity(matches.len());
    // How much of the text after the reading accounts for so far.
    let mut read = 0;
    let reads = |piece: &str, read: &mut usize| {
        let agrees = new_text[*read..].starts_with(piece);
        if agrees {
            *read += piece.len();
        }
        agrees
    };
    let mut at = 0;
    for (range, value) in matches {
        if !reads(&old_text[at..range.start], &mut read) {
            return None;
        }
        let found = &old_text[range.clone()];
        let start = read;
        if value != found && reads(&value, &mut read) {
            new_ranges.push(start..read);
            old_ranges.push(range.clone());
        } else if !reads(found, &mut read) {
            return None;
        }
        at = range.end;
    }
    if old_ranges.is_empty() || !reads(&old_text[at..], &mut read) || read != new_text.len() {
        return None;
    }
    let classify = |spans: &[(usize, usize)], ranges: &[Range<usize>]| {
        spans
            .iter()
            .map(|&(offset, width)| {
                ranges.iter().position(|range| {
                    range.start < offset && offset < range.end
                        || width > 0 && range.start == offset && offset < range.end
                })
            })
            .collect::<Vec<_>>()
    };
    let (old_class, new_class) = (
        classify(&old_spans, &old_ranges),
        classify(&new_spans, &new_ranges),
    );
    let mut ops = Vec::new();
    let (mut a, mut b) = (0, 0);
    for k in 0..=old_ranges.len() {
        while a < old_class.len()
            && b < new_class.len()
            && old_class[a].is_none()
            && new_class[b].is_none()
        {
            if key(old, old_tokens[a]) != key(new, new_tokens[b]) {
                return None;
            }
            ops.push(Op::Keep(a, b));
            a += 1;
            b += 1;
        }
        if k == old_ranges.len() {
            break;
        }
        while a < old_class.len() && old_class[a] == Some(k) {
            ops.push(Op::Delete(a));
            a += 1;
        }
        while b < new_class.len() && new_class[b] == Some(k) {
            ops.push(Op::Insert(b));
            b += 1;
        }
    }
    (a == old_class.len() && b == new_class.len()).then_some(ops)
}

/// Where an insertion goes: right after the leaf of a before token, or right
/// before it, or at the end of the paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    After(usize),
    Before(usize),
    End,
}

/// The redline of `after`, an edit of `before`, both paragraph XML with
/// their namespaces declared on their root.
pub(super) fn redline_xml(
    before: &str,
    after: &str,
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
) -> Result<String> {
    let old = flatten_one(before)?;
    let new = flatten_one(after)?;
    if old.props.kept != new.props.kept {
        return Err(refused(
            "a change of the paragraph mark's formatting or its section",
        ));
    }
    // A change of the paragraph's properties, written as one `w:pPrChange`
    // (Word check W5): a style or a numbering change included.
    let paragraph_changed = old.props.base != new.props.base;
    if old.range_markers() != new.range_markers() {
        return Err(refused(
            "an edit that adds, removes or reorders a bookmark or comment mark",
        ));
    }
    // An edit that changes no content, character by character, such as a
    // run split, is kept as it is: Word records nothing for it.
    let characters = |flat: &Flat| {
        let flat = by_character(flat);
        flat.tokens()
            .into_iter()
            .map(|leaf| {
                let leaf = &flat.leaves[leaf];
                (
                    leaf.item.clone(),
                    flat.run_properties(leaf).to_owned(),
                    flat.holders(leaf).join("\u{0}"),
                )
            })
            .collect::<Vec<_>>()
    };
    if !paragraph_changed && characters(&old) == characters(&new) {
        return Ok(after.to_owned());
    }
    // A replacement deletes each whole match and inserts the whole value, as
    // Word's tracked Replace All does, at each match the replacement wrote;
    // any other edit, or a replacement its matches do not explain, is aligned
    // word by word.
    let replaced = tracking.pattern.as_deref().and_then(|pattern| {
        let (old, new) = (by_character(&old), by_character(&new));
        let (old_tokens, new_tokens) = (old.tokens(), new.tokens());
        replace_ops(&old, &new, &old_tokens, &new_tokens, pattern)
            .map(|ops| (old, new, old_tokens, new_tokens, ops))
    });
    let (old, new, old_tokens, new_tokens, ops) = match replaced {
        Some(replaced) => replaced,
        None => {
            let (old_tokens, new_tokens) = (old.tokens(), new.tokens());
            let item = |flat: &Flat, leaf: usize| flat.leaves[leaf].item.clone();
            let old_items = old_tokens
                .iter()
                .map(|&l| item(&old, l))
                .collect::<Vec<_>>();
            let new_items = new_tokens
                .iter()
                .map(|&l| item(&new, l))
                .collect::<Vec<_>>();
            let old_keys = old_tokens.iter().map(|&l| key(&old, l)).collect::<Vec<_>>();
            let new_keys = new_tokens.iter().map(|&l| key(&new, l)).collect::<Vec<_>>();
            if old_items == new_items && old_keys != new_keys {
                return Err(refused("text moved into or out of a link or control"));
            }
            let items = old_items.iter().collect::<Vec<_>>();
            let ops = absorb_spaces(grouped(align(&old_keys, &new_keys)), &items);
            (old, new, old_tokens, new_tokens, ops)
        }
    };
    let author = tracking.changes.author();
    let own_session = tracking.ids();
    let own = |kind: &ContainerKind| {
        matches!(kind, ContainerKind::Inserted { author: by, id: Some(id) }
            if by == author && own_session.contains(id))
    };

    let mut output = old.clone();
    // What happens to each before token: kept, or deleted under this new
    // deletion; and the insertions placed around it.
    let mut deleted: HashMap<usize, Option<usize>> = HashMap::new();
    let mut placed: Vec<(Place, Vec<Leaf>)> = Vec::new();
    let mut index = 0;
    let mut last_kept: Option<usize> = None;
    while index < ops.len() {
        if let Op::Keep(i, _) = ops[index] {
            last_kept = Some(i);
            index += 1;
            continue;
        }
        let end = index
            + ops[index..]
                .iter()
                .take_while(|op| !matches!(op, Op::Keep(..)))
                .count();
        let next_kept = ops.get(end).map(|op| match op {
            Op::Keep(i, _) => *i,
            _ => unreachable!("a stretch ends at a kept token"),
        });
        let mut deletion = None;
        let mut last_deleted = None;
        for op in &ops[index..end] {
            if let Op::Delete(i) = *op {
                let leaf = &old.leaves[old_tokens[i]];
                if matches!(leaf.item, Item::Opaque(_)) {
                    return Err(refused("the removal of a drawing, a field or a reference"));
                }
                let mine = leaf.path.iter().any(|&c| {
                    matches!(&old.containers[c].kind, ContainerKind::Inserted { author: by, .. } if by == author)
                });
                if mine {
                    deleted.insert(i, None);
                } else {
                    let container = *deletion.get_or_insert_with(|| {
                        output.containers.push(Container {
                            open: String::new(),
                            end: format!("</{}del>", old.prefix),
                            kind: ContainerKind::New { deletion: true },
                        });
                        output.containers.len() - 1
                    });
                    deleted.insert(i, Some(container));
                }
                last_deleted = Some(i);
            }
        }
        let inserted = ops[index..end]
            .iter()
            .filter_map(|op| match op {
                Op::Insert(j) => Some(*j),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !inserted.is_empty() {
            let anchor = last_deleted.or(last_kept);
            let holders = new.holders(&new.leaves[new_tokens[inserted[0]]]);
            if inserted
                .iter()
                .any(|&j| new.holders(&new.leaves[new_tokens[j]]) != holders)
            {
                return Err(refused("an insertion across the edge of a link or control"));
            }
            let context = |i: usize| &old.leaves[old_tokens[i]];
            let (place, context) = match (anchor, next_kept) {
                (Some(i), _) if old.holders(context(i)) == holders => {
                    (Place::After(i), Some(context(i)))
                }
                (_, Some(k)) if old.holders(context(k)) == holders => {
                    (Place::Before(k), Some(context(k)))
                }
                _ if holders.is_empty() => (
                    anchor.map_or(next_kept.map_or(Place::End, Place::Before), Place::After),
                    None,
                ),
                _ => return Err(refused("text in a new link or control")),
            };
            // The holders and an insertion of this session the new text
            // joins; another insertion it falls in is split around it.
            let mut base = Vec::new();
            let mut joined = false;
            for &container in context.map_or(&[][..], |leaf| leaf.path.as_slice()) {
                match &old.containers[container].kind {
                    ContainerKind::Holder { .. } => base.push(container),
                    kind if own(kind) => {
                        base.push(container);
                        joined = true;
                    }
                    _ => {}
                }
            }
            if !joined {
                output.containers.push(Container {
                    open: String::new(),
                    end: format!("</{}ins>", old.prefix),
                    kind: ContainerKind::New { deletion: false },
                });
                base.push(output.containers.len() - 1);
            }
            let mut runs: HashMap<usize, usize> = HashMap::new();
            let mut leaves = Vec::new();
            for &j in &inserted {
                let leaf = &new.leaves[new_tokens[j]];
                if matches!(leaf.item, Item::Opaque(_)) {
                    return Err(refused(
                        "the insertion of a drawing, a field or a reference",
                    ));
                }
                let Some(&run) = leaf
                    .path
                    .last()
                    .filter(|&&c| matches!(new.containers[c].kind, ContainerKind::Run { .. }))
                else {
                    return Err(refused("an insertion outside a run"));
                };
                let copy = *runs.entry(run).or_insert_with(|| {
                    // The new run's properties, without a change it took
                    // from the run it was split from: the insertion records it.
                    let mut copy = new.containers[run].clone();
                    if let ContainerKind::Run { properties, props } = &mut copy.kind
                        && props.prior.take().is_some()
                    {
                        *properties =
                            format!("{}{}{}{}", props.open, props.base, props.kept, props.end);
                    }
                    output.containers.push(copy);
                    output.containers.len() - 1
                });
                let mut path = base.clone();
                path.push(copy);
                leaves.push(Leaf {
                    path,
                    item: leaf.item.clone(),
                });
            }
            placed.push((place, leaves));
        }
        index = end;
    }

    // The output leaves, in order.
    let token_of = old_tokens
        .iter()
        .enumerate()
        .map(|(token, &leaf)| (leaf, token))
        .collect::<HashMap<_, _>>();
    let mut leaves = Vec::with_capacity(old.leaves.len());
    let take = |place: Place, placed: &mut Vec<(Place, Vec<Leaf>)>, leaves: &mut Vec<Leaf>| {
        for (at, group) in placed.iter_mut() {
            if *at == place {
                leaves.append(group);
            }
        }
    };
    // A kept token whose run took other properties goes into a copy of its
    // run with those properties and a `w:rPrChange` of the old ones, one
    // copy per run and new properties: inside an insertion too, even this
    // author's own (Word check W3).
    let kept = ops
        .iter()
        .filter_map(|op| match *op {
            Op::Keep(a, b) => Some((a, b)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    // Each copy, and the run and properties it copies: a copy goes on only
    // while the leaves before it took it, so that each run element written
    // has a change, and an id, of its own.
    let mut changed_runs: HashMap<usize, (usize, String)> = HashMap::new();
    // The ids of the changes a new change of the same owner replaces.
    let mut replaced = Vec::new();
    if paragraph_changed && old.props.prior.is_some() {
        replaced.push(change_id_marked_in(&old.properties, "pPrChange").1);
    }
    for (position, leaf) in old.leaves.iter().enumerate() {
        let Some(&token) = token_of.get(&position) else {
            leaves.push(leaf.clone());
            continue;
        };
        take(Place::Before(token), &mut placed, &mut leaves);
        match deleted.get(&token) {
            None => {
                let mut leaf = leaf.clone();
                let new_leaf = kept.get(&token).map(|&b| &new.leaves[new_tokens[b]]);
                let run = leaf
                    .path
                    .iter()
                    .rposition(|&c| matches!(old.containers[c].kind, ContainerKind::Run { .. }));
                if let (Some(new_leaf), Some(at)) = (new_leaf, run) {
                    let new_run =
                        new_leaf
                            .path
                            .iter()
                            .rev()
                            .find_map(|&c| match &new.containers[c].kind {
                                ContainerKind::Run { props, .. } => Some(props),
                                _ => None,
                            });
                    let old_container = leaf.path[at];
                    let ContainerKind::Run {
                        props: old_props, ..
                    } = &old.containers[old_container].kind
                    else {
                        unreachable!("found as a run")
                    };
                    let empty = Props::default();
                    let new_props = new_run.unwrap_or(&empty);
                    if new_props.base != old_props.base {
                        let wanted = (old_container, new_props.base.clone());
                        let previous = leaves
                            .last()
                            .and_then(|previous: &Leaf| previous.path.get(at))
                            .copied()
                            .filter(|copy| changed_runs.get(copy) == Some(&wanted));
                        let copy = previous.unwrap_or_else(|| {
                            if old_props.prior.is_some()
                                && let ContainerKind::Run { properties, .. } =
                                    &old.containers[old_container].kind
                            {
                                replaced.push(change_id_marked(properties).1);
                            }
                            let props = new_props.clone();
                            output.containers.push(Container {
                                open: old.containers[old_container].open.clone(),
                                end: old.containers[old_container].end.clone(),
                                kind: ContainerKind::Run {
                                    properties: changed_props(
                                        new_props,
                                        old_props,
                                        &old.prefix,
                                        "rPr",
                                        "rPrChange",
                                        tracking,
                                    ),
                                    props,
                                },
                            });
                            changed_runs.insert(output.containers.len() - 1, wanted.clone());
                            output.containers.len() - 1
                        });
                        leaf.path[at] = copy;
                    }
                }
                leaves.push(leaf);
            }
            Some(None) => {}
            Some(Some(deletion)) => {
                let run = leaf
                    .path
                    .iter()
                    .rposition(|&c| matches!(old.containers[c].kind, ContainerKind::Run { .. }))
                    .ok_or_else(|| refused("the removal of content outside a run"))?;
                let mut path = leaf.path.clone();
                path.insert(run, *deletion);
                leaves.push(Leaf {
                    path,
                    item: leaf.item.clone(),
                });
            }
        }
        take(Place::After(token), &mut placed, &mut leaves);
    }
    take(Place::End, &mut placed, &mut leaves);
    adopt_markers(&mut leaves, &output.containers);
    let remarked = one_id_per_element(&mut leaves, &mut output.containers);
    output.leaves = leaves;
    output.start = new.start.clone();

    // Ids in document order, held once the result resolves as it should, so
    // that a refused edit leaves none taken.
    let timestamp = tracking.changes.timestamp();
    // The paragraph's property change first, as Word numbers a paragraph's
    // revisions; `usize::MAX` stands for it.
    let changed = changed_runs
        .keys()
        .copied()
        .chain(remarked.iter().map(|(copy, _)| *copy))
        .collect::<HashSet<_>>();
    let mut order = Vec::new();
    if paragraph_changed {
        order.push(usize::MAX);
    }
    for leaf in &output.leaves {
        for &container in &leaf.path {
            if (matches!(
                output.containers[container].kind,
                ContainerKind::New { .. } | ContainerKind::Split { .. }
            ) || changed.contains(&container))
                && !order.contains(&container)
            {
                order.push(container);
            }
        }
    }
    let ids = identifiers.free_revision_ids(order.len())?;
    let mut written = Vec::new();
    for (&container, &id) in order.iter().zip(&ids) {
        if container == usize::MAX {
            written.push(id);
            output.properties = changed_props(
                &new.props,
                &old.props,
                &old.prefix,
                "pPr",
                "pPrChange",
                tracking,
            )
            .replace(ID_MARK, &id.to_string());
            continue;
        }
        if changed.contains(&container) {
            // A later part of a change the session did not write stays the
            // other author's.
            if remarked
                .iter()
                .find(|(copy, _)| *copy == container)
                .is_none_or(|(_, old)| own_session.contains(old))
            {
                written.push(id);
            }
            if let ContainerKind::Run { properties, .. } = &mut output.containers[container].kind {
                *properties = properties.replace(ID_MARK, &id.to_string());
            }
            continue;
        }
        // A part of an insertion of this session stays the session's.
        if let ContainerKind::Split { of } = output.containers[container].kind
            && own(&output.containers[of].kind)
        {
            written.push(id);
        }
        let entry = &mut output.containers[container];
        let prefix = &old.prefix;
        match entry.kind {
            ContainerKind::New { deletion } => {
                written.push(id);
                let author = quick_xml::escape::escape(author);
                let date = timestamp.map_or(String::new(), |date| {
                    format!(" {prefix}date=\"{}\"", quick_xml::escape::escape(date))
                });
                let name = if deletion { "del" } else { "ins" };
                entry.open = format!(
                    "<{prefix}{name} {prefix}id=\"{id}\" {prefix}author=\"{author}\"{date}>"
                );
            }
            ContainerKind::Split { .. } => entry.open = with_id(&entry.open, id)?,
            _ => unreachable!("only new revisions and split halves take an id"),
        }
    }
    let redlined = write(&output);
    check(before, after, &redlined)?;
    identifiers.hold_revision_ids(&ids);
    tracking.record(&written);
    tracking.forget(&replaced);
    Ok(redlined)
}

/// One id per revision element, as Word writes them: a revision the output
/// writes as several elements, an earlier insertion that new text split or a
/// new revision that other content cut, gives each later element a copy of
/// its own, a `Split` or another `New`, which takes a new id (Word checks W1,
/// W2). An element goes on while the leaves around it share the path up to
/// it, as [`write`] keeps it open.
fn one_id_per_element(leaves: &mut [Leaf], containers: &mut Vec<Container>) -> Vec<(usize, i32)> {
    let mut seen = HashSet::new();
    let mut previous: Vec<usize> = Vec::new();
    let mut alias: HashMap<usize, usize> = HashMap::new();
    // The later parts of a run that holds a property change, each with the
    // id of the change it copies: their change takes an id of its own.
    let mut remarked = Vec::new();
    for leaf in leaves {
        let original = leaf.path.clone();
        for (depth, slot) in leaf.path.iter_mut().enumerate() {
            let container = *slot;
            let kind = match &containers[container].kind {
                ContainerKind::Inserted { .. } => Some(ContainerKind::Split { of: container }),
                ContainerKind::New { deletion } => Some(ContainerKind::New {
                    deletion: *deletion,
                }),
                ContainerKind::Run { properties, props }
                    if props.prior.is_some() && !properties.contains(ID_MARK) =>
                {
                    None
                }
                _ => continue,
            };
            let open = previous.get(..=depth) == original.get(..=depth);
            if !open {
                let part = if seen.insert(container) {
                    container
                } else {
                    let mut copy = containers[container].clone();
                    match (kind, &mut copy.kind) {
                        (Some(kind), _) => copy.kind = kind,
                        (None, ContainerKind::Run { properties, .. }) => {
                            let (marked, id) = change_id_marked(properties);
                            *properties = marked;
                            remarked.push((containers.len(), id));
                        }
                        (None, _) => unreachable!("a run holding a change"),
                    }
                    containers.push(copy);
                    containers.len() - 1
                };
                alias.insert(container, part);
            }
            *slot = alias[&container];
        }
        previous = original;
    }
    remarked
}

/// Run properties holding a `w:rPrChange` with its id as [`ID_MARK`], and
/// the id it had.
fn change_id_marked(properties: &str) -> (String, i32) {
    change_id_marked_in(properties, "rPrChange")
}

/// [`change_id_marked`] for the change element `local`.
fn change_id_marked_in(properties: &str, local: &str) -> (String, i32) {
    let Some(change) = properties.find(local) else {
        return (properties.to_owned(), -1);
    };
    let tag_end = properties[change..]
        .find('>')
        .map_or(properties.len(), |end| change + end);
    let Some(id) = properties[change..tag_end]
        .find("id=\"")
        .map(|at| change + at + 4)
    else {
        return (properties.to_owned(), -1);
    };
    let end = properties[id..].find('"').map_or(id, |end| id + end);
    let old = properties[id..end].parse().unwrap_or(-1);
    (
        format!("{}{ID_MARK}{}", &properties[..id], &properties[end..]),
        old,
    )
}

/// A revision start tag, as the paragraph wrote it, with its `w:id` set to
/// `id`. The tag carries no namespace declaration: the revision element and
/// its id share the WordprocessingML prefix.
fn with_id(tag: &str, id: i32) -> Result<String> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_str(tag);
    let (Event::Start(start) | Event::Empty(start)) = reader.read_event().map_err(xml_error)?
    else {
        return Err(xml_error("a revision start tag"));
    };
    let prefix = start.name().prefix().map(|prefix| prefix.as_ref().to_vec());
    let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
    let mut copy = BytesStart::new(name);
    for attribute in start.attributes() {
        let attribute = attribute.map_err(xml_error)?;
        let key = attribute.key;
        let word = key.prefix().map(|prefix| prefix.as_ref().to_vec()) == prefix;
        if word && key.local_name().as_ref() == b"id" {
            let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
            copy.push_attribute((key.as_str(), id.to_string().as_str()));
        } else {
            copy.push_attribute(attribute);
        }
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Start(copy)).map_err(xml_error)?;
    String::from_utf8(writer.into_inner()).map_err(xml_error)
}

/// A marker between two leaves removed by the same new deletion goes into it.
fn adopt_markers(leaves: &mut [Leaf], containers: &[Container]) {
    for index in 1..leaves.len().saturating_sub(1) {
        if !matches!(leaves[index].item, Item::Marker(_)) {
            continue;
        }
        let (previous, next) = (&leaves[index - 1].path, &leaves[index + 1].path);
        let Some(at) = previous
            .iter()
            .position(|&c| containers[c].kind == ContainerKind::New { deletion: true })
        else {
            continue;
        };
        if next.get(at) != previous.get(at) || next[..at] != previous[..at] {
            continue;
        }
        let marker = &leaves[index].path;
        let in_run = marker.len() == previous.len() - 1 && marker[at..] == previous[at + 1..];
        let outside = marker.as_slice() == &previous[..at];
        if in_run && marker[..at] == previous[..at] {
            leaves[index].path = previous.clone();
        } else if outside {
            leaves[index].path = previous[..=at].to_vec();
        }
    }
}

/// The XML of a paragraph read as leaves.
fn write(flat: &Flat) -> String {
    let mut out = String::with_capacity(flat.start.len() + flat.properties.len() + 256);
    out.push_str(&flat.start);
    out.push_str(&flat.properties);
    let mut open: Vec<usize> = Vec::new();
    let mut text: Option<(bool, String)> = None;
    let flush = |out: &mut String, text: &mut Option<(bool, String)>| {
        if let Some((deleted, value)) = text.take() {
            let name = if deleted { "delText" } else { "t" };
            out.push_str(&format!(
                "<{prefix}{name} xml:space=\"preserve\">{}</{prefix}{name}>",
                quick_xml::escape::partial_escape(&value),
                prefix = flat.prefix
            ));
        }
    };
    for leaf in &flat.leaves {
        let common = open
            .iter()
            .zip(&leaf.path)
            .take_while(|(left, right)| left == right)
            .count();
        if common < open.len() || common < leaf.path.len() {
            flush(&mut out, &mut text);
        }
        for &container in open[common..].iter().rev() {
            out.push_str(&flat.containers[container].end);
        }
        open.truncate(common);
        for &container in &leaf.path[common..] {
            let entry = &flat.containers[container];
            out.push_str(&entry.open);
            if let ContainerKind::Run { properties, .. } = &entry.kind {
                out.push_str(properties);
            }
            open.push(container);
        }
        match &leaf.item {
            Item::Text(value) => {
                let deleted = leaf.path.iter().any(|&c| {
                    matches!(
                        flat.containers[c].kind,
                        ContainerKind::Removed { deletion: true }
                            | ContainerKind::New { deletion: true }
                    )
                });
                match &mut text {
                    Some((kind, pending)) if *kind == deleted => pending.push_str(value),
                    _ => {
                        flush(&mut out, &mut text);
                        text = Some((deleted, value.clone()));
                    }
                }
            }
            Item::Mark(xml) | Item::Opaque(xml) | Item::Marker(xml) => {
                flush(&mut out, &mut text);
                out.push_str(xml);
            }
        }
    }
    flush(&mut out, &mut text);
    for &container in open.iter().rev() {
        out.push_str(&flat.containers[container].end);
    }
    out.push_str(&flat.end);
    out
}

/// What resolving a paragraph leaves, for comparison: its properties, each
/// character or mark with its run properties and holders, and its range
/// markers.
type Resolved = (String, Vec<(String, String, Vec<String>)>, Vec<String>);

fn resolved(xml: &str, resolution: Resolution) -> Result<Resolved> {
    let wrapped = format!("<w:document xmlns:w=\"{W_NS}\"><w:body>{xml}</w:body></w:document>");
    let (output, _) = resolve_story_xml(wrapped.as_bytes(), resolution, RevisionScope::All)?;
    let output = String::from_utf8(output).map_err(xml_error)?;
    let mut properties = String::new();
    let mut content = Vec::new();
    let mut markers = Vec::new();
    for paragraph in paragraphs(&output)? {
        let flat = flatten(&output, &paragraph)?;
        properties.push_str(&flat.properties);
        markers.extend(flat.range_markers().into_iter().map(str::to_owned));
        for leaf in flat.leaves.iter().filter(|leaf| flat.token(leaf)) {
            let holders = flat
                .holders(leaf)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let properties = flat.run_properties(leaf).to_owned();
            match &leaf.item {
                Item::Text(text) => content.extend(
                    text.chars()
                        .map(|c| (c.to_string(), properties.clone(), holders.clone())),
                ),
                Item::Mark(xml) | Item::Opaque(xml) => {
                    content.push((xml.clone(), properties, holders))
                }
                Item::Marker(_) => {}
            }
        }
    }
    markers.sort();
    Ok((properties, content, markers))
}

/// Accepting the redline gives what accepting the edited paragraph gives,
/// and rejecting it what rejecting the paragraph before the edit gives.
fn check(before: &str, after: &str, redlined: &str) -> Result<()> {
    if resolved(redlined, Resolution::Accept)? != resolved(after, Resolution::Accept)?
        || resolved(redlined, Resolution::Reject)? != resolved(before, Resolution::Reject)?
    {
        return Err(Error::Other(
            "this edit cannot be written as a tracked change: accepting it would not give the edit, \
             or rejecting it the paragraph before it"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The XML of a paragraph, its namespaces declared on its root.
fn paragraph_xml(paragraph: &CT_P) -> Result<String> {
    let fragment = ContentFragment::paragraph(paragraph.clone())?;
    String::from_utf8(fragment.xml).map_err(xml_error)
}

/// `after`, an edit of `before`, with the edit written as tracked changes of
/// the session under way.
pub(super) fn redline_paragraph(
    before: &CT_P,
    after: &CT_P,
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
) -> Result<CT_P> {
    if before == after {
        return Ok(after.clone());
    }
    let (old, new) = (paragraph_xml(before)?, paragraph_xml(after)?);
    if old == new {
        return Ok(after.clone());
    }
    let redlined = redline_xml(&old, &new, tracking, identifiers)?;
    if redlined == new {
        return Ok(after.clone());
    }
    Ok(CT_P::from_xml_fragment(redlined.as_bytes())?)
}

/// The outermost paragraphs of a part, as source ranges.
fn outer_paragraphs(xml: &str) -> Result<Vec<Range<usize>>> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut found = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        let (start, empty) = match event {
            Event::Start(start) => (start.into_owned(), false),
            Event::Empty(start) => (start.into_owned(), true),
            Event::Eof => return Ok(found),
            _ => continue,
        };
        if word && start.local_name().as_ref() == b"p" {
            found.push(element(&mut reader, xml, &start, true, before..after, empty)?.span());
        }
    }
}

/// Part `after`, an edit of part `before` that keeps its paragraphs, with
/// each paragraph the edit changed written as tracked changes: the notes
/// parts, which replacement edits as XML.
pub(super) fn redline_part_paragraphs(
    before: &[u8],
    after: &[u8],
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
) -> Result<Vec<u8>> {
    let (old, new) = (
        std::str::from_utf8(before).map_err(xml_error)?,
        std::str::from_utf8(after).map_err(xml_error)?,
    );
    let (old_spans, new_spans) = (outer_paragraphs(old)?, outer_paragraphs(new)?);
    if old_spans.len() != new_spans.len() {
        return Err(refused("an edit that adds or removes a note paragraph"));
    }
    let mut output = String::with_capacity(new.len());
    let mut copied = 0;
    for (old_span, new_span) in old_spans.into_iter().zip(new_spans) {
        if old[old_span.clone()] == new[new_span.clone()] {
            continue;
        }
        let fragment = |xml: &str, span: Range<usize>| -> Result<String> {
            let scope = story_namespace_scope_at(xml.as_bytes(), span.start)?;
            let closed = close_content_fragment_namespaces(xml[span].as_bytes(), &scope)?;
            String::from_utf8(closed).map_err(xml_error)
        };
        let redlined = redline_xml(
            &fragment(old, old_span)?,
            &fragment(new, new_span.clone())?,
            tracking,
            identifiers,
        )?;
        output.push_str(&new[copied..new_span.start]);
        output.push_str(&redlined);
        copied = new_span.end;
    }
    output.push_str(&new[copied..]);
    Ok(output.into_bytes())
}

/// The first WordprocessingML element `local` of `xml`.
fn first_word_element(xml: &str, local: &str) -> Result<Node> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        let (start, empty) = match event {
            Event::Start(start) => (start.into_owned(), false),
            Event::Empty(start) => (start.into_owned(), true),
            Event::Eof => return Err(xml_error(format!("no w:{local}"))),
            _ => continue,
        };
        if word && start.local_name().as_ref() == local.as_bytes() {
            return element(&mut reader, xml, &start, true, before..after, empty);
        }
    }
}

fn word_children<'a>(node: &'a Node, local: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
    node.children.iter().filter_map(move |child| match child {
        Child::Element(element) if element.word && element.local == local => Some(element),
        _ => None,
    })
}

/// Properties `props` read as the old ones of an unchanged owner: Word's
/// shape for a row's change, whose table, grid and cells take changes that
/// hold what they hold now (Word check W8).
fn as_old(props: &Props) -> Props {
    Props {
        prior: props.prior.clone(),
        base: props.base.clone(),
        ..Props::default()
    }
}

/// The property elements of the first table of `xml` resolved as `how`:
/// its properties, grid, row and cell properties.
fn table_properties(xml: &str, how: Resolution) -> Result<Vec<String>> {
    let wrapped = format!("<w:document xmlns:w=\"{W_NS}\"><w:body>{xml}</w:body></w:document>");
    let (output, _) = resolve_story_xml(wrapped.as_bytes(), how, RevisionScope::All)?;
    let output = String::from_utf8(output).map_err(xml_error)?;
    let table = first_word_element(&output, "tbl")?;
    // An owner without properties reads as none: a rejected row change whose
    // row had none leaves an empty `w:trPr`.
    let text = |node: Option<&Node>| {
        node.map_or(String::new(), |node| {
            let text = without_declarations(&output[node.span()]);
            match text.matches('<').count() == 1
                || text.contains("></") && text.matches('<').count() == 2
            {
                true => String::new(),
                false => text,
            }
        })
    };
    let mut found = vec![
        text(word_children(&table, "tblPr").next()),
        text(word_children(&table, "tblGrid").next()),
    ];
    for row in word_children(&table, "tr") {
        found.push(text(word_children(row, "trPr").next()));
        for cell in word_children(row, "tc") {
            found.push(text(word_children(cell, "tcPr").next()));
        }
    }
    Ok(found)
}

/// `xml` without its namespace declarations, which a resolution places
/// where its output needs them.
pub(super) fn without_declarations(xml: &str) -> String {
    let mut output = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(at) = rest.find(" xmlns") {
        output.push_str(&rest[..at]);
        let value = rest[at..]
            .find('"')
            .map_or(rest.len(), |open| at + open + 1);
        let end = rest[value..]
            .find('"')
            .map_or(rest.len(), |close| value + close + 1);
        rest = &rest[end..];
    }
    output.push_str(rest);
    output
}

/// Each paragraph of `xml`, blocks a splice inserted, as the accepted view
/// reads it: the check that marking them changed nothing they show.
/// Check that `marked`, blocks written as removed, rejected gives `original`
/// back and accepted holds none of their content.
pub(super) fn check_removed(marked: &str, original: &str) -> Result<()> {
    let accepted = resolved(marked, Resolution::Accept)?;
    match accepted.1.is_empty()
        && resolved(marked, Resolution::Reject)? == resolved(original, Resolution::Reject)?
    {
        true => Ok(()),
        false => Err(Error::Other(
            "this removal cannot be written as a tracked change".to_owned(),
        )),
    }
}

/// Check that `marked`, rejected, gives `original` back.
pub(super) fn check_rejected(marked: &str, original: &str) -> Result<()> {
    match resolved(marked, Resolution::Reject)? == resolved(original, Resolution::Reject)? {
        true => Ok(()),
        false => Err(Error::Other(
            "this removal cannot be written as a tracked change".to_owned(),
        )),
    }
}

pub(super) fn check_inserted(marked: &str, unmarked: &str) -> Result<()> {
    match resolved(marked, Resolution::Accept)? == resolved(unmarked, Resolution::Accept)? {
        true => Ok(()),
        false => Err(Error::Other(
            "this insertion cannot be written as a tracked change".to_owned(),
        )),
    }
}

/// Give each [`ID_MARK`] of `xml` a new revision id of the session, in
/// order, and hold them once `check` passes.
pub(super) fn give_ids(
    xml: &str,
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
    check: impl FnOnce(&str) -> Result<()>,
) -> Result<String> {
    // The old ids of the revisions a copy brought, each given a new one too.
    let mut foreign: Vec<&str> = Vec::new();
    for piece in xml.split(FOREIGN_ID.0).skip(1) {
        let old = piece.split(FOREIGN_ID.1).next().unwrap_or_default();
        if !foreign.contains(&old) {
            foreign.push(old);
        }
    }
    let count = xml.matches(ID_MARK).count();
    let all = identifiers.free_revision_ids(count + foreign.len())?;
    let (ids, renumbered) = all.split_at(count);
    let mut xml = xml.to_owned();
    for (old, new) in foreign.iter().zip(renumbered) {
        xml = xml.replace(
            &format!("{}{old}{}", FOREIGN_ID.0, FOREIGN_ID.1),
            &new.to_string(),
        );
    }
    let mut output = String::with_capacity(xml.len() + ids.len() * 4);
    let mut rest = xml.as_str();
    for id in ids {
        let at = rest.find(ID_MARK).expect("one mark per id");
        output.push_str(&rest[..at]);
        output.push_str(&id.to_string());
        rest = &rest[at + ID_MARK.len()..];
    }
    output.push_str(rest);
    check(&output)?;
    identifiers.hold_revision_ids(&all);
    tracking.record(ids);
    Ok(output)
}

/// `after`, a table an edit of its properties made from `before`, with the
/// changes written as tracked changes, as Word writes them: a `w:tblPrChange`,
/// a `w:tblGridChange` (an id alone) and a `w:tcPrChange` of each owner
/// that changed. A row's changed properties take a `w:trPrChange`, which a
/// rejection undoes as Word's does, beside what Word writes for them: its
/// table, its grid and each cell of the row take a change holding what they
/// hold now (Word check W8). Content, rows or cells changed are refused.
pub(super) fn redline_table_xml(
    before: &str,
    after: &str,
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
) -> Result<String> {
    let (old, new) = (
        first_word_element(before, "tbl")?,
        first_word_element(after, "tbl")?,
    );
    let prefix = new
        .name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    let read = |xml: &str, node: Option<&Node>, change: &str, kept: &[&str]| {
        node.map_or(Ok(Props::default()), |node| props(xml, node, change, kept))
    };
    let (old_rows, new_rows) = (
        word_children(&old, "tr").collect::<Vec<_>>(),
        word_children(&new, "tr").collect::<Vec<_>>(),
    );
    if old_rows.len() != new_rows.len() {
        return Err(refused("a change of a table's rows"));
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut rows_changed = false;
    let mut cells = Vec::new();
    for (old_row, new_row) in old_rows.iter().zip(&new_rows) {
        let trpr = |xml: &str, row: &Node| {
            read(
                xml,
                word_children(row, "trPr").next(),
                "trPrChange",
                &["ins", "del"],
            )
        };
        let (old_trpr, new_trpr) = (trpr(before, old_row)?, trpr(after, new_row)?);
        let row_changed = old_trpr.base != new_trpr.base;
        rows_changed |= row_changed;
        // The row's own change, which a rejection undoes (Word's Reject All
        // brings a row's height back, W8), beside the changes Word writes.
        if row_changed {
            let written = changed_props(
                &new_trpr,
                &old_trpr,
                &prefix,
                "trPr",
                "trPrChange",
                tracking,
            );
            edits.push(match word_children(new_row, "trPr").next() {
                Some(properties) => (properties.span(), written),
                None => {
                    let at = word_children(new_row, "tblPrEx")
                        .next()
                        .map_or(new_row.start.end, |exceptions| exceptions.span().end);
                    (at..at, written)
                }
            });
        }
        let (old_cells, new_cells) = (
            word_children(old_row, "tc").collect::<Vec<_>>(),
            word_children(new_row, "tc").collect::<Vec<_>>(),
        );
        if old_cells.len() != new_cells.len() {
            return Err(refused("a change of a row's cells"));
        }
        for (old_cell, new_cell) in old_cells.into_iter().zip(new_cells) {
            let content = |xml: &str, cell: &Node| {
                cell.children
                    .iter()
                    .filter_map(|child| match child {
                        Child::Element(element) if !(element.word && element.local == "tcPr") => {
                            Some(&xml[element.span()])
                        }
                        _ => None,
                    })
                    .collect::<String>()
            };
            if content(before, old_cell) != content(after, new_cell) {
                return Err(refused("an edit of a cell's content with its properties"));
            }
            cells.push((old_cell, new_cell, row_changed));
        }
    }
    let kept = ["cellIns", "cellDel", "cellMerge"];
    for (old_cell, new_cell, row_changed) in cells {
        let (old_tcpr, new_tcpr) = (
            word_children(old_cell, "tcPr").next(),
            word_children(new_cell, "tcPr").next(),
        );
        let (old_props, new_props) = (
            read(before, old_tcpr, "tcPrChange", &kept)?,
            read(after, new_tcpr, "tcPrChange", &kept)?,
        );
        let old_props = match (old_props.base != new_props.base, row_changed) {
            (true, _) => old_props,
            (false, true) => as_old(&new_props),
            (false, false) => continue,
        };
        let written = changed_props(
            &new_props,
            &old_props,
            &prefix,
            "tcPr",
            "tcPrChange",
            tracking,
        );
        edits.push(match new_tcpr {
            Some(tcpr) => (tcpr.span(), written),
            None => (new_cell.start.end..new_cell.start.end, written),
        });
    }
    let (old_tblpr, new_tblpr) = (
        word_children(&old, "tblPr").next(),
        word_children(&new, "tblPr").next(),
    );
    let (old_props, new_props) = (
        read(before, old_tblpr, "tblPrChange", &[])?,
        read(after, new_tblpr, "tblPrChange", &[])?,
    );
    if old_props.base != new_props.base || rows_changed {
        let old_props = match old_props.base != new_props.base {
            true => old_props,
            false => as_old(&new_props),
        };
        let written = changed_props(
            &new_props,
            &old_props,
            &prefix,
            "tblPr",
            "tblPrChange",
            tracking,
        );
        edits.push(match new_tblpr {
            Some(tblpr) => (tblpr.span(), written),
            None => (new.start.end..new.start.end, written),
        });
    }
    let (old_grid, new_grid) = (
        word_children(&old, "tblGrid").next(),
        word_children(&new, "tblGrid").next(),
    );
    let (old_props, new_props) = (
        read(before, old_grid, "tblGridChange", &[])?,
        read(after, new_grid, "tblGridChange", &[])?,
    );
    if let Some(grid) = new_grid
        && (old_props.base != new_props.base || rows_changed)
    {
        let old_props = match old_props.base != new_props.base {
            true => old_props,
            false => as_old(&new_props),
        };
        let prior = match old_props.prior {
            Some(prior) if !prior.is_empty() => prior,
            _ => format!("<{prefix}tblGrid>{}</{prefix}tblGrid>", old_props.base),
        };
        edits.push((
            grid.span(),
            format!(
                "{}{}<{prefix}tblGridChange {prefix}id=\"{ID_MARK}\">{prior}</{prefix}tblGridChange>{}",
                new_props.open, new_props.base, new_props.end
            ),
        ));
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut written = String::with_capacity(after.len() + edits.len() * 128);
    let mut copied = 0;
    for (range, text) in &edits {
        written.push_str(&after[copied..range.start]);
        written.push_str(text);
        copied = range.end;
    }
    written.push_str(&after[copied..]);
    give_ids(&written, tracking, identifiers, |redlined| {
        let accepted = table_properties(redlined, Resolution::Accept)?
            == table_properties(after, Resolution::Accept)?;
        let rejected = table_properties(redlined, Resolution::Reject)?
            == table_properties(before, Resolution::Reject)?;
        match accepted && rejected {
            true => Ok(()),
            false => Err(Error::Other(
                "this table edit cannot be written as a tracked change".to_owned(),
            )),
        }
    })
}

/// `after`, section properties an edit made from `before`, with the change
/// written as one `w:sectPrChange`, last, holding the old properties without
/// the header and footer references, as for the other property changes.
pub(super) fn redline_section_xml(
    before: &str,
    after: &str,
    tracking: &Tracking,
    identifiers: &mut DocumentIdentifiers,
) -> Result<String> {
    let (old, new) = (
        first_word_element(before, "sectPr")?,
        first_word_element(after, "sectPr")?,
    );
    let prefix = new
        .name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    let references = ["headerReference", "footerReference"];
    let old_props = props(before, &old, "sectPrChange", &references)?;
    let new_props = props(after, &new, "sectPrChange", &references)?;
    if old_props.base == new_props.base {
        return Ok(after.to_owned());
    }
    let prior = match &old_props.prior {
        Some(prior) if !prior.is_empty() => prior.clone(),
        _ => format!("<{prefix}sectPr>{}</{prefix}sectPr>", old_props.base),
    };
    let author = quick_xml::escape::escape(tracking.changes.author());
    let date = tracking.changes.timestamp().map_or(String::new(), |date| {
        format!(" {prefix}date=\"{}\"", quick_xml::escape::escape(date))
    });
    // The references first, as the schema orders them.
    let written = format!(
        "{}{}{}{}<{prefix}sectPrChange {prefix}id=\"{ID_MARK}\" {prefix}author=\"{author}\"{date}>{prior}</{prefix}sectPrChange>{}",
        &after[..new.start.start],
        new_props.open,
        new_props.kept,
        new_props.base,
        new_props.end
    ) + &after[new.span().end..];
    give_ids(&written, tracking, identifiers, |redlined| {
        let resolved = |xml: &str, how| -> Result<String> {
            let wrapped =
                format!("<w:document xmlns:w=\"{W_NS}\"><w:body>{xml}</w:body></w:document>");
            let (output, _) = resolve_story_xml(wrapped.as_bytes(), how, RevisionScope::All)?;
            let output = String::from_utf8(output).map_err(xml_error)?;
            let section = first_word_element(&output, "sectPr")?;
            Ok(without_declarations(&output[section.span()]))
        };
        match resolved(redlined, Resolution::Accept)? == resolved(after, Resolution::Accept)?
            && resolved(redlined, Resolution::Reject)? == resolved(before, Resolution::Reject)?
        {
            true => Ok(()),
            false => Err(Error::Other(
                "this section edit cannot be written as a tracked change".to_owned(),
            )),
        }
    })
}

/// `xml`, a paragraph or a table a splice inserts, written as inserted: each
/// paragraph's runs in a `w:ins` and its mark with a `w:ins`, each row's
/// properties with one. With `keep_last_mark`, the last paragraph's mark
/// stays unmarked: a story's final mark, which is never marked (Word check
/// W12). The ids are [`ID_MARK`] until the caller gives them.
pub(super) fn inserted_block_xml(
    xml: &str,
    tracking: &Tracking,
    keep_last_mark: bool,
    content: bool,
    previous: Option<&str>,
) -> Result<String> {
    marked_block_xml(xml, tracking, "ins", keep_last_mark, content, previous)
}

/// [`inserted_block_xml`] for a block removed (`w:del`, its texts as
/// `w:delText`). A kept last mark's paragraph takes the properties of
/// `previous`, whose mark is the deleted one, with a change holding its own,
/// so that a rejection gives them back. A block that holds a revision is
/// refused.
pub(super) fn deleted_block_xml(
    xml: &str,
    tracking: &Tracking,
    keep_last_mark: bool,
    content: bool,
    previous: Option<&str>,
) -> Result<String> {
    marked_block_xml(xml, tracking, "del", keep_last_mark, content, previous)
}

fn marked_block_xml(
    xml: &str,
    tracking: &Tracking,
    marker: &str,
    keep_last_mark: bool,
    content: bool,
    previous: Option<&str>,
) -> Result<String> {
    let deleted = marker == "del";
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(false);
    let root = loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event().map_err(xml_error)?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(start) => {
                break element(
                    &mut reader,
                    xml,
                    &start.into_owned(),
                    word,
                    before..after,
                    false,
                )?;
            }
            Event::Empty(start) => {
                break element(
                    &mut reader,
                    xml,
                    &start.into_owned(),
                    word,
                    before..after,
                    true,
                )?;
            }
            Event::Eof => return Err(xml_error("no block")),
            _ => {}
        }
    };
    let prefix = root
        .name
        .split_once(':')
        .map_or(String::new(), |(prefix, _)| format!("{prefix}:"));
    let author = quick_xml::escape::escape(tracking.changes.author());
    let date = tracking.changes.timestamp().map_or(String::new(), |date| {
        format!(" {prefix}date=\"{}\"", quick_xml::escape::escape(date))
    });
    let mark =
        format!("<{prefix}{marker} {prefix}id=\"{ID_MARK}\" {prefix}author=\"{author}\"{date}/>");
    let open =
        format!("<{prefix}{marker} {prefix}id=\"{ID_MARK}\" {prefix}author=\"{author}\"{date}>");
    let close = format!("</{prefix}{marker}>");
    if deleted && holds_revision(&root) {
        return Err(refused("the removal of a block that holds a revision"));
    }
    let mut paragraphs = Vec::new();
    match root.word && root.local == "p" {
        true => paragraphs.push(&root),
        false => collect_word(&root, "p", &mut paragraphs),
    }
    let mut rows = Vec::new();
    collect_word(&root, "tr", &mut rows);
    if content
        && holds_any(
            &root,
            &["del", "moveFrom", "moveTo", "cellDel", "moveFromRangeStart"],
        )
    {
        return Err(refused(
            "the insertion of content that holds a deletion or a move",
        ));
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    // The revisions a copy brings take ids of their own (`FOREIGN_ID`).
    let mut renumbered = Vec::new();
    if content {
        foreign_ids(xml, &root, &mut renumbered);
    }
    let last = paragraphs.len().saturating_sub(1);
    for (index, paragraph) in paragraphs.iter().enumerate() {
        // The mark first: a new `w:pPr` goes before the first run's `w:ins`
        // at the same place (the sort below keeps their order).
        let runs = |edits: &mut Vec<(Range<usize>, String)>| match content {
            true => {
                if deleted {
                    rename_deleted_texts(xml, paragraph, edits)?;
                }
                wrap_runs(paragraph, &open, &close, edits)
            }
            false => Ok(()),
        };
        if keep_last_mark && index == last {
            // The story's final mark holds no revision (Word check W12): one
            // a copy brings is cleared. Its properties first: a new `w:pPr`
            // goes before the first run's `w:ins` at the same place.
            let properties = word_children(paragraph, "pPr").next();
            let mark_revisions = properties
                .and_then(|properties| word_children(properties, "rPr").next())
                .into_iter()
                .flat_map(|run| run.children.iter())
                .filter_map(|child| match child {
                    Child::Element(element)
                        if element.word
                            && matches!(
                                element.local.as_str(),
                                "ins" | "del" | "moveFrom" | "moveTo"
                            ) =>
                    {
                        Some(element.span())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            // The mark's properties left with nothing but those revisions go
            // with them, and the paragraph's properties so too.
            let elements = |node: &Node| {
                node.children
                    .iter()
                    .filter(|child| matches!(child, Child::Element(_)))
                    .count()
            };
            let mark_properties =
                properties.and_then(|properties| word_children(properties, "rPr").next());
            let bare_mark = !mark_revisions.is_empty()
                && mark_properties.is_some_and(|run| elements(run) == mark_revisions.len());
            let mut changed = false;
            // The kept mark is the one of `previous`, whose properties it
            // takes: a change of them gives them back when rejected, as Word
            // writes properties given to a new last paragraph.
            if let Some(previous) = previous {
                let before = first_word_element(previous, "p")?;
                let read = |xml: &str, node: &Node| {
                    word_children(node, "pPr")
                        .next()
                        .map_or(Ok(Props::default()), |properties| {
                            props(xml, properties, "pPrChange", &["rPr", "sectPr"])
                        })
                };
                let (old, mut new) = match deleted {
                    // The paragraph takes the properties of `previous`, and
                    // keeps its mark's properties and its section.
                    true => {
                        let own = read(xml, paragraph)?;
                        let taken = Props {
                            kept: own.kept.clone(),
                            open: own.open.clone(),
                            end: own.end.clone(),
                            ..read(previous, &before)?
                        };
                        (own, taken)
                    }
                    false => (read(previous, &before)?, read(xml, paragraph)?),
                };
                if old.base != new.base || old.prior.is_some() {
                    changed = true;
                    let kept = |local: &str| {
                        properties
                            .and_then(|properties| word_children(properties, local).next())
                            .filter(|_| !(local == "rPr" && bare_mark))
                            .map(Node::span)
                    };
                    new.kept = [kept("rPr"), kept("sectPr")]
                        .into_iter()
                        .flatten()
                        .map(|span| {
                            without_spans(xml, span, &mark_revisions, &renumbered)
                                .unwrap_or_default()
                        })
                        .collect();
                    let written = changed_props(&new, &old, &prefix, "pPr", "pPrChange", tracking);
                    edits.push(match word_children(paragraph, "pPr").next() {
                        Some(properties) => (properties.span(), written),
                        None if paragraph.end.is_none() => {
                            (paragraph.start.clone(), opened(xml, paragraph, &written)?)
                        }
                        None => (paragraph.start.end..paragraph.start.end, written),
                    });
                }
            }
            if !changed {
                match (properties, mark_properties) {
                    (Some(properties), Some(_)) if bare_mark && elements(properties) == 1 => {
                        edits.push((properties.span(), String::new()));
                    }
                    (_, Some(run)) if bare_mark => edits.push((run.span(), String::new())),
                    _ => edits.extend(mark_revisions.into_iter().map(|span| (span, String::new()))),
                }
            }
            runs(&mut edits)?;
            continue;
        }
        let properties = word_children(paragraph, "pPr").next();
        let marked = format!("<{prefix}pPr><{prefix}rPr>{mark}</{prefix}rPr></{prefix}pPr>");
        match properties {
            // An empty paragraph, `<w:p/>`, opened to hold its mark.
            None if paragraph.end.is_none() => {
                edits.push((paragraph.start.clone(), opened(xml, paragraph, &marked)?));
            }
            None => edits.push((paragraph.start.end..paragraph.start.end, marked)),
            Some(properties) => match word_children(properties, "rPr").next() {
                // A mark a copy brings already inserted keeps its own.
                Some(run) if word_children(run, "ins").next().is_some() => {}
                // First in the mark's properties, as the schema orders it.
                Some(run) => edits.push((run.start.end..run.start.end, mark.clone())),
                None => {
                    let at = word_children(properties, "sectPr")
                        .chain(word_children(properties, "pPrChange"))
                        .map(|later| later.start.start)
                        .min()
                        .unwrap_or_else(|| properties.content().end);
                    edits.push((at..at, format!("<{prefix}rPr>{mark}</{prefix}rPr>")));
                }
            },
        }
        runs(&mut edits)?;
    }
    for row in rows.into_iter().filter(|_| content) {
        match word_children(row, "trPr").next() {
            // After the row's own properties, before a change of them.
            Some(properties) => {
                let at = word_children(properties, "trPrChange")
                    .map(|change| change.start.start)
                    .next()
                    .unwrap_or_else(|| properties.content().end);
                edits.push((at..at, mark.clone()));
            }
            None => {
                let at = word_children(row, "tblPrEx")
                    .next()
                    .map_or(row.start.end, |exceptions| exceptions.span().end);
                edits.push((at..at, format!("<{prefix}trPr>{mark}</{prefix}trPr>")));
            }
        }
    }
    // An id inside a span another edit rewrites goes with that span.
    renumbered.retain(|(at, _): &(Range<usize>, String)| {
        !edits
            .iter()
            .any(|(range, _)| range.start <= at.start && at.end <= range.end && !range.is_empty())
    });
    edits.extend(renumbered);
    edits.sort_by_key(|(range, _)| range.start);
    let mut output = String::with_capacity(xml.len() + edits.len() * 96);
    let mut copied = 0;
    for (range, text) in edits {
        output.push_str(&xml[copied..range.start]);
        output.push_str(&text);
        copied = range.end;
    }
    output.push_str(&xml[copied..]);
    Ok(output)
}

/// Where a revision a copied block brings has its id, until the ids are
/// given: the old id between the two marks, so that ids shared by a range's
/// start and end stay shared.
const FOREIGN_ID: (&str, &str) = ("\u{2}", "\u{3}");

/// The id of each revision under `node`, to be given anew.
fn foreign_ids(xml: &str, node: &Node, found: &mut Vec<(Range<usize>, String)>) {
    for child in &node.children {
        let Child::Element(element) = child else {
            continue;
        };
        if element.word
            && matches!(
                element.local.as_str(),
                "ins"
                    | "rPrChange"
                    | "pPrChange"
                    | "sectPrChange"
                    | "tblPrChange"
                    | "tblPrExChange"
                    | "trPrChange"
                    | "tcPrChange"
                    | "tblGridChange"
                    | "numberingChange"
                    | "cellIns"
                    | "cellMerge"
            )
        {
            let open = &xml[element.start.clone()];
            let prefix = &element.name[..element.name.len() - element.local.len()];
            let attribute = format!(" {prefix}id=\"");
            if let Some(at) = open.find(&attribute) {
                let value = element.start.start + at + attribute.len();
                if let Some(length) = xml[value..].find('"') {
                    let old = &xml[value..value + length];
                    found.push((
                        value..value + length,
                        format!("{}{old}{}", FOREIGN_ID.0, FOREIGN_ID.1),
                    ));
                }
            }
        }
        foreign_ids(xml, element, found);
    }
}

/// Whether `node` holds a Word element named one of `locals`.
fn holds_any(node: &Node, locals: &[&str]) -> bool {
    node.children.iter().any(|child| match child {
        Child::Element(element) => {
            (element.word && locals.contains(&element.local.as_str())) || holds_any(element, locals)
        }
        Child::Text(_) => false,
    })
}

/// `xml[span]` without the `removed` spans inside it, and with the
/// `replaced` ones inside it rewritten (the ids a copy brings).
fn without_spans(
    xml: &str,
    span: Range<usize>,
    removed: &[Range<usize>],
    replaced: &[(Range<usize>, String)],
) -> Option<String> {
    let mut cuts = removed
        .iter()
        .map(|cut| (cut.clone(), ""))
        .chain(
            replaced
                .iter()
                .map(|(cut, text)| (cut.clone(), text.as_str())),
        )
        .filter(|(cut, _)| span.start <= cut.start && cut.end <= span.end)
        .collect::<Vec<_>>();
    cuts.sort_by_key(|(cut, _)| cut.start);
    let mut output = String::new();
    let mut at = span.start;
    for (cut, text) in cuts {
        // A rewrite inside a removed span goes with it.
        if cut.start < at {
            continue;
        }
        output.push_str(xml.get(at..cut.start)?);
        output.push_str(text);
        at = cut.end;
    }
    output.push_str(xml.get(at..span.end)?);
    Some(output)
}

fn collect_word<'a>(node: &'a Node, local: &str, found: &mut Vec<&'a Node>) {
    for child in &node.children {
        if let Child::Element(element) = child {
            if element.word && element.local == local {
                found.push(element);
            }
            if !(element.word && element.local == "p" && local == "p") {
                collect_word(element, local, found);
            }
        }
    }
}

/// Wrap each stretch of runs of `owner` (a paragraph, a link, a control's
/// content) in `open`..`close`. A field or another inline element that is no
/// run is refused: its insertion comes with its own shape.
fn wrap_runs(
    owner: &Node,
    open: &str,
    close: &str,
    edits: &mut Vec<(Range<usize>, String)>,
) -> Result<()> {
    // The change these runs take, for the refusal's words.
    let change = match open.contains("del ") {
        true => "removal",
        false => "insertion",
    };
    let mut stretch: Option<Range<usize>> = None;
    let end = |stretch: &mut Option<Range<usize>>, edits: &mut Vec<(Range<usize>, String)>| {
        if let Some(range) = stretch.take() {
            edits.push((range.start..range.start, open.to_owned()));
            edits.push((range.end..range.end, close.to_owned()));
        }
    };
    for child in &owner.children {
        let Child::Element(element) = child else {
            continue;
        };
        match (element.word, element.local.as_str()) {
            (true, "r") => {
                let span = element.span();
                stretch = Some(stretch.map_or(span.clone(), |range| range.start..span.end));
            }
            (true, "pPr") => {}
            (true, "hyperlink") => {
                end(&mut stretch, edits);
                wrap_runs(element, open, close, edits)?;
            }
            (true, "sdt") => {
                end(&mut stretch, edits);
                if let Some(content) = word_children(element, "sdtContent").next() {
                    wrap_runs(content, open, close, edits)?;
                }
            }
            (true, local)
                if RANGE_MARKERS.contains(&local)
                    || local.ends_with("RangeStart")
                    || local.ends_with("RangeEnd")
                    || local == "proofErr" =>
            {
                end(&mut stretch, edits);
            }
            (true, "ins") => end(&mut stretch, edits),
            _ => {
                return Err(refused(&format!(
                    "the {change} of a field, an equation or other inline content"
                )));
            }
        }
    }
    end(&mut stretch, edits);
    Ok(())
}

/// `node`, an element written self-closing (`<w:p/>`), written with an end
/// tag to hold `content`.
fn opened(xml: &str, node: &Node, content: &str) -> Result<String> {
    oxoffice_core::xml_element::replace_content(&xml[node.start.clone()], content)
        .map(|(opened, _)| opened)
        .map_err(xml_error)
}

/// Whether `node` holds a revision: a wrapper, a mark or a row marked, or a
/// property change.
fn holds_revision(node: &Node) -> bool {
    node.children.iter().any(|child| match child {
        Child::Element(element) => {
            (element.word
                && matches!(
                    element.local.as_str(),
                    "ins"
                        | "del"
                        | "moveFrom"
                        | "moveTo"
                        | "rPrChange"
                        | "pPrChange"
                        | "trPrChange"
                        | "tcPrChange"
                        | "tblPrChange"
                        | "cellIns"
                        | "cellDel"
                        | "cellMerge"
                ))
                || holds_revision(element)
        }
        Child::Text(_) => false,
    })
}

/// Rename the texts of the runs of `owner` (a paragraph, a link, a control's
/// content) as a deletion writes them: `w:t` as `w:delText`, `w:instrText`
/// as `w:delInstrText`.
fn rename_deleted_texts(
    xml: &str,
    owner: &Node,
    edits: &mut Vec<(Range<usize>, String)>,
) -> Result<()> {
    for child in &owner.children {
        let Child::Element(element) = child else {
            continue;
        };
        match (element.word, element.local.as_str()) {
            (true, "r") => {
                for text in word_children(element, "t").chain(word_children(element, "instrText")) {
                    let renamed = match text.local.as_str() {
                        "t" => "delText",
                        _ => "delInstrText",
                    };
                    let prefix = &text.name[..text.name.len() - text.local.len()];
                    let written = oxoffice_core::xml_element::rename(
                        &xml[text.span()],
                        &format!("{prefix}{renamed}"),
                    )
                    .map_err(xml_error)?;
                    edits.push((text.span(), written));
                }
            }
            (true, "hyperlink") => rename_deleted_texts(xml, element, edits)?,
            (true, "sdt") => {
                if let Some(content) = word_children(element, "sdtContent").next() {
                    rename_deleted_texts(xml, content, edits)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
