//! Paragraph properties of a bibliography field's result.

use super::*;

pub(crate) fn bibliography_paragraph_property_ranges(
    xml: &[u8],
) -> Result<BTreeMap<usize, Range<usize>>> {
    const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let parsed = SourceXml::parse(xml)?;
    let mut ranges = BTreeMap::new();
    for paragraph in parsed
        .nodes
        .iter()
        .filter(|node| node.namespace == WORD && node.local == "p")
    {
        let properties = paragraph
            .children
            .iter()
            .map(|&child| &parsed.nodes[child])
            .filter(|node| node.namespace == WORD && node.local == "pPr")
            .collect::<Vec<_>>();
        match properties.as_slice() {
            [] => {}
            [properties] => {
                ranges.insert(paragraph.full.start, properties.full.clone());
            }
            _ => return Err(bibliography_error("ambiguous paragraph property ownership")),
        }
    }
    Ok(ranges)
}

pub(crate) fn bibliography_end_paragraph_properties(
    xml: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let mut prefix = String::from("<rdocx-bibliography-properties");
    for (name, value) in bindings {
        if name == "xml" {
            continue;
        }
        let attribute = if name.is_empty() {
            "xmlns".into()
        } else {
            format!("xmlns:{name}")
        };
        prefix.push_str(&format!(
            " {attribute}=\"{}\"",
            quick_xml::escape::escape(value)
        ));
    }
    prefix.push('>');
    let mut scoped = prefix.as_bytes().to_vec();
    scoped.extend_from_slice(xml);
    scoped.extend_from_slice(b"</rdocx-bibliography-properties>");
    let parsed = SourceXml::parse(&scoped)?;
    let [root] = parsed.nodes[0].children.as_slice() else {
        return Err(bibliography_error(
            "ambiguous preserved paragraph properties",
        ));
    };
    let properties = &parsed.nodes[*root];
    if properties.namespace != WORD || properties.local != "pPr" {
        return Err(bibliography_error(
            "unsupported paragraph property namespace",
        ));
    }
    let styles = properties
        .children
        .iter()
        .map(|&child| &parsed.nodes[child])
        .filter(|node| node.namespace == WORD && node.local == "pStyle")
        .collect::<Vec<_>>();
    if styles.len() > 1 {
        return Err(bibliography_error("ambiguous preserved paragraph style"));
    }
    if let [style] = styles.as_slice()
        && style.empty
        && style.children.is_empty()
        && !style.opaque_scalar
        && style.attributes.as_slice() == [(WORD.into(), "val".into(), "Normal".into())]
    {
        let range = style
            .full
            .start
            .checked_sub(prefix.len())
            .zip(style.full.end.checked_sub(prefix.len()))
            .filter(|(start, end)| start <= end && *end <= xml.len())
            .ok_or_else(|| {
                bibliography_error("preserved style escaped its paragraph properties")
            })?;
        return apply_source_edits(xml, vec![(range.0..range.1, Vec::new())]);
    }
    Ok(xml.to_vec())
}

