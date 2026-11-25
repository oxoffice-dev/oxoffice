//! The rewrites: each writes one part another way and copies every other
//! byte as it was, so that a failure points at one writing.

use std::collections::{BTreeMap, BTreeSet};

use crate::tokens::{
    Attribute, Kind, Scopes, Tag, Token, declared_prefix, end_name, parse_tag, split_bom,
    split_name, tokens,
};

/// The namespace of markup compatibility, whose attributes list prefixes.
pub const MC_NAMESPACE: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// The main namespaces of WordprocessingML, PresentationML and DrawingML,
/// transitional and strict: the ones [`Writing::OtherPrefixes`] binds to
/// other prefixes.
pub const MAIN_NAMESPACES: [&str; 18] = [
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://schemas.openxmlformats.org/presentationml/2006/main",
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://schemas.openxmlformats.org/drawingml/2006/picture",
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://schemas.openxmlformats.org/drawingml/2006/diagram",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
    "http://purl.oclc.org/ooxml/presentationml/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/math",
    "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
    "http://purl.oclc.org/ooxml/drawingml/picture",
    "http://purl.oclc.org/ooxml/drawingml/chart",
    "http://purl.oclc.org/ooxml/drawingml/diagram",
];

/// One way of writing the same XML.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Writing {
    /// Every empty-element tag written as a start and an end tag:
    /// `<w:fldChar ...></w:fldChar>`, as LibreOffice writes some.
    EmptyWithEndTag,
    /// Every element with nothing between its start and end tags written as
    /// one empty-element tag: `<w:sdtContent/>`, as Google Docs writes it.
    EmptySelfClosed,
    /// The main namespaces bound to other prefixes (`ox0:p` for `w:p`),
    /// declared where the original prefixes are.
    OtherPrefixes,
    /// The namespace of the root as the default namespace: its elements
    /// without a prefix, its attributes keeping theirs.
    DefaultNamespace,
    /// A line break and an indent between tags, as a pretty printer writes
    /// them: never in an element without child elements, with text, or under
    /// `xml:space="preserve"`.
    Indented,
    /// The attributes of every tag, namespace declarations included, in
    /// reverse order.
    AttributeOrder,
    /// Every attribute value between single quotes.
    SingleQuotes,
    /// Line breaks and spaces inside tags: around `=`, between attributes,
    /// before `>` and `/>`, and before the `>` of end tags.
    TagSpacing,
    /// Every character outside ASCII as a numeric character reference, and
    /// `>`, `"` and `'` as entity references, in text and attribute values.
    CharacterReferences,
    /// The first character of every text and of every attribute value that
    /// declares no namespace as a numeric character reference, an ASCII
    /// letter or digit included.
    LeadingCharacterReference,
}

impl Writing {
    pub const ALL: [Writing; 10] = [
        Writing::EmptyWithEndTag,
        Writing::EmptySelfClosed,
        Writing::OtherPrefixes,
        Writing::DefaultNamespace,
        Writing::Indented,
        Writing::AttributeOrder,
        Writing::SingleQuotes,
        Writing::TagSpacing,
        Writing::CharacterReferences,
        Writing::LeadingCharacterReference,
    ];

    /// The name tests and known failures use.
    pub fn name(self) -> &'static str {
        match self {
            Writing::EmptyWithEndTag => "empty-with-end-tag",
            Writing::EmptySelfClosed => "empty-self-closed",
            Writing::OtherPrefixes => "other-prefixes",
            Writing::DefaultNamespace => "default-namespace",
            Writing::Indented => "indented",
            Writing::AttributeOrder => "attribute-order",
            Writing::SingleQuotes => "single-quotes",
            Writing::TagSpacing => "tag-spacing",
            Writing::CharacterReferences => "character-references",
            Writing::LeadingCharacterReference => "leading-character-reference",
        }
    }

    pub fn from_name(name: &str) -> Option<Writing> {
        Writing::ALL
            .into_iter()
            .find(|writing| writing.name() == name)
    }
}

