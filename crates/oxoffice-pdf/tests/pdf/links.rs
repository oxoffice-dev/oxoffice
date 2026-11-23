//! Link annotations: tagged under `/Link` in tagged output, so assistive
//! technology finds them in reading order.

use lopdf::{Dictionary, Object, ObjectId};
use oxoffice_layout::{GroupElement, LayoutResult, Point, PositionedElement, Rect, Transform};

use crate::forms::{
    field, fragment, label, line, paragraph, paragraph_kids, pdf, tagged, text_kind, with_fields,
};
use crate::support::{Pdf, layout, run};

const URL: &str = "https://example.com/report";

fn link(x: f64) -> PositionedElement {
    link_to(x, URL)
}

fn link_to(x: f64, url: &str) -> PositionedElement {
    PositionedElement::LinkAnnotation {
        rect: Rect {
            x,
            y: 100.0,
            width: 60.0,
            height: 14.0,
        },
        url: url.to_owned(),
    }
}

fn destination(name: &str, y: f64) -> PositionedElement {
    PositionedElement::Destination {
        name: name.to_owned(),
        point: Point { x: 72.0, y },
    }
}

/// The link annotations of a page, with their references.
fn links(read: &Pdf, page: usize) -> Vec<(ObjectId, &Dictionary)> {
    let page = read
        .doc
        .get_object(read.page_ids()[page])
        .unwrap()
        .as_dict()
        .unwrap();
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    read.resolve(annots)
        .as_array()
        .unwrap()
        .iter()
        .map(|annot| annot.as_reference().unwrap())
        .map(|id| (id, read.doc.get_object(id).unwrap().as_dict().unwrap()))
        .filter(|(_, annot)| annot.get(b"Subtype").unwrap().as_name().unwrap() == b"Link")
        .collect()
}

/// The structure element the parent tree gives for `key`.
fn parent_tree_entry(read: &Pdf, key: i64) -> &Dictionary {
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let nums = read
        .dict(root, b"ParentTree")
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap();
    let index = nums
        .chunks(2)
        .position(|pair| pair[0].as_i64().unwrap() == key)
        .unwrap_or_else(|| panic!("no parent tree entry for key {key}"));
    read.resolve(&nums[index * 2 + 1]).as_dict().unwrap()
}

/// The one object reference kid of a structure element.
fn object_ref(read: &Pdf, element: &Dictionary) -> (ObjectId, ObjectId) {
    let objr = match read.resolve(element.get(b"K").unwrap()) {
        Object::Array(kids) => {
            assert_eq!(kids.len(), 1, "one kid");
            read.resolve(&kids[0]).as_dict().unwrap()
        }
        other => other.as_dict().unwrap(),
    };
    assert_eq!(objr.get(b"Type").unwrap().as_name().unwrap(), b"OBJR");
    (
        objr.get(b"Obj").unwrap().as_reference().unwrap(),
        objr.get(b"Pg").unwrap().as_reference().unwrap(),
    )
}

fn next_key(read: &Pdf) -> i64 {
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    root.get(b"ParentTreeNextKey").unwrap().as_i64().unwrap()
}

