//! The `sheetViews` element of a worksheet: the frozen panes.
//!
//! A frozen view is a `pane` and the `selection` of each pane the split
//! makes, as Excel writes them: the split counts the rows and columns that
//! stay in view, `topLeftCell` is the first cell that scrolls, and the pane
//! the cursor is in is the active one.

use crate::reference::CellRef;
use crate::xml::{Element, Node, Tag, prefix_of_name, qualified};

/// `sheetViews` with the panes frozen above and left of `at`, or, for
/// `None`, with its frozen panes removed. A missing `sheetViews` is made
/// for a freeze and stays missing otherwise. `prefix` names the elements a
/// missing one is written with.
///
/// Every `sheetView` takes the change, each in its own element: a freeze
/// replaces any pane, drops the `selection` of every pane (the cursor goes
/// to `at`, as when Excel freezes the panes at the cursor) and the view's
/// `topLeftCell`, which a split would show twice; removing the panes drops
/// the `selection` of each pane that goes. What the view says otherwise
/// (zoom, grid lines, `pivotSelection`, an extension) stays.
pub fn set_frozen_panes(
    views: Option<Element>,
    prefix: &str,
    at: Option<CellRef>,
) -> Option<Element> {
    let mut views = match (views, at) {
        (Some(views), _) => views,
        (None, None) => return None,
        (None, Some(_)) => {
            let mut views = Element::new(&qualified(prefix, "sheetViews"));
            let mut view = Element::new(&qualified(prefix, "sheetView"));
            view.tag.set("workbookViewId", Some("0"));
            views.children.push(Node::Element(view));
            views
        }
    };
    for child in &mut views.children {
        if let Node::Element(view) = child
            && view.tag.is("sheetView")
        {
            match at {
                Some(cell) => freeze(view, cell),
                None => unfreeze(view),
            }
        }
    }
    Some(views)
}

fn named(node: &Node, local: &str) -> bool {
    matches!(node, Node::Element(element) if element.tag.is(local))
}

fn leaf(prefix: &str, local: &str, attributes: &[(&str, &str)]) -> Node {
    let mut element = Element::new(&qualified(prefix, local));
    for (name, value) in attributes {
        element.tag.set(name, Some(value));
    }
    Node::Element(element)
}

/// The panes frozen above and left of `cell`.
fn freeze(view: &mut Element, cell: CellRef) {
    let prefix = prefix_of_name(&view.tag.name()).to_owned();
    let (rows, columns) = (cell.row, cell.column);
    let at = cell.to_string();
    let top_right = CellRef::new(0, columns).to_string();
    let bottom_left = CellRef::new(rows, 0).to_string();

    let mut pane = Tag::new(&qualified(&prefix, "pane"));
    if columns > 0 {
        pane.set("xSplit", Some(&columns.to_string()));
    }
    if rows > 0 {
        pane.set("ySplit", Some(&rows.to_string()));
    }
    pane.set("topLeftCell", Some(&at));
    let (active, selections): (&str, Vec<(&str, &str)>) = match (rows > 0, columns > 0) {
        (true, true) => (
            "bottomRight",
            vec![
                ("topRight", &top_right),
                ("bottomLeft", &bottom_left),
                ("bottomRight", &at),
            ],
        ),
        (false, true) => ("topRight", vec![("topRight", &at)]),
        _ => ("bottomLeft", vec![("bottomLeft", &at)]),
    };
    pane.set("activePane", Some(active));
    pane.set("state", Some("frozen"));

    view.tag.set("topLeftCell", None);
    let others: Vec<Node> = std::mem::take(&mut view.children)
        .into_iter()
        .filter(|node| !named(node, "pane") && !named(node, "selection"))
        .collect();
    let mut children = vec![Node::Element(Element {
        tag: pane,
        children: Vec::new(),
    })];
    for (pane, cell) in selections {
        children.push(leaf(
            &prefix,
            "selection",
            &[("pane", pane), ("activeCell", cell), ("sqref", cell)],
        ));
    }
    view.children = children.into_iter().chain(others).collect();
}