/// A part written another way, and how many places the rewrite changed.
pub struct Rewritten {
    pub xml: Vec<u8>,
    pub changes: usize,
}

/// `xml` written the way `writing` says. Every byte the rewrite does not
/// concern is copied.
pub fn rewrite(xml: &[u8], writing: Writing) -> Result<Rewritten, String> {
    let (bom, body) = split_bom(xml);
    let text = std::str::from_utf8(body).map_err(|error| format!("not UTF-8: {error}"))?;
    let tokens = tokens(body)?;
    let mut out = String::with_capacity(body.len() + body.len() / 4);
    let changes = match writing {
        Writing::EmptyWithEndTag => empty_with_end_tag(text, &tokens, &mut out)?,
        Writing::EmptySelfClosed => empty_self_closed(text, &tokens, &mut out),
        Writing::OtherPrefixes => other_prefixes(text, &tokens, &mut out)?,
        Writing::DefaultNamespace => default_namespace(text, &tokens, &mut out)?,
        Writing::Indented => indented(text, &tokens, &mut out)?,
        Writing::AttributeOrder => each_tag(text, &tokens, &mut out, |tag| {
            (tag.attributes.len() > 1).then(|| {
                let mut reversed = tag.clone();
                reversed.attributes.reverse();
                write_tag(&reversed, &TagStyle::AS_WRITTEN)
            })
        })?,
        Writing::SingleQuotes => each_tag(text, &tokens, &mut out, |tag| {
            tag.attributes
                .iter()
                .any(|attribute| attribute.quote == '"')
                .then(|| {
                    write_tag(
                        tag,
                        &TagStyle {
                            quote: Some('\''),
                            ..TagStyle::AS_WRITTEN
                        },
                    )
                })
        })?,
        Writing::TagSpacing => tag_spacing(text, &tokens, &mut out)?,
        Writing::CharacterReferences => {
            character_references(text, &tokens, &mut out, ReferenceRule::NonAscii)?
        }
        Writing::LeadingCharacterReference => {
            character_references(text, &tokens, &mut out, ReferenceRule::Leading)?
        }
    };
    let mut xml = bom.to_vec();
    xml.extend_from_slice(out.as_bytes());
    Ok(Rewritten { xml, changes })
}

/// How a tag is written back.
struct TagStyle<'a> {
    /// The quote of every value; `None` keeps each attribute's own.
    quote: Option<char>,
    /// Before each attribute.
    separator: &'a str,
    /// Between a name and its value.
    equals: &'a str,
    /// Before `>` or `/>`.
    before_close: &'a str,
}

impl TagStyle<'_> {
    const AS_WRITTEN: TagStyle<'static> = TagStyle {
        quote: None,
        separator: " ",
        equals: "=",
        before_close: "",
    };
}

fn write_tag(tag: &Tag<'_>, style: &TagStyle<'_>) -> String {
    let mut out = format!("<{}", tag.name);
    for attribute in &tag.attributes {
        out.push_str(style.separator);
        out.push_str(attribute.name);
        out.push_str(style.equals);
        push_value(&mut out, attribute.value, attribute.quote, style.quote);
    }
    out.push_str(style.before_close);
    out.push_str(if tag.empty { "/>" } else { ">" });
    out
}

