//! The parts of an element read whatever its writing
//! (`oxoffice_core::xml_element`), and the check that no code searches for
//! an end tag as text instead (`specs/2026-11-25-equivalent-xml-writings.md`).

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
const ALLOWED: [(&str, &str); 4] = [
    ("crates/oxoffice-docx/src/epub.rs", "</li>"),
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

/// Each search for an end tag as text in `source`, before its tests (a
/// `#[cfg(test)]` module, at the end of a file by convention): its line and
/// the literal searched, directly or through a variable bound to it.
fn end_tag_searches(source: &str) -> Vec<(usize, String)> {
    let code = source.split("#[cfg(test)]").next().unwrap_or(source);
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
    for method in SEARCHES {
        let call = format!(".{method}(");
        for (at, _) in code.match_indices(&call) {
            let argument = &code[at + call.len()..];
            let literal = end_tag_literal(argument).or_else(|| {
                let argument = argument.trim_start().trim_start_matches('&').trim_start();
                variables.iter().find_map(|(name, literal)| {
                    argument
                        .strip_prefix(name.as_str())
                        .filter(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
                        .map(|_| literal.clone())
                })
            });
            // `windows(b"</p:sld>".len())` and the like.
            let literal = literal.or_else(|| {
                (method == "windows")
                    .then(|| end_tag_literal(argument))
                    .flatten()
            });
            if let Some(literal) = literal {
                found.push((line_of(at), literal));
            }
        }
    }
    // `window == b"</p:sld>"`.
    for (at, _) in code.match_indices("== ") {
        if let Some(literal) = end_tag_literal(&code[at + 3..]) {
            found.push((line_of(at), literal));
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
    );
    let lines: Vec<usize> = end_tag_searches(source)
        .into_iter()
        .map(|(line, _)| line)
        .collect();
    assert_eq!(lines, [1, 3, 5, 6, 7]);
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