/// The view without its frozen panes, and the selections of the panes that
/// go; a view with no frozen pane (a split, or none) is left as it is.
fn unfreeze(view: &mut Element) {
    let frozen = view.child("pane").is_some_and(|pane| {
        matches!(
            pane.tag.get("state").as_deref(),
            Some("frozen" | "frozenSplit")
        )
    });
    if !frozen {
        return;
    }
    view.children.retain(|node| match node {
        Node::Element(element) if element.tag.is("pane") => false,
        Node::Element(element) if element.tag.is("selection") => !matches!(
            element.tag.get("pane").as_deref(),
            Some("topRight" | "bottomLeft" | "bottomRight")
        ),
        _ => true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str =
        r#"<sheetViews><sheetView tabSelected="1" workbookViewId="0"/></sheetViews>"#;

    fn frozen(views: &str, at: Option<&str>) -> Option<String> {
        let views = (!views.is_empty()).then(|| Element::parse(views.as_bytes()).unwrap());
        let at = at.map(|at| CellRef::parse(at).unwrap());
        set_frozen_panes(views, "", at).map(|views| String::from_utf8(views.to_xml()).unwrap())
    }

    #[test]
    fn a_freeze_writes_the_panes_as_excel_does_and_replaces_what_it_found() {
        let cases: [(&str, &str, &str, &str); 8] = [
            (
                "rows and columns, in a missing sheetViews",
                "",
                "B2",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0">"#,
                    r#"<pane xSplit="1" ySplit="1" topLeftCell="B2" activePane="bottomRight" state="frozen"/>"#,
                    r#"<selection pane="topRight" activeCell="B1" sqref="B1"/>"#,
                    r#"<selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#,
                    r#"<selection pane="bottomRight" activeCell="B2" sqref="B2"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "rows only, the view's attributes kept",
                VIEW,
                "A4",
                concat!(
                    r#"<sheetViews><sheetView tabSelected="1" workbookViewId="0">"#,
                    r#"<pane ySplit="3" topLeftCell="A4" activePane="bottomLeft" state="frozen"/>"#,
                    r#"<selection pane="bottomLeft" activeCell="A4" sqref="A4"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "columns only",
                VIEW,
                "C1",
                concat!(
                    r#"<sheetViews><sheetView tabSelected="1" workbookViewId="0">"#,
                    r#"<pane xSplit="2" topLeftCell="C1" activePane="topRight" state="frozen"/>"#,
                    r#"<selection pane="topRight" activeCell="C1" sqref="C1"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "LibreOffice's selection of the one pane goes, its other attributes stay",
                r#"<sheetViews><sheetView showGridLines="false" zoomScale="90" workbookViewId="0"><selection pane="topLeft" activeCell="D9" sqref="D9:E10"/></sheetView></sheetViews>"#,
                "B2",
                concat!(
                    r#"<sheetViews><sheetView showGridLines="false" zoomScale="90" workbookViewId="0">"#,
                    r#"<pane xSplit="1" ySplit="1" topLeftCell="B2" activePane="bottomRight" state="frozen"/>"#,
                    r#"<selection pane="topRight" activeCell="B1" sqref="B1"/>"#,
                    r#"<selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#,
                    r#"<selection pane="bottomRight" activeCell="B2" sqref="B2"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "a scrolled view starts at its first cell again; a pivot selection and an extension stay after the panes",
                r#"<sheetViews><sheetView topLeftCell="A40" workbookViewId="0"><selection activeCell="A41" sqref="A41"/><pivotSelection pane="topLeft"/><extLst/></sheetView></sheetViews>"#,
                "A2",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0">"#,
                    r#"<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>"#,
                    r#"<selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#,
                    r#"<pivotSelection pane="topLeft"/><extLst/></sheetView></sheetViews>"#
                ),
            ),
            (
                "another freeze replaces the pane and its selections",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0">"#,
                    r#"<pane xSplit="3" ySplit="5" topLeftCell="D6" activePane="bottomRight" state="frozen"/>"#,
                    r#"<selection pane="topRight"/><selection pane="bottomLeft"/><selection pane="bottomRight" activeCell="D9" sqref="D9"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
                "A2",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0">"#,
                    r#"<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>"#,
                    r#"<selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "a split pane is replaced as well",
                r#"<sheetViews><sheetView workbookViewId="0"><pane xSplit="2400" ySplit="1800" topLeftCell="C5" activePane="bottomRight" state="split"/></sheetView></sheetViews>"#,
                "C1",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0">"#,
                    r#"<pane xSplit="2" topLeftCell="C1" activePane="topRight" state="frozen"/>"#,
                    r#"<selection pane="topRight" activeCell="C1" sqref="C1"/>"#,
                    r#"</sheetView></sheetViews>"#
                ),
            ),
            (
                "every view of the sheet is frozen",
                r#"<sheetViews><sheetView workbookViewId="0"/><sheetView workbookViewId="1"/></sheetViews>"#,
                "A2",
                concat!(
                    r#"<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><selection pane="bottomLeft" activeCell="A2" sqref="A2"/></sheetView>"#,
                    r#"<sheetView workbookViewId="1"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><selection pane="bottomLeft" activeCell="A2" sqref="A2"/></sheetView></sheetViews>"#
                ),
            ),
        ];
        for (case, before, at, expected) in cases {
            assert_eq!(
                frozen(before, Some(at)).as_deref(),
                Some(expected),
                "{case}"
            );
        }
    }

    #[test]
    fn removing_the_panes_drops_their_selections_and_nothing_else() {
        let frozen_view = concat!(
            r#"<sheetViews><sheetView workbookViewId="0">"#,
            r#"<pane xSplit="1" ySplit="1" topLeftCell="B2" activePane="bottomRight" state="frozen"/>"#,
            r#"<selection pane="topRight" activeCell="B1" sqref="B1"/><selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#,
            r#"<selection pane="bottomRight" activeCell="B2" sqref="B2"/><selection activeCell="A1" sqref="A1"/>"#,
            r#"</sheetView></sheetViews>"#
        );
        let split = r#"<sheetViews><sheetView workbookViewId="0"><pane xSplit="2400" topLeftCell="C1" activePane="topRight" state="split"/></sheetView></sheetViews>"#;
        for (case, before, expected) in [
            (
                "a frozen view",
                frozen_view,
                Some(
                    r#"<sheetViews><sheetView workbookViewId="0"><selection activeCell="A1" sqref="A1"/></sheetView></sheetViews>"#,
                ),
            ),
            ("a view with no pane", VIEW, Some(VIEW)),
            ("a split is not frozen", split, Some(split)),
            ("no sheetViews stays none", "", None),
        ] {
            assert_eq!(frozen(before, None).as_deref(), expected, "{case}");
        }
    }
}