/// A raw value between `quote`, or between its own quote when `quote` is
/// `None`; the new quote, if the value holds it, becomes a reference.
fn push_value(out: &mut String, value: &str, own: char, quote: Option<char>) {
    let quote = quote.unwrap_or(own);
    out.push(quote);
    for c in value.chars() {
        match c {
            '"' if quote == '"' => out.push_str("&quot;"),
            '\'' if quote == '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out.push(quote);
}

/// Each start and empty-element tag written as `write` says, or as it was
/// when `write` gives `None`; everything else copied.
fn each_tag(
    text: &str,
    tokens: &[Token],
    out: &mut String,
    write: impl Fn(&Tag<'_>) -> Option<String>,
) -> Result<usize, String> {
    let mut changes = 0;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        if matches!(token.kind, Kind::Start | Kind::Empty)
            && let Some(written) = write(&parse_tag(raw)?)
        {
            out.push_str(&written);
            changes += 1;
        } else {
            out.push_str(raw);
        }
    }
    Ok(changes)
}

fn empty_with_end_tag(text: &str, tokens: &[Token], out: &mut String) -> Result<usize, String> {
    let mut changes = 0;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        if token.kind == Kind::Empty {
            let tag = parse_tag(raw)?;
            out.push_str(raw.strip_suffix("/>").expect("an empty-element tag"));
            out.push_str(&format!("></{}>", tag.name));
            changes += 1;
        } else {
            out.push_str(raw);
        }
    }
    Ok(changes)
}

fn empty_self_closed(text: &str, tokens: &[Token], out: &mut String) -> usize {
    let mut changes = 0;
    let mut index = 0;
    while index < tokens.len() {
        let raw = &text[tokens[index].raw.clone()];
        if tokens[index].kind == Kind::Start
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == Kind::End)
        {
            out.push_str(raw.strip_suffix('>').expect("a start tag"));
            out.push_str("/>");
            changes += 1;
            index += 2;
        } else {
            out.push_str(raw);
            index += 1;
        }
    }
    changes
}

/// Whether an attribute of `element` lists prefixes, and how: `Some(false)`
/// for a list of prefixes (`mc:Ignorable`), `Some(true)` for one of
/// qualified names (`mc:ProcessContent`).
fn prefix_list(scopes: &Scopes, element: &str, attribute: &str) -> Result<Option<bool>, String> {
    let (prefix, local) = split_name(attribute);
    if prefix.is_empty() {
        // `Requires` of `mc:Choice` has no prefix.
        let (element_prefix, element_local) = split_name(element);
        let on_choice =
            element_local == "Choice" && scopes.resolve(element_prefix) == Some(MC_NAMESPACE);
        return Ok((on_choice && local == "Requires").then_some(false));
    }
    if prefix == "xmlns" || scopes.resolve(prefix) != Some(MC_NAMESPACE) {
        return Ok(None);
    }
    Ok(match local {
        "Ignorable" | "MustUnderstand" => Some(false),
        "ProcessContent" | "PreserveElements" | "PreserveAttributes" => Some(true),
        _ => None,
    })
}

