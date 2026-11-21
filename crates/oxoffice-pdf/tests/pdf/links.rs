//! Link annotations: tagged under `/Link` in tagged output, so assistive
//! technology finds them in reading order.

use lopdf::{Dictionary, Object, ObjectId};
use oxoffice_layout::{LayoutResult, Point, PositionedElement, Rect};

use crate::forms::{
    field, fragment, label, line, paragraph, paragraph_kids, pdf, tagged, text_kind, with_fields,
};
use crate::support::{Pdf, layout, run};

const URL: &str = "https://example.com/report";

fn link(x: f64) -> PositionedElement {
    PositionedElement::LinkAnnotation {
        rect: Rect {
            x,
            y: 100.0,
            width: 60.0,
            height: 14.0,
        },
        url: URL.to_owned(),
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
