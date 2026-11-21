//! Reading a few elements and attributes of a package part as text, for the
//! facts a check needs that the facades do not give.

/// An XML text without the elements `<name>...</name>`, which do not nest.
pub fn without(xml: &str, name: &str) -> String {
    let mut out = xml.to_owned();
    for element in elements_with_body(xml, name) {
        out = out.replacen(element, "", 1);
    }
    out
}

/// A part name relative to the folder of the part that names it.
pub fn resolve(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        folder.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in target.trim_start_matches('/').split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The start tags `<name ...>` of an XML text, attributes included.
pub fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        if after.starts_with([' ', '>', '/', '\n', '\r', '\t']) {
            let end = after.find('>').map_or(after.len(), |end| end + 1);
            found.push(&rest[start..start + open.len() + end]);
        }
        rest = after;
    }
    found
}

/// The elements `<name ...>...</name>` of an XML text, start tag and body,
/// assuming they do not nest; an empty element `<name .../>` alone.
pub fn elements_with_body<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    for start_tag in elements(xml, name) {
        let Some(offset) = rest.find(start_tag) else {
            continue;
        };
        let from = &rest[offset..];
        let element = if start_tag.ends_with("/>") {
            start_tag
        } else {
            from.find(&close)
                .map_or(start_tag, |end| &from[..end + close.len()])
        };
        found.push(element);
        rest = &from[start_tag.len()..];
    }
    found
}

pub fn attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = element.find(&key)? + key.len();
    let end = element[start..].find('"')?;
    Some(&element[start..start + end])
}

/// Whether an on/off element is present and on (`<w:titlePg/>`,
/// `<w:titlePg w:val="true"/>`).
pub fn on(xml: &str, name: &str) -> bool {
    elements(xml, name)
        .iter()
        .any(|element| !matches!(attribute(element, "w:val"), Some("0" | "false" | "off")))
}