/// A list of prefixes or of qualified names with each prefix renamed by
/// `rename`, the whitespace between them kept.
fn rename_listed(value: &str, qualified: bool, rename: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let (prefix, rest) = if qualified {
            match token.split_once(':') {
                Some((prefix, local)) => (prefix.to_owned(), format!(":{local}")),
                None => (String::new(), token.clone()),
            }
        } else {
            (token.clone(), String::new())
        };
        match (!prefix.is_empty()).then(|| rename(&prefix)).flatten() {
            Some(renamed) => out.push_str(&format!("{renamed}{rest}")),
            None => out.push_str(token),
        }
        token.clear();
    };
    for c in value.chars() {
        if c.is_ascii_whitespace() {
            flush(&mut token, &mut out);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

fn other_prefixes(text: &str, tokens: &[Token], out: &mut String) -> Result<usize, String> {
    // A new prefix for each (prefix, namespace) pair declared, `ox0`, `ox1`...
    // The part must not use such a prefix already.
    let mut used = BTreeSet::new();
    for token in tokens {
        if matches!(token.kind, Kind::Start | Kind::Empty) {
            for attribute in parse_tag(&text[token.raw.clone()])?.attributes {
                if let Some(prefix) = declared_prefix(attribute.name) {
                    used.insert(prefix.to_owned());
                }
            }
        }
    }
    if used.iter().any(|prefix| prefix.starts_with("ox")) {
        return Err(format!("the part already uses a prefix ox...: {used:?}"));
    }
    let mut renamed: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut scopes = Scopes::default();
    let mut names = Vec::new();
    let mut changes = 0;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        match token.kind {
            Kind::Start | Kind::Empty => {
                let tag = parse_tag(raw)?;
                for attribute in &tag.attributes {
                    if let Some(prefix) = declared_prefix(attribute.name)
                        && !prefix.is_empty()
                        && MAIN_NAMESPACES.contains(&attribute.value)
                    {
                        let next = format!("ox{}", renamed.len());
                        renamed
                            .entry((prefix.to_owned(), attribute.value.to_owned()))
                            .or_insert(next);
                    }
                }
                scopes.push(&tag.attributes)?;
                let rename = |prefix: &str| {
                    let uri = scopes.resolve(prefix)?;
                    renamed.get(&(prefix.to_owned(), uri.to_owned())).cloned()
                };
                let rename_name = |name: &str| -> String {
                    match split_name(name) {
                        ("", _) => name.to_owned(),
                        (prefix, local) => match rename(prefix) {
                            Some(renamed) => format!("{renamed}:{local}"),
                            None => name.to_owned(),
                        },
                    }
                };
                let mut written = tag.clone();
                let name = rename_name(tag.name);
                let mut owned = Vec::new();
                for attribute in &tag.attributes {
                    let new_name = match declared_prefix(attribute.name) {
                        Some("") => attribute.name.to_owned(),
                        Some(prefix) => {
                            match renamed.get(&(prefix.to_owned(), attribute.value.to_owned())) {
                                Some(renamed) => format!("xmlns:{renamed}"),
                                None => attribute.name.to_owned(),
                            }
                        }
                        None => rename_name(attribute.name),
                    };
                    let new_value = match prefix_list(&scopes, tag.name, attribute.name)? {
                        Some(qualified) => rename_listed(attribute.value, qualified, rename),
                        None => attribute.value.to_owned(),
                    };
                    owned.push((new_name, new_value, attribute.quote));
                }
                if token.kind == Kind::Empty {
                    scopes.pop();
                } else {
                    names.push(name.clone());
                }
                written.name = &name;
                written.attributes = owned
                    .iter()
                    .map(|(name, value, quote)| Attribute {
                        name,
                        value,
                        quote: *quote,
                    })
                    .collect();
                let written = write_tag(&written, &TagStyle::AS_WRITTEN);
                if written != write_tag(&tag, &TagStyle::AS_WRITTEN) {
                    changes += 1;
                    out.push_str(&written);
                } else {
                    out.push_str(raw);
                }
            }
            Kind::End => {
                let name = names.pop().ok_or("an end tag without a start")?;
                if name == end_name(raw) {
                    out.push_str(raw);
                } else {
                    out.push_str(&format!("</{name}>"));
                }
                scopes.pop();
            }
            _ => out.push_str(raw),
        }
    }
    Ok(changes)
}

fn default_namespace(text: &str, tokens: &[Token], out: &mut String) -> Result<usize, String> {
    let root = tokens
        .iter()
        .find(|token| matches!(token.kind, Kind::Start | Kind::Empty))
        .ok_or("no root element")?;
    let root_tag = parse_tag(&text[root.raw.clone()])?;
    if split_name(root_tag.name).0.is_empty() {
        // Already in the default namespace, or in none.
        out.push_str(text);
        return Ok(0);
    }
    let mut scopes = Scopes::default();
    scopes.push(&root_tag.attributes)?;
    let target = scopes.element_namespace(root_tag.name)?;
    let mut scopes = Scopes::default();
    // The default namespace of the output in scope, and the name each open
    // element is written with.
    let mut defaults: Vec<String> = vec![String::new()];
    let mut names: Vec<String> = Vec::new();
    let mut changes = 0;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        match token.kind {
            Kind::Start | Kind::Empty => {
                let tag = parse_tag(raw)?;
                scopes.push(&tag.attributes)?;
                let namespace = scopes.element_namespace(tag.name)?;
                let (prefix, local) = split_name(tag.name);
                let in_scope = defaults.last().expect("the document's default").clone();
                // The name written and the default namespace it needs, if any.
                let (name, needs) = if namespace == target {
                    (local.to_owned(), Some(target.clone()))
                } else if prefix.is_empty() {
                    (local.to_owned(), Some(namespace.clone()))
                } else {
                    (tag.name.to_owned(), None)
                };
                let mut attributes: Vec<Attribute<'_>> = tag
                    .attributes
                    .iter()
                    .filter(|attribute| attribute.name != "xmlns")
                    .cloned()
                    .collect();
                let default = match &needs {
                    Some(needed) if *needed != in_scope => {
                        attributes.insert(
                            0,
                            Attribute {
                                name: "xmlns",
                                value: needed,
                                quote: '"',
                            },
                        );
                        needed.clone()
                    }
                    _ => in_scope,
                };
                let written = write_tag(
                    &Tag {
                        name: &name,
                        attributes,
                        empty: tag.empty,
                    },
                    &TagStyle::AS_WRITTEN,
                );
                if written != write_tag(&tag, &TagStyle::AS_WRITTEN) {
                    changes += 1;
                    out.push_str(&written);
                } else {
                    out.push_str(raw);
                }
                if token.kind == Kind::Empty {
                    scopes.pop();
                } else {
                    defaults.push(default);
                    names.push(name);
                }
            }
            Kind::End => {
                let name = names.pop().ok_or("an end tag without a start")?;
                if name == end_name(raw) {
                    out.push_str(raw);
                } else {
                    out.push_str(&format!("</{name}>"));
                }
                defaults.pop();
                scopes.pop();
            }
            _ => out.push_str(raw),
        }
    }
    Ok(changes)
}

