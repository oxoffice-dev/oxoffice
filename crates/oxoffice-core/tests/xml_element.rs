//! The parts of an element read whatever its writing
//! (`oxoffice_core::xml_element`), the check that no code searches for an
//! end tag as text instead (`specs/2026-11-25-equivalent-xml-writings.md`),
//! and the check that no code reads an attribute value from its raw bytes,
//! its references unresolved, but to write it back as read.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_core::xml_element::{
    append_content, canonical_fragment, element_span, find_element, rename, replace_content,
    root_element_span,
};

#[test]
fn an_element_reads_the_same_parts_whatever_its_writing() {
    for (xml, content) in [
        (r#"<w:p w:x="1"><w:r/></w:p>"#, "<w:r/>"),
        ("<w:p\n\tw:x = '1' ><w:r/></w:p\n>", "<w:r/>"),
        (r#"<ox0:p><ox0:r></ox0:r></ox0:p>tail"#, "<ox0:r></ox0:r>"),
        (r#"<p xmlns="urn:x"><p>inner</p></p>"#, "<p>inner</p>"),
        (r#"<w:sdtContent/>"#, ""),
        (r#"<w:sdtContent />after"#, ""),
    ] {
        let span = element_span(xml).unwrap();
        assert_eq!(&xml[span.content.clone()], content, "{xml}");
        assert_eq!(
            span.is_self_closing(),
            content.is_empty() && xml.contains("/>"),
            "{xml}"
        );
        assert!(xml[span.range()].ends_with('>'), "{xml}");
    }
    assert!(element_span("text").is_err());
    assert!(element_span("<w:p><w:r>").is_err());
}

#[test]
fn content_written_into_a_self_closed_element_gets_an_end_tag() {
    assert_eq!(
        replace_content("<w:sdtContent/>", "<w:r/>").unwrap(),
        ("<w:sdtContent><w:r/></w:sdtContent>".to_owned(), 14)
    );
    assert_eq!(
        replace_content(r#"<w:sdtContent w:x="1" />rest"#, "").unwrap(),
        (r#"<w:sdtContent w:x="1" />rest"#.to_owned(), 24)
    );
    assert_eq!(
        replace_content("<w:p ><w:r/></w:p >", "<w:t/>").unwrap(),
        ("<w:p ><w:t/></w:p >".to_owned(), 6)
    );
    assert_eq!(
        append_content("<w:pPr/>", "<w:jc/>").unwrap(),
        "<w:pPr><w:jc/></w:pPr>"
    );
    assert_eq!(
        append_content("<w:pPr><w:b/></w:pPr\n>", "<w:jc/>").unwrap(),
        "<w:pPr><w:b/><w:jc/></w:pPr\n>"
    );
}

#[test]
fn an_element_is_renamed_in_both_tags() {
    assert_eq!(
        rename(r#"<x:t a="1">text</x:t >"#, "x:delText").unwrap(),
        r#"<x:delText a="1">text</x:delText >"#
    );
    assert_eq!(rename("<w:t/>", "w:delText").unwrap(), "<w:delText/>");
}

#[test]
fn an_element_is_found_by_its_name_at_any_depth_and_the_root_after_the_prolog() {
    let xml = r#"<?xml version="1.0"?><a><mc:Choice Requires="wps"><x/></mc:Choice ></a>"#;
    let span = find_element(xml, "mc:Choice").unwrap().unwrap();
    assert_eq!(
        &xml[span.range()],
        r#"<mc:Choice Requires="wps"><x/></mc:Choice >"#
    );
    assert_eq!(find_element(xml, "mc:Fallback").unwrap(), None);
    // A byte order mark counts in the ranges.
    let marked = "\u{feff}<r><x/></r>";
    assert_eq!(find_element(marked, "x").unwrap().unwrap().range(), 6..10);
    assert_eq!(root_element_span(marked).unwrap().content, 6..10);
    let root = root_element_span(xml).unwrap();
    assert_eq!(
        &xml[root.content.clone()],
        &xml[xml.find("<mc").unwrap()..xml.len() - 4]
    );
}

#[test]
fn two_writings_of_a_fragment_read_the_same_canonical_form() {
    let pairs = [
        (r#"<w:b w:val="1"/>"#, "<w:b w:val='1'></w:b>"),
        (
            r#"<w:x w:a="1" w:b="2"/>"#,
            "<w:x\n w:b = \"2\"  w:a=\"1\" />",
        ),
        (
            r#"<ox0:x xmlns:ox0="urn:w" ox0:a="1"/>"#,
            r#"<w:x xmlns:w="urn:w" w:a="&#49;"/>"#,
        ),
        ("<w:t>a&amp;b é</w:t>", "<w:t>a&#38;b &#xE9;</w:t>"),
        (
            "<w:pPr><w:jc/><w:b/></w:pPr>",
            "<w:pPr>\n  <w:jc/>\n  <w:b/>\n</w:pPr>",
        ),
    ];
    for (left, right) in pairs {
        assert_eq!(
            canonical_fragment(left).unwrap(),
            canonical_fragment(right).unwrap(),
            "{left} {right}"
        );
    }
    for (left, right) in [
        (r#"<w:b w:val="1"/>"#, r#"<w:b w:val="0"/>"#),
        // A text's spaces are content, preserved or not.
        ("<w:t>a</w:t>", "<w:t> a</w:t>"),
        ("<w:t> </w:t>", "<w:t></w:t>"),
        (
            r#"<w:r xml:space="preserve"><w:t/> </w:r>"#,
            r#"<w:r xml:space="preserve"><w:t/></w:r>"#,
        ),
    ] {
        assert_ne!(
            canonical_fragment(left).unwrap(),
            canonical_fragment(right).unwrap(),
            "{left} {right}"
        );
    }
}

/// The search methods a hand-written end-tag search goes through.
const SEARCHES: [&str; 12] = [
    "find",
    "rfind",
    "strip_suffix",
    "ends_with",
    "contains",
    "matches",
    "rmatches",
    "match_indices",
    "rmatch_indices",
    "split_once",
    "rsplit_once",
    "windows",
];

/// Searches for an end tag as text that are allowed, by file and literal:
/// they read HTML or XHTML this library writes itself, or the corpus in a
/// test-support crate, never a document's XML.
const ALLOWED: [(&str, &str); 5] = [
    ("crates/oxoffice-docx/src/epub.rs", "</li>"),
    ("crates/oxoffice-docx/src/epub.rs", "</li>\\n"),
    ("crates/oxoffice-docx/src/html.rs", "</a>"),
    ("crates/oxoffice-docx-cli/src/commands.rs", "</body>"),
    ("crates/oxoffice-render-test/src/xml.rs", "</{name}>"),
];

/// Whether `text` starts with a string literal (plain, byte or raw), or a
/// `format!` of one, whose text starts with `</`; and that text.
fn end_tag_literal(text: &str) -> Option<String> {
    let text = text.trim_start().trim_start_matches('&').trim_start();
    let text = text
        .strip_prefix("format!(")
        .map(str::trim_start)
        .unwrap_or(text);
    let text = text
        .strip_prefix("br#")
        .or_else(|| text.strip_prefix("r#"))
        .or_else(|| text.strip_prefix("br"))
        .or_else(|| text.strip_prefix('b'))
        .or_else(|| text.strip_prefix('r'))
        .unwrap_or(text);
    let literal = text.strip_prefix("\"</")?;
    let end = literal.find('"')?;
    Some(format!("</{}", &literal[..end]))
}

/// `source` with each `#[cfg(test)]` item blanked, its lines kept.
fn without_test_items(source: &str) -> String {
    let mut code = source.to_owned();
    while let Some(at) = code.find("#[cfg(test)]") {
        // The item ends at a `;` before any `{`, or at the brace closing it.
        let rest = &code[at..];
        let end = match (rest.find(';'), rest.find('{')) {
            (Some(semicolon), brace) if brace.is_none_or(|brace| semicolon < brace) => {
                semicolon + 1
            }
            (_, Some(brace)) => {
                let mut depth = 0;
                let closing = rest[brace..].char_indices().find(|&(_, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    depth == 0
                });
                closing.map_or(rest.len(), |(i, _)| brace + i + 1)
            }
            _ => rest.len(),
        };
        let blank: String = rest[..end]
            .chars()
            .map(|c| if c == '\n' { c } else { ' ' })
            .collect();
        code.replace_range(at..at + end, &blank);
    }
    code
}

/// The free function called with the argument that starts at `at`, if any:
/// the name before the unclosed `(` it is in, not a method, macro or
/// constructor.
fn callee(code: &str, at: usize) -> Option<&str> {
    let mut depth = 0;
    let open = code[..at].char_indices().rev().find(|&(_, c)| {
        depth += match c {
            ')' => 1,
            '(' => -1,
            _ => 0,
        };
        depth < 0
    })?;
    let before = &code[..open.0];
    let name = &before[before
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .len()..];
    let free = !before[..before.len() - name.len()].ends_with(['.', '!', ':'])
        && name.starts_with(|c: char| c.is_lowercase());
    free.then_some(name)
}

/// Each search for an end tag as text in `source`, but in its
/// `#[cfg(test)]` items: its line and the literal or variable searched.
/// Searched are a literal starting `</` (or a `format!` of one), a variable
/// bound to one, a variable named for an end tag (`close`, `closing`,
/// `end_tag`), and such a literal handed to a function of ours.
fn end_tag_searches(source: &str) -> Vec<(usize, String)> {
    let code = without_test_items(source);
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    let mut found = Vec::new();
    // Variables bound to an end tag: `let close = format!("</{name}>");`.
    let mut variables = Vec::new();
    for (at, _) in code.match_indices("let ") {
        let rest = &code[at + 4..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if let Some(value) = rest[name.len()..].trim_start().strip_prefix('=')
            && let Some(literal) = end_tag_literal(value)
            && !name.is_empty()
        {
            variables.push((name, literal));
        }
    }
    let identifier = |text: &str| -> String {
        let text = text.trim_start().trim_start_matches('&').trim_start();
        text.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    for method in SEARCHES {
        let call = format!(".{method}(");
        for (at, _) in code.match_indices(&call) {
            let argument = &code[at + call.len()..];
            let name = identifier(argument);
            let literal = end_tag_literal(argument)
                .or_else(|| {
                    variables
                        .iter()
                        .find(|(bound, _)| *bound == name)
                        .map(|(_, l)| l.clone())
                })
                .or_else(|| {
                    ["close", "closing", "end_tag"]
                        .contains(&name.as_str())
                        .then_some(name)
                });
            if let Some(literal) = literal {
                found.push((line_of(at), literal));
            }
        }
    }
    // `window == b"</p:sld>"`, and `inject_before_close(xml, "</w:pPr>", ..)`.
    for (at, _) in code.match_indices("\"</") {
        let start = code[..at].trim_end_matches(['b', 'r', '#']).len();
        let before = code[..start].trim_end().trim_end_matches('&').trim_end();
        let handed =
            (before.ends_with('(') || before.ends_with(',')) && callee(&code, start).is_some();
        if before.ends_with("==") || handed {
            found.extend(end_tag_literal(&code[start..]).map(|literal| (line_of(at), literal)));
        }
    }
    found.sort();
    found.dedup();
    found
}

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a source folder is readable") {
        let path = entry.expect("a source entry is readable").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_check_finds_an_end_tag_searched_as_text() {
    let source = concat!(
        "let a = xml.rfind(&format!(\"</{name}>\"));\n",
        "let close = format!(\"</{name}>\");\nlet b = xml.rfind(&close);\n",
        "let c = xml\n    .find(\"</mc:Choice>\");\n",
        "let d = bytes.windows(b\"</p:sld>\".len()).position(|w| w == b\"</p:sld>\");\n",
        "let e = xml.strip_suffix(b\"</w:body>\");\n",
        "let f = xml.find(\"<w:p\");\n",
        "#[cfg(test)]\nmod tests { fn t() { xml.rfind(\"</w:p>\"); } }\n",
        "let (_, closing) = paragraph_shell(xml)?;\nlet g = xml.rfind(&closing);\n",
        "let h = inject_before_close(&xml, \"</w:pPr>\", &change)?;\n",
        "out.push_str(\"</w:p>\");\nlet i = Some(\"</w:p>\");\n",
    );
    let lines: Vec<usize> = end_tag_searches(source)
        .into_iter()
        .map(|(line, _)| line)
        .collect();
    assert_eq!(lines, [1, 3, 5, 6, 7, 12, 13]);
}

// An end tag found by searching for its text misses `</w:p >` and an element
// written self-closing, as #656 did: every such search goes through
// `oxoffice_core::xml_element`.
#[test]
fn no_code_searches_for_an_end_tag_as_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("crates")).expect("crates/ is readable") {
        let src = entry.expect("a crate entry is readable").path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    assert!(files.len() > 100, "the walk found {} files", files.len());
    let mut refused = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).expect("a source file is UTF-8");
        for (line, literal) in end_tag_searches(&source) {
            if !ALLOWED.contains(&(relative.as_str(), literal.as_str())) {
                refused.push(format!("{relative}:{line} searches for {literal:?}"));
            }
        }
    }
    assert!(
        refused.is_empty(),
        "find an element's parts with oxoffice_core::xml_element instead:\n{}",
        refused.join("\n")
    );
}

/// Files that use an attribute's raw bytes, by how many uses and why:
/// `preserved`, a value written back as read (by `push_raw` or a lexical
/// copy), which must keep its references; `lexical`, a value compared or
/// kept as written beside its decoded form, where another spelling counts
/// as a change; `namespace`, a namespace declaration, which quick-xml's
/// resolver also reads as written; `declaration`, the XML declaration, which
/// holds no reference; `strict`, a value that resolves its references itself
/// and refuses a malformed one; `position`, where a value sits in the
/// source, not what it says; `decoded`, a field of another type that holds
/// a decoded value; `reader`, a reader of attribute values itself; `test`, a
/// test module. Every other value is read with
/// `oxoffice_core::xml::attribute_text`.
const RAW_READS: [(&str, usize, &str); 57] = [
    ("crates/oxoffice-core/src/xml.rs", 1, "reader"),
    ("crates/oxoffice-core/src/xml.rs", 1, "preserved"),
    ("crates/oxoffice-core/src/xml.rs", 1, "namespace"),
    ("crates/oxoffice-core/src/xml.rs", 1, "declaration"),
    ("crates/oxoffice-docx-oxml/src/borders.rs", 2, "preserved"),
    (
        "crates/oxoffice-docx-oxml/src/document/section.rs",
        7,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/document/source/admission.rs",
        1,
        "namespace",
    ),
    (
        "crates/oxoffice-docx-oxml/src/document/source/qualification.rs",
        1,
        "namespace",
    ),
    ("crates/oxoffice-docx-oxml/src/glossary.rs", 1, "strict"),
    (
        "crates/oxoffice-docx-oxml/src/paragraph_properties.rs",
        2,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/placeholder.rs",
        1,
        "namespace",
    ),
    (
        "crates/oxoffice-docx-oxml/src/properties.rs",
        4,
        "preserved",
    ),
    ("crates/oxoffice-docx-oxml/src/properties.rs", 1, "lexical"),
    (
        "crates/oxoffice-docx-oxml/src/run_properties.rs",
        4,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/run_properties.rs",
        1,
        "strict",
    ),
    ("crates/oxoffice-docx-oxml/src/styles.rs", 1, "lexical"),
    ("crates/oxoffice-docx-oxml/src/styles.rs", 2, "namespace"),
    (
        "crates/oxoffice-docx-oxml/src/text/bindings.rs",
        3,
        "namespace",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/block_bookmarks.rs",
        1,
        "namespace",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/emission.rs",
        1,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_legacy_form.rs",
        1,
        "strict",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_rewrite.rs",
        3,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_write.rs",
        1,
        "preserved",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_write.rs",
        1,
        "namespace",
    ),
    ("crates/oxoffice-docx-oxml/src/text/run.rs", 1, "namespace"),
    (
        "crates/oxoffice-docx/src/bibliography/oracle_tests.rs",
        3,
        "test",
    ),
    (
        "crates/oxoffice-docx/src/comparison/fields.rs",
        2,
        "preserved",
    ),
    ("crates/oxoffice-docx/src/comparison/guard.rs", 1, "lexical"),
    (
        "crates/oxoffice-docx/src/comparison/leaf_publications.rs",
        1,
        "preserved",
    ),
    (
        "crates/oxoffice-docx/src/comparison/source_metadata.rs",
        1,
        "namespace",
    ),
    (
        "crates/oxoffice-docx/src/comparison/source_snapshots.rs",
        1,
        "namespace",
    ),
    (
        "crates/oxoffice-docx/src/content_control/custom_xml.rs",
        1,
        "declaration",
    ),
    (
        "crates/oxoffice-docx/src/content_control/custom_xml.rs",
        1,
        "strict",
    ),
    (
        "crates/oxoffice-docx/src/content_control/form_state.rs",
        4,
        "decoded",
    ),
    (
        "crates/oxoffice-docx/src/content_control/namespaces.rs",
        1,
        "position",
    ),
    (
        "crates/oxoffice-docx/src/document/edited_paragraph.rs",
        1,
        "preserved",
    ),
    (
        "crates/oxoffice-docx/src/document/main_part.rs",
        1,
        "lexical",
    ),
    (
        "crates/oxoffice-docx/src/document/owner_snapshots.rs",
        1,
        "lexical",
    ),
    (
        "crates/oxoffice-docx/src/document/story_scopes.rs",
        1,
        "position",
    ),
    (
        "crates/oxoffice-docx/src/document/story_text.rs",
        1,
        "preserved",
    ),
    (
        "crates/oxoffice-docx/src/document/story_text.rs",
        2,
        "namespace",
    ),
    (
        "crates/oxoffice-docx/src/document/story_xml.rs",
        1,
        "lexical",
    ),
    ("crates/oxoffice-docx/src/field/fragment.rs", 4, "strict"),
    (
        "crates/oxoffice-docx/src/field/story_sources.rs",
        1,
        "namespace",
    ),
    ("crates/oxoffice-docx/src/flat_opc.rs", 2, "namespace"),
    ("crates/oxoffice-docx/src/redaction.rs", 1, "declaration"),
    ("crates/oxoffice-opc/src/relationship.rs", 4, "decoded"),
    ("crates/oxoffice-opc/src/signature.rs", 1, "decoded"),
    ("crates/oxoffice-pptx-oxml/src/diagram.rs", 1, "position"),
    ("crates/oxoffice-pptx-oxml/src/picture.rs", 2, "position"),
    ("crates/oxoffice-pptx-oxml/src/relmap.rs", 1, "position"),
    ("crates/oxoffice-pptx-oxml/src/shape_tree.rs", 1, "position"),
    ("crates/oxoffice-pptx/src/header_footer.rs", 1, "namespace"),
    ("crates/oxoffice-pptx/src/import.rs", 1, "position"),
    ("crates/oxoffice-pptx/src/import_tables.rs", 1, "decoded"),
    ("crates/oxoffice-pptx/src/xml_owner.rs", 1, "preserved"),
    ("crates/oxoffice-xml-test/src/canonical.rs", 1, "reader"),
];

/// Uses of an attribute's raw bytes that say where it sits, not what it
/// says, and fields that only a decoded string has (quick-xml's
/// `Cow<[u8]>` has no `as_str` or `trim`).
const POSITIONAL: [&str; 8] = [
    ".len()",
    ".as_ptr()",
    ".as_ref().as_ptr()",
    ".as_ref().len()",
    ".contains(&b'",
    ".is_empty()",
    ".as_str()",
    ".trim()",
];

/// The lines of `source`, but its `#[cfg(test)]` items, that use an
/// attribute's raw bytes: every `attr.value` or `attribute.value` borrowed,
/// converted, compared or passed on, but its positional uses and an
/// assignment; and, under any name, a value handed to `from_utf8` (or
/// `_lossy`) or compared with a byte string. However the code is laid out.
fn raw_attribute_reads(source: &str) -> Vec<usize> {
    let code = without_test_items(source);
    let word = |c: char| c.is_alphanumeric() || c == '_';
    // The code without white space but between two words, each character
    // with its line.
    let (mut compact, mut lines) = (String::new(), Vec::new());
    let (mut line, mut spaced) = (1, false);
    for c in code.chars() {
        if c == '\n' {
            line += 1;
        }
        if c.is_whitespace() {
            spaced = true;
            continue;
        }
        if spaced && compact.ends_with(word) && word(c) {
            compact.push(' ');
            lines.push(line);
        }
        spaced = false;
        compact.push(c);
        lines.push(line);
    }
    let mut found = Vec::new();
    for (at, _) in compact.match_indices(".value") {
        let after = &compact[at + ".value".len()..];
        if after.starts_with(word) {
            continue;
        }
        let before = &compact[..at];
        let name = &before[before.trim_end_matches(word).len()..];
        let start = at - name.len();
        if name.is_empty() {
            continue;
        }
        let handed = [
            "from_utf8(",
            "from_utf8(&",
            "from_utf8_lossy(",
            "from_utf8_lossy(&",
        ]
        .iter()
        .any(|call| compact[..start].ends_with(call));
        let compared = [".as_ref()==b\"", ".as_ref()!=b\"", "==b\"", "!=b\""]
            .iter()
            .any(|comparison| after.starts_with(comparison));
        let attribute = matches!(name, "attr" | "attribute");
        let positional = POSITIONAL.iter().any(|use_| after.starts_with(use_))
            || (after.starts_with('=') && !after.starts_with("=="));
        if handed || compared || (attribute && !positional) {
            found.push(lines[start]);
        }
    }
    found
}

#[test]
fn the_check_finds_an_attribute_value_read_from_its_raw_bytes() {
    let source = concat!(
        "let a = std::str::from_utf8(&attr.value)?;\n",
        "let b = String::from_utf8_lossy(\n    &attribute.value,\n);\n",
        "let c = std::str::from_utf8(record.value.as_ref());\n",
        "let d = attribute.value.as_ref() == b\"preserve\";\n",
        "let e = matches!(attribute.value.as_ref(), b\"0\" | b\"false\");\n",
        "let f = oxoffice_core::xml::attribute_text(&attribute)?;\n",
        "let g = std::str::from_utf8(attribute.key.as_ref())?;\n",
        "let h = std::str::from_utf8(&run.value_text)?;\n",
        "#[cfg(test)]\nmod tests { fn t() { std::str::from_utf8(&attr.value); } }\n",
        "let i = String::from_utf8(attr\n    .value\n    .to_vec());\n",
        "let j = attribute.value.into_owned();\n",
        "let k = text(attribute.value.as_ref());\n",
        "match attr.value.as_ref() { b\"x\" => {} _ => {} }\n",
        "let l = b\"x\" == attr.value.as_ref();\n",
        "let v = attr.value.as_ref(); let m = std::str::from_utf8(v);\n",
        "let n = attribute.value.len() + attribute.value.as_ptr() as usize;\n",
        "attribute.value = Cow::Owned(quoted);\n",
        "let o = entry.value.clone() + self.value;\n",
        "let Cow::Borrowed(raw) = &attribute.value else { return };\n",
    );
    assert_eq!(
        raw_attribute_reads(source),
        [1, 3, 5, 6, 7, 13, 16, 17, 18, 19, 20, 24]
    );
}

// A value read from its raw bytes ignores its character references, as #668
// had it: `w:before="&#50;40"` did not open, and ids and style ids written
// with references read wrong. A value is read with
// `oxoffice_core::xml::attribute_text`, or kept raw only to be written back
// as read.
#[test]
fn no_code_reads_an_attribute_value_from_its_raw_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("crates")).expect("crates/ is readable") {
        let src = entry.expect("a crate entry is readable").path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    assert!(files.len() > 100, "the walk found {} files", files.len());
    let mut refused = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).expect("a source file is UTF-8");
        let reads = raw_attribute_reads(&source);
        let allowed: usize = RAW_READS
            .iter()
            .filter(|(file, ..)| *file == relative)
            .map(|(_, count, _)| count)
            .sum();
        if reads.len() != allowed {
            refused.push(format!(
                "{relative} reads {} raw values (lines {reads:?}), {allowed} allowed",
                reads.len()
            ));
        }
    }
    assert!(
        refused.is_empty(),
        "read a value with oxoffice_core::xml::attribute_text, or list a value \
         written back as read in RAW_READS:\n{}",
        refused.join("\n")
    );
}