// Reset only recognized formatting particles. Namespace-qualified opaque XML remains raw.
pub(crate) fn bibliography_first_paragraph_properties(
    original: &[u8],
    generated: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const P: &[&str] = &[
        "pStyle",
        "keepNext",
        "keepLines",
        "pageBreakBefore",
        "framePr",
        "widowControl",
        "numPr",
        "suppressLineNumbers",
        "pBdr",
        "shd",
        "tabs",
        "suppressAutoHyphens",
        "kinsoku",
        "wordWrap",
        "overflowPunct",
        "topLinePunct",
        "autoSpaceDE",
        "autoSpaceDN",
        "bidi",
        "adjustRightInd",
        "snapToGrid",
        "spacing",
        "ind",
        "contextualSpacing",
        "mirrorIndents",
        "suppressOverlap",
        "jc",
        "textDirection",
        "textAlignment",
        "textboxTightWrap",
        "outlineLvl",
        "divId",
        "cnfStyle",
        "rPr",
        "sectPr",
        "pPrChange",
    ];
    const R: &[&str] = &[
        "rStyle",
        "rFonts",
        "b",
        "bCs",
        "i",
        "iCs",
        "caps",
        "smallCaps",
        "strike",
        "dstrike",
        "outline",
        "shadow",
        "emboss",
        "imprint",
        "noProof",
        "snapToGrid",
        "vanish",
        "webHidden",
        "color",
        "spacing",
        "w",
        "kern",
        "position",
        "sz",
        "szCs",
        "highlight",
        "u",
        "effect",
        "bdr",
        "shd",
        "fitText",
        "vertAlign",
        "rtl",
        "cs",
        "em",
        "lang",
        "eastAsianLayout",
        "specVanish",
        "oMath",
        "ins",
        "del",
        "rPrChange",
    ];
    let mut prefix = String::from("<rdocx-bibliography-properties");
    for (name, value) in bindings {
        if name == "xml" {
            continue;
        }
        let attribute = if name.is_empty() {
            "xmlns".into()
        } else {
            format!("xmlns:{name}")
        };
        prefix.push_str(&format!(
            " {attribute}=\"{}\"",
            quick_xml::escape::escape(value)
        ));
    }
    prefix.push('>');
    let mut scoped = prefix.as_bytes().to_vec();
    scoped.extend_from_slice(original);
    scoped.extend_from_slice(generated);
    scoped.extend_from_slice(b"</rdocx-bibliography-properties>");
    let parsed = SourceXml::parse(&scoped)?;
    let [old, new] = parsed.nodes[0].children.as_slice() else {
        return Err(bibliography_error("ambiguous first paragraph properties"));
    };
    if [&parsed.nodes[*old], &parsed.nodes[*new]]
        .iter()
        .any(|node| node.namespace != WORD || node.local != "pPr")
    {
        return Err(bibliography_error(
            "unsupported first paragraph property namespace",
        ));
    }
    fn owned(node: &SourceXmlNode, order: &[&str]) -> bool {
        node.namespace == WORD
            && order.contains(&node.local.as_str())
            && !matches!(
                node.local.as_str(),
                "sectPr" | "pPrChange" | "ins" | "del" | "rPrChange"
            )
    }
    fn merge(
        parsed: &SourceXml,
        xml: &[u8],
        old: usize,
        new: usize,
        order: &[&str],
    ) -> Result<Vec<u8>> {
        let old = &parsed.nodes[old];
        let new = &parsed.nodes[new];
        // History and section boundaries are caller metadata rather than formatting.
        fn validate(parsed: &SourceXml, old: &SourceXmlNode, order: &[&str]) -> Result<()> {
            let mut seen = BTreeSet::new();
            let mut previous = 0;
            for &child in &old.children {
                let node = &parsed.nodes[child];
                if owned(node, order) {
                    let rank = order.iter().position(|name| *name == node.local).unwrap();
                    if !seen.insert(node.local.as_str()) || rank < previous {
                        return Err(bibliography_error(
                            "ambiguous first paragraph formatting particles",
                        ));
                    }
                    previous = rank;
                    if node.local != "rPr"
                        && (node.attributes.iter().any(|(ns, local, _)| {
                            let attributes: &[&str] = match node.local.as_str() {
                                "numPr" | "pBdr" | "tabs" => &[],
                                "ind" => &[
                                    "left",
                                    "leftChars",
                                    "right",
                                    "rightChars",
                                    "start",
                                    "startChars",
                                    "end",
                                    "endChars",
                                    "hanging",
                                    "hangingChars",
                                    "firstLine",
                                    "firstLineChars",
                                ],
                                "spacing" if order == P => &[
                                    "before",
                                    "beforeLines",
                                    "beforeAutospacing",
                                    "after",
                                    "afterLines",
                                    "afterAutospacing",
                                    "line",
                                    "lineRule",
                                ],
                                "rFonts" => &[
                                    "ascii",
                                    "hAnsi",
                                    "eastAsia",
                                    "cs",
                                    "asciiTheme",
                                    "hAnsiTheme",
                                    "eastAsiaTheme",
                                    "cstheme",
                                    "hint",
                                ],
                                "color" => &["val", "themeColor", "themeTint", "themeShade"],
                                "lang" => &["val", "eastAsia", "bidi"],
                                "u" => &["val", "color", "themeColor", "themeTint", "themeShade"],
                                "shd" => &[
                                    "val",
                                    "color",
                                    "fill",
                                    "themeColor",
                                    "themeTint",
                                    "themeShade",
                                    "themeFill",
                                    "themeFillTint",
                                    "themeFillShade",
                                ],
                                "bdr" => &[
                                    "val",
                                    "color",
                                    "themeColor",
                                    "themeTint",
                                    "themeShade",
                                    "sz",
                                    "space",
                                    "shadow",
                                    "frame",
                                ],
                                "fitText" => &["val", "id"],
                                "eastAsianLayout" => {
                                    &["id", "combine", "combineBrackets", "vert", "vertCompress"]
                                }
                                "framePr" => &[
                                    "dropCap",
                                    "lines",
                                    "w",
                                    "h",
                                    "vSpace",
                                    "hSpace",
                                    "wrap",
                                    "hAnchor",
                                    "vAnchor",
                                    "x",
                                    "xAlign",
                                    "y",
                                    "yAlign",
                                    "hRule",
                                    "anchorLock",
                                ],
                                "cnfStyle" => &[
                                    "val",
                                    "firstRow",
                                    "lastRow",
                                    "firstColumn",
                                    "lastColumn",
                                    "oddVBand",
                                    "evenVBand",
                                    "oddHBand",
                                    "evenHBand",
                                    "firstRowFirstColumn",
                                    "firstRowLastColumn",
                                    "lastRowFirstColumn",
                                    "lastRowLastColumn",
                                ],
                                _ => &["val"],
                            };
                            ns != WORD || !attributes.contains(&local.as_str())
                        }) || !node.children.is_empty()
                            || node.opaque_scalar
                            || !node.text.trim().is_empty())
                    {
                        return Err(bibliography_error(
                            "opaque metadata in replaced formatting particle",
                        ));
                    }
                    if node.local == "rPr" {
                        validate(parsed, node, R)?;
                    }
                }
            }
            Ok(())
        }
        validate(parsed, old, order)?;
        if old.empty {
            let tag = oxoffice_core::xml_element::start_tag(&xml[old.full.start..])?;
            let mut result = xml[old.full.start..old.full.start + tag.attributes_end].to_vec();
            result.push(b'>');
            for &child in &new.children {
                result.extend_from_slice(&generated_particle(
                    &xml[parsed.nodes[child].full.clone()],
                )?);
            }
            result.extend_from_slice(format!("</{}>", old.name).as_bytes());
            return Ok(result);
        }
        let mut edits = Vec::new();
        let mut inserted = BTreeSet::new();
        for &child in &old.children {
            let node = &parsed.nodes[child];
            if !owned(node, order) {
                continue;
            }
            let counterpart = new.children.iter().copied().find(|&n| {
                parsed.nodes[n].namespace == WORD && parsed.nodes[n].local == node.local
            });
            let replacement = if let Some(n) = counterpart {
                inserted.insert(n);
                if node.local == "rPr" {
                    merge(parsed, xml, child, n, R)?
                } else {
                    generated_particle(&xml[parsed.nodes[n].full.clone()])?
                }
            } else {
                if node.local == "rPr"
                    && (!node.attributes.is_empty()
                        || node.opaque_scalar
                        || !node.text.trim().is_empty()
                        || node.children.iter().any(|&child| {
                            let child = &parsed.nodes[child];
                            child.namespace != WORD
                                || !R.contains(&child.local.as_str())
                                || matches!(child.local.as_str(), "ins" | "del" | "rPrChange")
                        }))
                {
                    return Err(bibliography_error(
                        "removal would discard paragraph run metadata",
                    ));
                }
                Vec::new()
            };
            edits.push((
                node.full.start - old.full.start..node.full.end - old.full.start,
                replacement,
            ));
        }
        for &child in &new.children {
            if inserted.contains(&child) {
                continue;
            }
            let node = &parsed.nodes[child];
            let rank = order
                .iter()
                .position(|name| *name == node.local)
                .ok_or_else(|| bibliography_error("unexpected generated formatting particle"))?;
            let position = old
                .children
                .iter()
                .map(|&n| &parsed.nodes[n])
                .find(|n| {
                    n.namespace == WORD
                        && order
                            .iter()
                            .position(|name| *name == n.local)
                            .is_some_and(|r| r > rank)
                })
                .map_or(old.content.end, |n| n.full.start);
            edits.push((
                position - old.full.start..position - old.full.start,
                generated_particle(&xml[node.full.clone()])?,
            ));
        }
        apply_source_edits(&xml[old.full.clone()], edits)
    }
    fn generated_particle(xml: &[u8]) -> Result<Vec<u8>> {
        crate::field::xml_fragment_with_namespaces(
            xml,
            &BTreeMap::from([("w".into(), WORD.into())]),
            "generated bibliography formatting",
        )
    }
    merge(&parsed, &scoped, *old, *new, P)
}