fn indented(text: &str, tokens: &[Token], out: &mut String) -> Result<usize, String> {
    // The elements with text, a reference or a CDATA section of their own
    // (mixed content), by the index of their start token.
    let mut mixed = BTreeSet::new();
    let mut open: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Start => open.push(index),
            Kind::End => {
                open.pop();
            }
            Kind::Text if text[token.raw.clone()].trim().is_empty() => {}
            Kind::Text | Kind::Reference | Kind::CData => {
                if let Some(parent) = open.last() {
                    mixed.insert(*parent);
                }
            }
            _ => {}
        }
    }
    // Each open element: its start token and whether it is under
    // `xml:space="preserve"`.
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut changes = 0;
    let mut previous: Option<Kind> = None;
    for (index, token) in tokens.iter().enumerate() {
        let raw = &text[token.raw.clone()];
        let tag_after_tag = matches!(token.kind, Kind::Start | Kind::Empty | Kind::End)
            && matches!(
                previous,
                Some(Kind::Start | Kind::Empty | Kind::End | Kind::Other)
            );
        // The element the whitespace would go in: the one this end tag
        // closes, or the one open around this start tag. Never between a
        // start tag and its own end tag.
        let around = stack.last().copied();
        let own_end = token.kind == Kind::End && previous == Some(Kind::Start);
        let allowed = match around {
            None => token.kind != Kind::End,
            Some((start, preserve)) => !preserve && !mixed.contains(&start) && !own_end,
        };
        // Nothing after the root's end tag.
        let after_root = around.is_none() && previous == Some(Kind::End);
        if tag_after_tag && allowed && !after_root {
            let depth = stack.len() - usize::from(token.kind == Kind::End);
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
            changes += 1;
        }
        out.push_str(raw);
        match token.kind {
            Kind::Start => {
                let tag = parse_tag(raw)?;
                let space = tag
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == "xml:space")
                    .map(|attribute| attribute.value == "preserve");
                let inherited = stack.last().is_some_and(|(_, preserve)| *preserve);
                stack.push((index, space.unwrap_or(inherited)));
            }
            Kind::End => {
                stack.pop();
            }
            _ => {}
        }
        previous = Some(token.kind);
    }
    Ok(changes)
}

