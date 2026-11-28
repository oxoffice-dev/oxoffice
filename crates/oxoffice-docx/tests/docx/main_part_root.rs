//! A body edit keeps the declaration and root start tag a producer wrote for
//! the main part, when the written root differs only in attribute order.

use std::io::Read;

/// The main part's bytes up to the end of its root start tag.
fn root_prefix(saved: &[u8]) -> String {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(saved)).expect("a package");
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .expect("a main part")
        .read_to_string(&mut xml)
        .expect("UTF-8");
    let root = xml.find("<w:document").expect("a root");
    xml[..root + xml[root..].find('>').expect("a root end") + 1].to_owned()
}

/// A new document whose main part root is rewritten by `rewrite`.
fn doc_with_root(rewrite: impl FnOnce(&str) -> String) -> oxoffice_docx::Document {
    let bytes = oxoffice_docx::Document::new().to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    package.set_part(&part, rewrite(&xml).into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    oxoffice_docx::Document::from_bytes(&out.into_inner()).unwrap()
}

/// The root with its attributes in reverse order, after a declaration and a
/// line break.
fn reversed_root(xml: &str) -> String {
    let start = xml.find("<w:document ").unwrap();
    let end = start + xml[start..].find('>').unwrap();
    let mut attributes = xml[start + "<w:document ".len()..end]
        .split(' ')
        .collect::<Vec<_>>();
    assert!(attributes.len() > 1 && attributes.iter().all(|a| a.contains("=\"")));
    attributes.reverse();
    format!(
        "{}\n<w:document {}{}",
        &xml[..start],
        attributes.join(" "),
        &xml[end..]
    )
}

#[test]
fn a_root_that_differs_only_in_attribute_order_is_kept() {
    let mut doc = doc_with_root(reversed_root);
    let opened = root_prefix(&doc.to_bytes().unwrap());
    doc.add_paragraph("added");
    assert_eq!(root_prefix(&doc.to_bytes().unwrap()), opened);
}

/// A root default namespace, which the writer writes back as declared: the
/// roots differ only in order, and the producer's stays.
#[test]
fn a_root_with_a_default_namespace_is_kept() {
    let mut doc = doc_with_root(|xml| {
        reversed_root(xml).replacen("<w:document ", "<w:document xmlns=\"urn:x\" ", 1)
    });
    let opened = root_prefix(&doc.to_bytes().unwrap());
    doc.add_paragraph("added");
    assert_eq!(root_prefix(&doc.to_bytes().unwrap()), opened);
}