#[test]
fn a_tagged_link_sits_in_a_link_element_of_its_paragraph() {
    let layout = tagged(layout(vec![vec![paragraph(vec![
        run("Read the report", Point { x: 72.0, y: 112.0 }, 11.0),
        link(100.0),
    ])]]));
    let read = pdf(&layout);
    assert_eq!(paragraph_kids(&read), ["MCR", "Link"]);

    let links = links(&read, 0);
    assert_eq!(links.len(), 1);
    let (annotation_id, annotation) = links[0];
    let key = annotation.get(b"StructParent").unwrap().as_i64().unwrap();
    assert_eq!(key, 1, "keys after the one page's");
    assert_eq!(next_key(&read), 2);
    let element = parent_tree_entry(&read, key);
    assert_eq!(element.get(b"S").unwrap().as_name().unwrap(), b"Link");
    assert_eq!(
        object_ref(&read, element),
        (annotation_id, read.page_ids()[0])
    );
    assert_eq!(
        annotation.get(b"Contents").unwrap().as_str().unwrap(),
        URL.as_bytes(),
        "the link is described by its target"
    );

    let page = read
        .doc
        .get_object(read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(page.get(b"Tabs").unwrap().as_name().unwrap(), b"S");
}

#[test]
fn links_take_their_place_in_reading_order_after_the_widgets_keys() {
    // Two pages: a link before a field on the first, a link inside a field's
    // drawing and one outside any marked content on the second.
    let mut field_with_link = fragment(1, line(110.0, 100.0, 200.0), "Jane");
    if let PositionedElement::FormField(fragment) = &mut field_with_link {
        fragment.children.push(link(110.0));
    }
    let layout: LayoutResult = tagged(with_fields(
        layout(vec![
            vec![paragraph(vec![
                label(),
                link(72.0),
                fragment(0, line(200.0, 100.0, 100.0), "Doe"),
            ])],
            vec![paragraph(vec![label(), field_with_link]), link(300.0)],
        ]),
        vec![
            field("last", text_kind("Doe")),
            field("first", text_kind("Jane")),
        ],
    ));
    let read = pdf(&layout);
    assert_eq!(
        paragraph_kids(&read),
        ["MCR", "Link", "Form", "MCR", "Form", "Link"]
    );
    // Two pages and two widgets come first, then the three links.
    assert_eq!(next_key(&read), 7);
    let mut keys = Vec::new();
    for page in 0..2 {
        for (id, annotation) in links(&read, page) {
            let key = annotation.get(b"StructParent").unwrap().as_i64().unwrap();
            let element = parent_tree_entry(&read, key);
            assert_eq!(element.get(b"S").unwrap().as_name().unwrap(), b"Link");
            assert_eq!(object_ref(&read, element), (id, read.page_ids()[page]));
            keys.push(key);
        }
    }
    assert_eq!(keys, [4, 5, 6]);

    // The loose link of the second page sits under the document element.
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let document = read.resolve(root.get(b"K").unwrap()).as_dict().unwrap();
    let roles: Vec<Vec<u8>> = document
        .get(b"K")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|kid| {
            let kid = read.resolve(kid).as_dict().unwrap();
            kid.get(b"S").unwrap().as_name().unwrap().to_vec()
        })
        .collect();
    assert_eq!(roles, [b"P".to_vec(), b"Link".to_vec()]);
}

#[test]
fn an_untagged_link_carries_no_structure_key() {
    let layout = layout(vec![vec![
        run("Read the report", Point { x: 72.0, y: 112.0 }, 11.0),
        link(100.0),
    ]]);
    let read = pdf(&layout);
    let links = links(&read, 0);
    assert_eq!(links.len(), 1);
    assert!(links[0].1.get(b"StructParent").is_err());
    assert!(read.catalog().get(b"StructTreeRoot").is_err());
}

#[test]
fn an_internal_link_goes_to_the_first_destination_of_its_name() {
    let layout = tagged(layout(vec![
        vec![paragraph(vec![
            run("See the results", Point { x: 72.0, y: 112.0 }, 11.0),
            // A skipped link first, so the kept one's rank counts it.
            link_to(200.0, "#nowhere"),
            link_to(100.0, "#results"),
        ])],
        // The destination sits in a group moved down 50 points; a later one
        // of the same name loses.
        vec![
            PositionedElement::Group(GroupElement {
                transform: Transform {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: 50.0,
                },
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children: vec![destination("results", 250.0)],
            }),
            destination("results", 600.0),
        ],
    ]));
    let read = pdf(&layout);
    let links = links(&read, 0);
    assert_eq!(
        links.len(),
        1,
        "a link to no destination gets no annotation"
    );
    let (id, annotation) = links[0];
    let action = read
        .resolve(annotation.get(b"A").unwrap())
        .as_dict()
        .unwrap();
    assert_eq!(action.get(b"S").unwrap().as_name().unwrap(), b"GoTo");
    let destination = read.resolve(action.get(b"D").unwrap()).as_array().unwrap();
    assert_eq!(destination[0].as_reference().unwrap(), read.page_ids()[1]);
    assert_eq!(destination[1].as_name().unwrap(), b"XYZ");
    let top = destination[3].as_float().unwrap();
    assert!(
        (f64::from(top) - (crate::support::PAGE_HEIGHT - 300.0)).abs() < 0.01,
        "{top}"
    );
    // An internal link is tagged like any other.
    let key = annotation.get(b"StructParent").unwrap().as_i64().unwrap();
    assert_eq!(object_ref(&read, parent_tree_entry(&read, key)).0, id);
}