fn tag_spacing(text: &str, tokens: &[Token], out: &mut String) -> Result<usize, String> {
    let style = TagStyle {
        quote: None,
        separator: "\n\t",
        equals: " = ",
        before_close: " ",
    };
    let mut changes = 0;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        match token.kind {
            Kind::Start | Kind::Empty => {
                out.push_str(&write_tag(&parse_tag(raw)?, &style));
                changes += 1;
            }
            Kind::End => {
                out.push_str(&format!("</{}\n>", end_name(raw)));
                changes += 1;
            }
            _ => out.push_str(raw),
        }
    }
    Ok(changes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceRule {
    /// Characters outside ASCII as references, `>`, `"` and `'` as entities.
    NonAscii,
    /// The first character as a reference.
    Leading,
}

/// `raw` (escaped text or an escaped attribute value) with the references
/// `rule` asks for. A character already written as a reference stays one.
fn with_references(raw: &str, rule: ReferenceRule) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.char_indices().peekable();
    let mut first = true;
    while let Some((at, c)) = chars.next() {
        if c == '&' {
            // An existing reference, copied whole.
            let end = raw[at..].find(';').map_or(raw.len(), |end| at + end + 1);
            out.push_str(&raw[at..end]);
            while chars.peek().is_some_and(|(next, _)| *next < end) {
                chars.next();
            }
            first = false;
            continue;
        }
        // Whitespace stays literal: a reference to it in an attribute value
        // is not normalised to a space as the literal character is.
        let whitespace = matches!(c, ' ' | '\t' | '\n' | '\r');
        match rule {
            ReferenceRule::Leading if first && !whitespace => {
                out.push_str(&format!("&#{};", u32::from(c)));
            }
            ReferenceRule::NonAscii if !c.is_ascii() => {
                out.push_str(&format!("&#x{:X};", u32::from(c)));
            }
            ReferenceRule::NonAscii if c == '>' => out.push_str("&gt;"),
            ReferenceRule::NonAscii if c == '"' => out.push_str("&quot;"),
            ReferenceRule::NonAscii if c == '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
        first &= whitespace;
    }
    out
}

fn character_references(
    text: &str,
    tokens: &[Token],
    out: &mut String,
    rule: ReferenceRule,
) -> Result<usize, String> {
    let mut changes = 0;
    let mut previous: Option<Kind> = None;
    for token in tokens {
        let raw = &text[token.raw.clone()];
        match token.kind {
            // A text right after a reference continues the same text.
            Kind::Text
                if !(rule == ReferenceRule::Leading && previous == Some(Kind::Reference)) =>
            {
                let written = with_references(raw, rule);
                changes += usize::from(written != raw);
                out.push_str(&written);
            }
            Kind::Start | Kind::Empty => {
                let tag = parse_tag(raw)?;
                let values: Vec<String> = tag
                    .attributes
                    .iter()
                    .map(|attribute| {
                        if declared_prefix(attribute.name).is_some()
                            || (rule == ReferenceRule::Leading && attribute.name == "xml:space")
                        {
                            attribute.value.to_owned()
                        } else {
                            with_references(attribute.value, rule)
                        }
                    })
                    .collect();
                if values
                    .iter()
                    .zip(&tag.attributes)
                    .any(|(value, attribute)| value != attribute.value)
                {
                    let mut written = tag.clone();
                    for (attribute, value) in written.attributes.iter_mut().zip(&values) {
                        attribute.value = value;
                    }
                    out.push_str(&write_tag(&written, &TagStyle::AS_WRITTEN));
                    changes += 1;
                } else {
                    out.push_str(raw);
                }
            }
            _ => out.push_str(raw),
        }
        previous = Some(token.kind);
    }
    Ok(changes)
}
