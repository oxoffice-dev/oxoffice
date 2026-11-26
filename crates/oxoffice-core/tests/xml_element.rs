//! The parts of an element read whatever its writing
//! (`oxoffice_core::xml_element`), the check that no code searches for an
//! end tag as text instead (`specs/2026-11-25-equivalent-xml-writings.md`),
//! the check that no code takes a tag's close as `/>`
//! (`specs/2026-11-26-empty-elements-with-end-tags.md`), and the check that
//! no code reads an attribute value from its raw bytes, its references
//! unresolved, but to write it back as read.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_core::xml_element::{
    append_content, canonical_fragment, element_span, find_element, rename, replace_content,
    root_element_span, start_tag,
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
fn a_start_tag_says_where_its_attributes_end_whatever_its_writing() {
    for (xml, end, attributes_end, self_closing) in [
        ("<x/>", 4, 2, true),
        ("<x />", 5, 3, true),
        ("<x a='1'\n/>", 11, 9, true),
        ("<x>", 3, 2, false),
        (r#"<x a="1" >"#, 10, 9, false),
        ("<x></x>", 3, 2, false),
    ] {
        let tag = start_tag(xml).unwrap();
        assert_eq!(
            (
                tag.name.as_str(),
                tag.range.clone(),
                tag.attributes_end,
                tag.self_closing
            ),
            ("x", 0..end, attributes_end, self_closing),
            "{xml}"
        );
        assert_eq!(start_tag(xml.as_bytes()).unwrap(), tag, "{xml}");
        let marked = start_tag(&format!("\u{feff}{xml}")).unwrap();
        assert_eq!(
            (marked.range, marked.attributes_end),
            (3..end + 3, attributes_end + 3),
            "{xml}"
        );
    }
    assert!(start_tag(" <x/>").is_err());
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
    // Bytes give bytes back, the start tag kept as written.
    assert_eq!(
        replace_content(b"<w:pPr />".as_slice(), "<w:jc/>").unwrap(),
        (b"<w:pPr ><w:jc/></w:pPr>".to_vec(), 8)
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
        // Under any name, a value tested against a byte string: compared
        // either way round, or matched (`matches!` or `match`).
        let value = after.strip_prefix(".as_ref()").unwrap_or(after);
        let compared_after = ["==b\"", "!=b\""]
            .iter()
            .any(|comparison| value.starts_with(comparison));
        let compared_before = ["==", "!="].iter().any(|operator| {
            compact[..start]
                .strip_suffix(operator)
                .and_then(|left| left.strip_suffix('"'))
                .and_then(|left| left.rfind('"').map(|quote| &left[..quote]))
                .is_some_and(|left| left.ends_with('b'))
        });
        let matched = (compact[..start].ends_with("matches!(") && value.starts_with(",b\""))
            || (compact[..start].ends_with("match ") && value.starts_with("{b\""));
        let compared = compared_after || compared_before || matched;
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
        "let p = matches!(a.value.as_ref(), b\"0\" | b\"false\");\n",
        "let q = b\"x\" == a.value.as_ref();\n",
        "match a.value.as_ref() { b\"x\" => {} _ => {} }\n",
        "match option.value { Some(v) => {} None => {} }\n",
    );
    assert_eq!(
        raw_attribute_reads(source),
        [1, 3, 5, 6, 7, 13, 16, 17, 18, 19, 20, 24, 25, 26, 27]
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

/// `line` without white space but between two words.
fn compact(line: &str) -> String {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut compact = String::new();
    let mut spaced = false;
    for c in line.chars() {
        if c.is_whitespace() {
            spaced = true;
            continue;
        }
        if spaced && compact.ends_with(word) && word(c) {
            compact.push(' ');
        }
        spaced = false;
        compact.push(c);
    }
    compact
}

/// Whether `code`, compacted, ending just before `- 2` or `.saturating_sub(2)`,
/// ends with a tag's end: a name `after`, `end` or `*_end`, or the reader's
/// position.
fn ends_with_a_tag_end(code: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let name = &code[code.trim_end_matches(word).len()..];
    name == "after"
        || name == "end"
        || name.ends_with("_end")
        || code.ends_with("buffer_position()as usize")
        || code.ends_with("buffer_position()as usize)")
}

/// `code` before the path (`a.b.c`) it ends with, not before the `..` of a
/// range.
fn before_path(code: &str) -> &str {
    let mut rest = code;
    loop {
        rest = rest.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        match rest.strip_suffix('.') {
            Some(shorter) if !shorter.ends_with('.') => rest = shorter,
            _ => return rest,
        }
    }
}

/// Whether one line of code takes two bytes from a tag's end: `after - 2`,
/// `buffer_position() as usize - 2`, `end.saturating_sub(2)`,
/// `truncate(x.len() - 2)`, `[..x.len() - 2]`, `x.len() - 2..`.
fn takes_two_from_a_tag_end(line: &str) -> bool {
    let code = compact(line);
    // `code` before a path (`a.b.c`), not before the `..` of a range.
    for (at, _) in code.match_indices("-2") {
        let (before, after) = (&code[..at], &code[at + 2..]);
        let mut next = after.chars();
        let number = match next.next() {
            Some(c) if c.is_ascii_digit() => true,
            Some('.') => next.next().is_some_and(|c| c.is_ascii_digit()),
            _ => false,
        };
        if number {
            continue;
        }
        if ends_with_a_tag_end(before) {
            return true;
        }
        if let Some(receiver) = before.strip_suffix(".len()") {
            let outside = before_path(receiver);
            if after.starts_with("..")
                || (outside.ends_with("truncate(") && after.starts_with(')'))
                || (outside.ends_with("[..") && after.starts_with(']'))
            {
                return true;
            }
        }
    }
    ["saturating_sub(2)", "checked_sub(2)", "wrapping_sub(2)"]
        .iter()
        .any(|call| {
            code.match_indices(&format!(".{call}"))
                .any(|(at, _)| ends_with_a_tag_end(&code[..at]))
        })
}

/// Whether one line of code assumes that an empty element's bytes end with
/// `/>`: a two-character literal `"/>"`, a byte compared with `b'/'` at an
/// offset, the last `/` taken for the close, a tag's close taken as two
/// bytes or one, or two bytes taken from a tag's end.
fn assumes_self_closing(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with("//") {
        return false;
    }
    let literal = line.match_indices("\"/>\"").any(|(at, _)| {
        let before = &line[..at];
        let before = before
            .strip_suffix("br")
            .or_else(|| before.strip_suffix('b'))
            .or_else(|| before.strip_suffix('r'))
            .unwrap_or(before);
        before.is_empty()
            || before.ends_with(|c: char| c.is_whitespace() || "(,=[{&!|:".contains(c))
    });
    let code = compact(line);
    let at_offset = [
        "-1]==b'/'",
        "-2]==b'/'",
        "-1]!=b'/'",
        "-2]!=b'/'",
        "==Some(&b'/')",
        "!=Some(&b'/')",
    ]
    .iter()
    .any(|pattern| code.contains(pattern));
    // `.rposition(|byte| *byte == b'/')`: the last slash taken for the close.
    let last_slash = code.contains("rposition(|") && code.contains("==b'/')");
    let one_or_two = ((code.contains("-if ") || code.contains("_sub(if "))
        && code.contains("{2}else{1}"))
        || code.contains("Empty(_)=>2");
    literal || at_offset || last_slash || one_or_two || takes_two_from_a_tag_end(line)
}

/// The lines of `source`, but its `#[cfg(test)]` items, that assume an empty
/// element's bytes end with `/>`.
fn self_closing_assumptions(source: &str) -> Vec<usize> {
    without_test_items(source)
        .lines()
        .enumerate()
        .filter(|(_, line)| assumes_self_closing(line))
        .map(|(index, _)| index + 1)
        .collect()
}

/// Files that write `/>` or count a tag's close, by how many lines and why:
/// `helper`, the one place that knows a tag's close (`xml_element`) and the
/// verbatim capture (`raw_xml`); `writes`, an element the code builds, not
/// one it read; `not a document part`, HTML, EPUB, SVG or RTF; `not a tag`,
/// arithmetic on something else; `verbatim reader`, a crate whose readers
/// deliver elements as written (packages, spreadsheets, test support);
/// `to move`, a site this pull request moves onto `xml_element`. Every other
/// line takes where a tag ends from `oxoffice_core::xml_element`.
const SELF_CLOSING: [(&str, usize, &str); 61] = [
    ("crates/oxoffice-chart/src/authoring.rs", 2, "to move"),
    ("crates/oxoffice-core/src/raw_xml.rs", 1, "helper"),
    ("crates/oxoffice-core/src/xml_element.rs", 2, "helper"),
    (
        "crates/oxoffice-docx-layout/src/engine/cache_accounting.rs",
        1,
        "to move",
    ),
    ("crates/oxoffice-docx-oxml/src/math.rs", 2, "writes"),
    (
        "crates/oxoffice-docx-oxml/src/text/bindings.rs",
        1,
        "writes",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_legacy_form.rs",
        1,
        "writes",
    ),
    (
        "crates/oxoffice-docx-oxml/src/text/field_rewrite.rs",
        1,
        "writes",
    ),
    ("crates/oxoffice-docx/src/bibliography.rs", 2, "to move"),
    (
        "crates/oxoffice-docx/src/bibliography/paragraph_properties.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/bibliography/source.rs",
        3,
        "to move",
    ),
    ("crates/oxoffice-docx/src/building_block.rs", 1, "to move"),
    ("crates/oxoffice-docx/src/comments.rs", 5, "to move"),
    (
        "crates/oxoffice-docx/src/comparison/controls.rs",
        1,
        "to move",
    ),
    ("crates/oxoffice-docx/src/comparison/fields.rs", 1, "writes"),
    (
        "crates/oxoffice-docx/src/comparison/leaf_publications.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/comparison/paragraphs.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/comparison/text_boxes.rs",
        1,
        "to move",
    ),
    ("crates/oxoffice-docx/src/comparison/xml.rs", 1, "to move"),
    (
        "crates/oxoffice-docx/src/content_control/data_binding.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/content_control/element_index.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/content_control/namespaces.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/block_locations.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/edited_paragraph.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/fragment_checks.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/namespaces.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/story_scopes.rs",
        2,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/story_source_edits.rs",
        1,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/document/story_text.rs",
        1,
        "not a tag",
    ),
    (
        "crates/oxoffice-docx/src/document/story_text.rs",
        3,
        "to move",
    ),
    ("crates/oxoffice-docx/src/epub.rs", 2, "not a document part"),
    (
        "crates/oxoffice-docx/src/field/bibliography.rs",
        2,
        "to move",
    ),
    (
        "crates/oxoffice-docx/src/field/merge_package.rs",
        2,
        "to move",
    ),
    ("crates/oxoffice-docx/src/field/ref_copy.rs", 1, "to move"),
    ("crates/oxoffice-docx/src/field/scan.rs", 4, "to move"),
    (
        "crates/oxoffice-docx/src/field/story_sources.rs",
        1,
        "to move",
    ),
    ("crates/oxoffice-docx/src/field/toc.rs", 1, "to move"),
    ("crates/oxoffice-docx/src/html.rs", 1, "not a document part"),
    ("crates/oxoffice-docx/src/revision.rs", 3, "to move"),
    ("crates/oxoffice-docx/src/rtf.rs", 1, "not a document part"),
    ("crates/oxoffice-docx/src/svg.rs", 2, "not a document part"),
    ("crates/oxoffice-media/src/lib.rs", 1, "verbatim reader"),
    (
        "crates/oxoffice-opc/src/relationship.rs",
        5,
        "verbatim reader",
    ),
    ("crates/oxoffice-pptx-oxml/src/comments.rs", 2, "to move"),
    ("crates/oxoffice-pptx-oxml/src/comments.rs", 2, "writes"),
    ("crates/oxoffice-pptx-oxml/src/namespace.rs", 3, "to move"),
    ("crates/oxoffice-pptx-oxml/src/picture.rs", 1, "to move"),
    ("crates/oxoffice-pptx-oxml/src/picture.rs", 1, "writes"),
    (
        "crates/oxoffice-pptx-oxml/src/presentation.rs",
        2,
        "to move",
    ),
    ("crates/oxoffice-pptx-oxml/src/shape_tree.rs", 1, "to move"),
    ("crates/oxoffice-pptx-oxml/src/slide_parts.rs", 2, "to move"),
    ("crates/oxoffice-pptx-oxml/src/timing.rs", 3, "to move"),
    ("crates/oxoffice-pptx/src/hyperlinks.rs", 2, "to move"),
    ("crates/oxoffice-pptx/src/import_notes.rs", 3, "to move"),
    ("crates/oxoffice-pptx/src/import_tables.rs", 4, "to move"),
    ("crates/oxoffice-pptx/src/masters.rs", 1, "to move"),
    ("crates/oxoffice-pptx/src/properties.rs", 2, "to move"),
    ("crates/oxoffice-pptx/src/themes.rs", 3, "to move"),
    (
        "crates/oxoffice-render-test/src/xml.rs",
        1,
        "verbatim reader",
    ),
    ("crates/oxoffice-sml/src/lib.rs", 6, "verbatim reader"),
    (
        "crates/oxoffice-xml-test/src/writing.rs",
        4,
        "verbatim reader",
    ),
];

#[test]
fn the_check_finds_a_tag_close_taken_as_two_bytes() {
    let source = concat!(
        "let a = xml[..after].ends_with(b\"/>\");\n",
        "let b = xml.get(end - 2) == Some(&b'/');\n",
        "let c = after - if empty { 2 } else { 1 };\n",
        "let d = match event { Event::Empty(_) => 2, _ => 1 };\n",
        "let e = xml[..head_end - 2].to_vec();\n",
        "let f = (reader.buffer_position() as usize).saturating_sub(2);\n",
        "opening.truncate(opening.len() - 2);\n",
        "let g = &raw[..raw.len() - 2];\n",
        "tag.splice(tag.len() - 2.., *b\">\");\n",
        "let h = tag[end - 1] == b'/';\n",
        "let i = format!(r#\"<a:x val=\"{v}\"/>\"#);\n",
        "let j = stack[stack.len() - 2];\n",
        "let k = (width - 2.0 * margin).max(0.0);\n",
        "let l = raw[cursor] == b'/';\n",
        "#[cfg(test)]\nmod tests { fn t() { xml.ends_with(\"/>\"); } }\n",
        "// A comment on \"/>\".\n",
        "let m = if double { 2 } else { 1 };\n",
        "let n = open_end.checked_sub(if empty { 2 } else { 1 });\n",
        "let o = tag.iter().rposition(|byte| *byte == b'/');\n",
        "let p = tag.iter().rposition(|byte| *byte != b'/');\n",
    );
    assert_eq!(
        self_closing_assumptions(source),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 19, 20]
    );
}

// An empty element's bytes end with `/>` only when it is written
// self-closing: `<w:b></w:b>` says the same, and source surgery that takes
// two bytes off its end writes `</w: xmlns:...p>` (#664). Where a tag ends,
// and where its attributes end, comes from `oxoffice_core::xml_element`.
#[test]
fn no_code_takes_a_tag_close_as_two_bytes() {
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
        let lines = self_closing_assumptions(&source);
        let allowed: usize = SELF_CLOSING
            .iter()
            .filter(|(file, ..)| *file == relative)
            .map(|(_, count, _)| count)
            .sum();
        if lines.len() != allowed {
            refused.push(format!(
                "{relative} assumes `/>` on {} lines ({lines:?}), {allowed} allowed",
                lines.len()
            ));
        }
    }
    assert!(
        refused.is_empty(),
        "take where a tag or its attributes end from oxoffice_core::xml_element, \
         or list code that writes an element it builds in SELF_CLOSING:\n{}",
        refused.join("\n")
    );
}