/// A link's rectangle `width` wide on the line whose top is `top`, a
/// 14-point line.
fn link_at(x: f64, top: f64, width: f64, url: &str) -> PositionedElement {
    PositionedElement::LinkAnnotation {
        rect: Rect {
            x,
            y: top,
            width,
            height: 14.0,
        },
        url: url.to_owned(),
    }
}

/// One element in a marked content sequence of the paragraph, as the layout
/// marks each run of a paragraph.
fn marked(element: PositionedElement) -> PositionedElement {
    paragraph(vec![element])
}

/// A run whose baseline sits on the line whose top is `top`.
fn word(text: &str, x: f64, top: f64) -> PositionedElement {
    run(text, Point { x, y: top + 12.0 }, 11.0)
}

/// The kids of a structure element: `MCR` for marked content, `OBJR` for
/// an object reference.
fn kinds(read: &Pdf, element: &Dictionary) -> Vec<String> {
    let kids = match read.resolve(element.get(b"K").unwrap()) {
        Object::Array(kids) => kids.clone(),
        other => vec![other.clone()],
    };
    kids.iter()
        .map(|kid| match read.resolve(kid) {
            Object::Integer(_) => "MCR".to_owned(),
            Object::Dictionary(dict) if dict.get(b"MCID").is_ok() => "MCR".to_owned(),
            Object::Dictionary(dict) => {
                String::from_utf8(dict.get(b"Type").unwrap().as_name().unwrap().to_vec()).unwrap()
            }
            other => panic!("unexpected kid {other:?}"),
        })
        .collect()
}

/// Each link annotation of a page: its structure element and `/Contents`.
fn tagged_links(read: &Pdf, page: usize) -> Vec<(&Dictionary, String)> {
    links(read, page)
        .into_iter()
        .map(|(_, annotation)| {
            let key = annotation.get(b"StructParent").unwrap().as_i64().unwrap();
            let contents = annotation.get(b"Contents").unwrap().as_str().unwrap();
            (
                parent_tree_entry(read, key),
                String::from_utf8_lossy(contents).into_owned(),
            )
        })
        .collect()
}

/// The structure element the page's parent tree entry gives for `mcid`.
fn element_of_mcid(read: &Pdf, page: usize, mcid: usize) -> &Dictionary {
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let nums = read
        .dict(root, b"ParentTree")
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap();
    let index = nums
        .chunks(2)
        .position(|pair| pair[0].as_i64().unwrap() == page as i64)
        .unwrap();
    let array = read.resolve(&nums[index * 2 + 1]).as_array().unwrap();
    read.resolve(&array[mcid]).as_dict().unwrap()
}

// The layout starts the run after a link where the link's rectangle ends,
// so each test puts it there.

#[test]
fn a_tagged_link_holds_its_text_which_leaves_the_paragraph() {
    let layout = tagged(layout(vec![vec![
        marked(word("Read ", 72.0, 100.0)),
        marked(word("the report", 100.0, 100.0)),
        marked(link_at(100.0, 100.0, 60.0, URL)),
        marked(word(" today", 160.0, 100.0)),
    ]]));
    let read = pdf(&layout);
    assert_eq!(paragraph_kids(&read), ["MCR", "Link", "MCR"]);
    let links = tagged_links(&read, 0);
    assert_eq!(links.len(), 1);
    let (element, contents) = &links[0];
    assert_eq!(kinds(&read, element), ["MCR", "OBJR"]);
    assert_eq!(contents, "the report", "the link is described by its text");
    let of_text = element_of_mcid(&read, 0, 1);
    assert_eq!(of_text.get(b"S").unwrap().as_name().unwrap(), b"Link");
    assert!(std::ptr::eq(of_text, *element));
    for mcid in [0, 2] {
        let element = element_of_mcid(&read, 0, mcid);
        assert_eq!(element.get(b"S").unwrap().as_name().unwrap(), b"P");
    }
}

