//! `Shapes::walk` visits every shape of a slide depth first in z-order, a
//! group before its children, each with the path that finds it again.

use oxoffice_pptx::{Emu, Presentation, ShapeKind, ShapeOwner};

const INCH: Emu = Emu(914_400);

fn add_box(prs: &mut Presentation, group: &[usize], name: &str) {
    let mut shapes = prs.shapes_mut(0, group).expect("shape collection");
    let mut shape = shapes.add_textbox(INCH, INCH, INCH, INCH).expect("textbox");
    shape.set_name(name).unwrap();
    shape.set_text(name).unwrap();
}

fn add_group(prs: &mut Presentation, group: &[usize], name: &str) {
    let mut shapes = prs.shapes_mut(0, group).expect("shape collection");
    shapes
        .add_group_shape()
        .expect("group")
        .set_name(name)
        .unwrap();
}

/// A new slide, its `n` layout placeholders first, then
/// `a`, `outer` [`b`, `inner` [`c`], `d`], `e`.
fn nested() -> (Presentation, usize) {
    let mut prs = Presentation::new().unwrap();
    prs.add_slide(0).unwrap();
    let n = prs.slide(0).unwrap().shapes().len();
    add_box(&mut prs, &[], "a");
    add_group(&mut prs, &[], "outer");
    add_box(&mut prs, &[n + 1], "b");
    add_group(&mut prs, &[n + 1], "inner");
    add_box(&mut prs, &[n + 1, 1], "c");
    add_box(&mut prs, &[n + 1], "d");
    add_box(&mut prs, &[], "e");
    (prs, n)
}

fn named(walk: oxoffice_pptx::ShapeWalk<'_>) -> Vec<(Vec<usize>, String)> {
    walk.filter_map(|(path, shape)| Some((path, shape.non_visual_name()?)))
        .collect()
}

fn expected(rows: &[(&[usize], &str)]) -> Vec<(Vec<usize>, String)> {
    rows.iter()
        .map(|(path, name)| (path.to_vec(), (*name).to_owned()))
        .collect()
}

#[test]
fn walk_visits_a_group_before_its_children_in_z_order() {
    let (prs, n) = nested();
    let slide = prs.slide(0).unwrap();
    let walked = named(slide.shapes().walk());
    let (o, e) = (n + 1, n + 2);
    assert_eq!(
        walked[n..],
        expected(&[
            (&[n], "a"),
            (&[o], "outer"),
            (&[o, 0], "b"),
            (&[o, 1], "inner"),
            (&[o, 1, 0], "c"),
            (&[o, 2], "d"),
            (&[e], "e"),
        ])
    );
    assert!(
        walked[..n]
            .iter()
            .enumerate()
            .all(|(i, (path, _))| path == &[i])
    );
    let texts: Vec<_> = slide
        .shapes()
        .walk()
        .filter_map(|(_, shape)| shape.text())
        .filter(|text| text.len() == 1)
        .collect();
    assert_eq!(texts, ["a", "b", "c", "d", "e"]);
}

#[test]
fn every_walked_path_finds_its_shape_again() {
    let (prs, _) = nested();
    let walked: Vec<_> = prs.slide(0).unwrap().shapes().walk().collect();
    assert_eq!(walked.len(), prs.slide(0).unwrap().shapes().len() + 4);
    for (path, shape) in walked {
        assert!(
            prs.part_shape_at(ShapeOwner::Slide(0), &path).unwrap() == shape,
            "{path:?}"
        );
    }
    let owned: Vec<_> = prs
        .part_shapes(ShapeOwner::Slide(0))
        .unwrap()
        .walk()
        .collect();
    assert_eq!(owned.len(), prs.slide(0).unwrap().shapes().walk().count());
}

#[test]
fn walk_starts_where_the_shapes_iterator_is() {
    let (prs, n) = nested();
    let slide = prs.slide(0).unwrap();
    let (o, e) = (n + 1, n + 2);
    let mut shapes = slide.shapes();
    shapes.nth(n);
    assert_eq!(
        named(shapes.walk()),
        expected(&[
            (&[o], "outer"),
            (&[o, 0], "b"),
            (&[o, 1], "inner"),
            (&[o, 1, 0], "c"),
            (&[o, 2], "d"),
            (&[e], "e"),
        ])
    );
    let outer = slide.shape(o).unwrap();
    assert_eq!(
        named(outer.children().walk()),
        expected(&[(&[0], "b"), (&[1], "inner"), (&[1, 0], "c"), (&[2], "d")])
    );
    assert_eq!(slide.shape(n).unwrap().children().walk().count(), 0);
}

/// A shape for the shape tree of a slide, with no text.
fn bare_shape(id: u32, name: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp>"#
    )
}

/// The corpus deck with, at the end of its first slide, a compatibility
/// block whose choice holds one shape and whose fallback holds two.
fn deck_with_alternate_content() -> Presentation {
    use std::io::{Cursor, Read, Write};

    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files/powerpoint/deck.pptx");
    let mut source = zip::ZipArchive::new(Cursor::new(std::fs::read(corpus).unwrap())).unwrap();
    let block = format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14">{}</mc:Choice><mc:Fallback>{}{}</mc:Fallback></mc:AlternateContent>"#,
        bare_shape(900, "choice"),
        bare_shape(901, "fallback a"),
        bare_shape(902, "fallback b"),
    );
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == "ppt/slides/slide1.xml" {
            let xml = String::from_utf8(bytes).unwrap();
            assert!(xml.contains("</p:spTree>"), "{name} has a shape tree");
            bytes = xml
                .replace("</p:spTree>", &format!("{block}</p:spTree>"))
                .into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::DEFAULT)
            .unwrap();
        out.write_all(&bytes).unwrap();
    }
    Presentation::from_bytes(&out.finish().unwrap().into_inner()).unwrap()
}

#[test]
fn walk_enters_the_fallback_of_alternate_content() {
    let prs = deck_with_alternate_content();
    let slide = prs.slide(0).unwrap();
    let n = slide.shapes().len() - 1;
    let walked: Vec<_> = slide.shapes().walk().collect();
    let kinds: Vec<_> = walked[n..]
        .iter()
        .map(|(path, shape)| (path.clone(), shape.kind(), shape.non_visual_name()))
        .collect();
    assert_eq!(
        kinds,
        [
            (vec![n], ShapeKind::AlternateContent, None),
            (vec![n, 0], ShapeKind::Shape, Some("fallback a".to_owned())),
            (vec![n, 1], ShapeKind::Shape, Some("fallback b".to_owned())),
        ]
    );
    for (path, shape) in walked {
        assert!(prs.part_shape_at(ShapeOwner::Slide(0), &path).unwrap() == shape);
    }
}