#[test]
fn a_link_over_two_runs_and_two_lines_is_one_link_element() {
    let layout = tagged(layout(vec![vec![
        marked(word("Read ", 72.0, 100.0)),
        marked(word("the ", 100.0, 100.0)),
        marked(link_at(100.0, 100.0, 30.0, URL)),
        marked(word("annual", 130.0, 100.0)),
        marked(link_at(130.0, 100.0, 40.0, URL)),
        marked(word("report", 72.0, 114.0)),
        marked(link_at(72.0, 114.0, 40.0, URL)),
        marked(word(" now", 112.0, 114.0)),
    ]]));
    let read = pdf(&layout);
    assert_eq!(paragraph_kids(&read), ["MCR", "Link", "MCR"]);
    let links = tagged_links(&read, 0);
    assert_eq!(links.len(), 3);
    for (element, contents) in &links {
        assert!(std::ptr::eq(*element, links[0].0), "one element");
        assert_eq!(contents, "the annual report");
    }
    assert_eq!(
        kinds(&read, links[0].0),
        ["MCR", "OBJR", "MCR", "OBJR", "MCR", "OBJR"]
    );
}

#[test]
fn a_subscript_inside_a_link_reads_without_a_space() {
    let layout = tagged(layout(vec![vec![
        marked(word("H", 100.0, 100.0)),
        marked(run("2", Point { x: 108.0, y: 113.5 }, 7.0)),
        marked(word("O", 112.0, 100.0)),
        marked(link_at(100.0, 100.0, 20.0, URL)),
    ]]));
    let read = pdf(&layout);
    assert_eq!(tagged_links(&read, 0)[0].1, "H2O");
}

#[test]
fn links_side_by_side_to_two_targets_or_apart_in_the_text_stay_two_links() {
    for (second, between) in [("https://example.com/other", false), (URL, true)] {
        let mut elements = vec![
            marked(word("the report", 100.0, 100.0)),
            marked(link_at(100.0, 100.0, 60.0, URL)),
        ];
        let annex = if between {
            elements.push(marked(word(" and ", 160.0, 100.0)));
            200.0
        } else {
            160.0
        };
        elements.push(marked(word("the annex", annex, 100.0)));
        elements.push(marked(link_at(annex, 100.0, 60.0, second)));
        let read = pdf(&tagged(layout(vec![elements])));
        let links = tagged_links(&read, 0);
        assert_eq!(links.len(), 2);
        assert!(!std::ptr::eq(links[0].0, links[1].0), "{second}");
        assert_eq!(links[0].1, "the report");
        assert_eq!(links[1].1, "the annex");
        let kids = if between {
            vec!["Link", "MCR", "Link"]
        } else {
            vec!["Link", "Link"]
        };
        assert_eq!(paragraph_kids(&read), kids);
    }
}

#[test]
fn links_outside_any_structure_element_stay_one_link_each() {
    // A footer link repeated on every page: no paragraph element holds it.
    let footer = || link_at(72.0, 760.0, 60.0, URL);
    let layout = tagged(layout(vec![
        vec![paragraph(vec![word("Body", 72.0, 100.0)]), footer()],
        vec![footer()],
    ]));
    let read = pdf(&layout);
    let first = tagged_links(&read, 0);
    let second = tagged_links(&read, 1);
    assert!(!std::ptr::eq(first[0].0, second[0].0));
    assert_eq!(kinds(&read, first[0].0), ["OBJR"]);
}

#[test]
fn a_run_that_starts_outside_the_link_stays_in_the_paragraph() {
    // One marked sequence with a run inside the link and one outside: its
    // identifier cannot be split, so it stays the paragraph's.
    let layout = tagged(layout(vec![vec![
        paragraph(vec![
            word("the report", 100.0, 100.0),
            word(" today", 160.0, 100.0),
        ]),
        marked(link_at(100.0, 100.0, 60.0, URL)),
    ]]));
    let read = pdf(&layout);
    assert_eq!(paragraph_kids(&read), ["MCR", "Link"]);
    let links = tagged_links(&read, 0);
    assert_eq!(kinds(&read, links[0].0), ["OBJR"]);
    assert_eq!(
        links[0].1, URL,
        "a link without text is described by its target"
    );
}
