//! Deterministic allocation and serialization of tagged PDF structure.

use std::collections::{BTreeMap, HashMap};

use oxoffice_layout::{
    DocumentStructure, FormFieldId, PageFrame, PositionedElement, StructureId, StructureRole, walk,
};
use pdf_writer::types::{StructRole, TableHeaderScope};
use pdf_writer::{Finish, Name, Pdf, Ref, TextStr};

#[derive(Debug, Clone, Copy)]
struct MarkOccurrence {
    page_index: usize,
    mcid: i32,
}

#[derive(Debug, Clone, Copy)]
enum StructureKid {
    Element(Ref),
    MarkedContent(MarkOccurrence),
}

/// Where a form field fragment sits among a page's marked content.
#[derive(Debug, Clone, Copy)]
struct FragmentMark {
    field: FormFieldId,
    page_index: usize,
    /// Marked content sequences the page opens before the fragment.
    mcid_before: i32,
    /// The fragment's rank among the page's fragments, which orders
    /// fragments with no marked content between them.
    sequence: u32,
    /// The structure element of the marked content around the fragment.
    parent: Option<StructureId>,
}

/// Where a link annotation sits among a page's marked content.
#[derive(Debug, Clone, Copy)]
struct LinkMark {
    /// Marked content sequences the page opens before the link.
    mcid_before: i32,
    /// The link's rank among the page's fragments and links.
    sequence: u32,
    /// The structure element of the marked content around the link.
    parent: Option<StructureId>,
}

/// A form field widget to tag: its `/Form` element and its parent tree key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TaggedWidget {
    pub(crate) field: FormFieldId,
    pub(crate) page_index: usize,
    pub(crate) widget: Ref,
    pub(crate) element: Ref,
    pub(crate) key: i32,
}

/// A link annotation to tag: its `/Link` element and its parent tree key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TaggedLink {
    pub(crate) page_index: usize,
    /// The link's rank among the page's link annotations, in drawing order.
    pub(crate) ordinal: usize,
    pub(crate) annotation: Ref,
    pub(crate) element: Ref,
    pub(crate) key: i32,
}

/// Where each annotation's element goes: the kids of each parent, and each
/// annotation's parent by its parent tree key.
#[derive(Default)]
struct AnnotationPlacement {
    kids: BTreeMap<StructureId, Vec<AnnotationKid>>,
    parent: BTreeMap<i32, StructureId>,
}

/// Where a kid shows in the page's reading order: its page, the marked
/// content it is or precedes, and among fragments before that marked content
/// their rank. Marked content ranks after the fragments that precede it.
type ReadingOrder = (usize, i32, u32);

/// A widget's `/Form` or a link's `/Link` element placed under its parent
/// element.
#[derive(Debug, Clone, Copy)]
struct AnnotationKid {
    order: ReadingOrder,
    element: Ref,
}

pub(crate) struct PreparedStructure<'a> {
    structure: &'a DocumentStructure,
    pub(crate) root_ref: Ref,
    node_refs: BTreeMap<StructureId, Ref>,
    list_body_refs: BTreeMap<StructureId, Ref>,
    page_parent_refs: Vec<Ref>,
    occurrences: BTreeMap<StructureId, Vec<MarkOccurrence>>,
    fragments: Vec<FragmentMark>,
    /// Each link annotation by its page and its rank on the page.
    links: HashMap<(usize, usize), LinkMark>,
}

impl<'a> PreparedStructure<'a> {
    pub(crate) fn new(
        structure: &'a DocumentStructure,
        pages: &[std::sync::Arc<PageFrame>],
        alloc: &mut impl FnMut() -> Ref,
    ) -> Option<Self> {
        if !valid_structure(structure, pages) {
            return None;
        }
        let root_ref = alloc();
        let node_refs = structure
            .nodes
            .iter()
            .map(|node| (node.id, alloc()))
            .collect();
        let list_body_refs = structure
            .nodes
            .iter()
            .filter(|node| node.role == StructureRole::ListItem)
            .map(|node| (node.id, alloc()))
            .collect();
        let page_parent_refs = pages.iter().map(|_| alloc()).collect();
        let mut occurrences = BTreeMap::new();
        let mut fragments = Vec::new();
        let mut links = HashMap::new();
        for (page_index, page) in pages.iter().enumerate() {
            let mut marks = Marks {
                page_index,
                next_mcid: 0,
                next_fragment: 0,
                next_link: 0,
                occurrences: &mut occurrences,
                fragments: &mut fragments,
                links: &mut links,
            };
            marks.collect(&page.elements, None);
        }
        Some(Self {
            structure,
            root_ref,
            node_refs,
            list_body_refs,
            page_parent_refs,
            occurrences,
            fragments,
            links,
        })
    }

    pub(crate) fn node_ref(&self, id: StructureId) -> Option<Ref> {
        self.node_refs.get(&id).copied()
    }

    pub(crate) fn role_name(&self, id: StructureId) -> Option<&'static [u8]> {
        self.structure.node(id).map(|node| role_name(node.role))
    }

    /// Each widget's `/Form` element goes under the element of the marked
    /// content around its field's first fragment on the widget's page, and
    /// each link's `/Link` element under the element of the marked content
    /// around the link, else under the document element, ordered by where
    /// the fragment or the link sits.
    fn annotation_kids(
        &self,
        widgets: &[TaggedWidget],
        links: &[TaggedLink],
    ) -> AnnotationPlacement {
        let mut first: HashMap<(FormFieldId, usize), &FragmentMark> = HashMap::new();
        for mark in &self.fragments {
            first.entry((mark.field, mark.page_index)).or_insert(mark);
        }
        let mut placement = AnnotationPlacement::default();
        for widget in widgets {
            let mark = first.get(&(widget.field, widget.page_index)).copied();
            let parent = mark
                .and_then(|mark| mark.parent)
                .filter(|parent| self.node_refs.contains_key(parent))
                .unwrap_or(self.structure.root);
            let order = mark.map_or((widget.page_index, i32::MAX, u32::MAX), |mark| {
                (mark.page_index, mark.mcid_before, mark.sequence)
            });
            placement
                .kids
                .entry(parent)
                .or_default()
                .push(AnnotationKid {
                    order,
                    element: widget.element,
                });
            placement.parent.insert(widget.key, parent);
        }
        for link in links {
            let mark = self.links.get(&(link.page_index, link.ordinal));
            let parent = mark
                .and_then(|mark| mark.parent)
                .filter(|parent| self.node_refs.contains_key(parent))
                .unwrap_or(self.structure.root);
            let order = mark.map_or((link.page_index, i32::MAX, u32::MAX), |mark| {
                (link.page_index, mark.mcid_before, mark.sequence)
            });
            placement
                .kids
                .entry(parent)
                .or_default()
                .push(AnnotationKid {
                    order,
                    element: link.element,
                });
            placement.parent.insert(link.key, parent);
        }
        placement
    }

    pub(crate) fn write(
        &self,
        pdf: &mut Pdf,
        page_ids: &[Ref],
        widgets: &[TaggedWidget],
        links: &[TaggedLink],
    ) {
        let Some(document_ref) = self.node_ref(self.structure.root) else {
            return;
        };
        let placement = self.annotation_kids(widgets, links);
        let form_kids = &placement.kids;

        {
            let mut root = pdf.indirect(self.root_ref).dict();
            root.pair(Name(b"Type"), Name(b"StructTreeRoot"));
            root.pair(Name(b"K"), document_ref);
            root.pair(
                Name(b"ParentTreeNextKey"),
                (self.page_parent_refs.len() + widgets.len() + links.len()) as i32,
            );
            let mut parent_tree = root.insert(Name(b"ParentTree")).dict();
            let mut nums = parent_tree.insert(Name(b"Nums")).array();
            for (page_index, parent_ref) in self.page_parent_refs.iter().enumerate() {
                nums.item(page_index as i32);
                nums.item(*parent_ref);
            }
            for widget in widgets {
                nums.item(widget.key);
                nums.item(widget.element);
            }
            for link in links {
                nums.item(link.key);
                nums.item(link.element);
            }
            nums.finish();
            parent_tree.finish();
            root.finish();
        }

        let parents = parent_map(self.structure);
        for node in &self.structure.nodes {
            let Some(node_ref) = self.node_ref(node.id) else {
                continue;
            };
            let parent_ref = parents.get(&node.id).map_or(self.root_ref, |parent| {
                self.list_body_refs
                    .get(parent)
                    .copied()
                    .or_else(|| self.node_ref(*parent))
                    .unwrap_or(self.root_ref)
            });
            let mut element = pdf.struct_element(node_ref);
            element.kind(pdf_role(node.role));
            element.parent(parent_ref);
            if let Some(alt) = node.alternate_text.as_deref() {
                element.alt(TextStr(alt));
            }
            if node.role == StructureRole::TableHeaderCell {
                let mut attributes = element.attributes();
                attributes.push().table().scope(TableHeaderScope::Column);
                attributes.finish();
            }
            let marks = self.occurrences.get(&node.id);
            let forms = form_kids.get(&node.id).map_or(&[][..], Vec::as_slice);
            if !node.children.is_empty()
                || marks.is_some_and(|marks| !marks.is_empty())
                || !forms.is_empty()
            {
                let mut children = element.children();
                if node.role == StructureRole::ListItem {
                    if let Some(body_ref) = self.list_body_refs.get(&node.id) {
                        children.struct_element(*body_ref);
                    }
                } else {
                    for kid in self.ordered_kids(node.id, form_kids) {
                        match kid {
                            StructureKid::Element(reference) => {
                                children.struct_element(reference);
                            }
                            StructureKid::MarkedContent(mark) => {
                                let Some(page_ref) = page_ids.get(mark.page_index).copied() else {
                                    continue;
                                };
                                children
                                    .marked_content_ref()
                                    .page(page_ref)
                                    .marked_content_id(mark.mcid);
                            }
                        }
                    }
                }
                if node.role == StructureRole::ListItem {
                    for mark in marks.into_iter().flatten() {
                        let Some(page_ref) = page_ids.get(mark.page_index).copied() else {
                            continue;
                        };
                        children
                            .marked_content_ref()
                            .page(page_ref)
                            .marked_content_id(mark.mcid);
                    }
                }
                children.finish();
            }
            element.finish();

            if let Some(body_ref) = self.list_body_refs.get(&node.id) {
                let mut body = pdf.struct_element(*body_ref);
                body.kind(StructRole::LBody);
                body.parent(node_ref);
                if !node.children.is_empty() || !forms.is_empty() {
                    let mut children = body.children();
                    for child in &node.children {
                        if let Some(child_ref) = self.node_ref(*child) {
                            children.struct_element(child_ref);
                        }
                    }
                    for form in forms {
                        children.struct_element(form.element);
                    }
                    children.finish();
                }
                body.finish();
            }
        }

        let annotations = widgets
            .iter()
            .map(|widget| {
                (
                    StructRole::Form,
                    widget.key,
                    widget.element,
                    widget.widget,
                    widget.page_index,
                )
            })
            .chain(links.iter().map(|link| {
                (
                    StructRole::Link,
                    link.key,
                    link.element,
                    link.annotation,
                    link.page_index,
                )
            }));
        for (role, key, element_ref, annotation, page_index) in annotations {
            let parent = placement
                .parent
                .get(&key)
                .copied()
                .unwrap_or(self.structure.root);
            let parent_ref = self
                .list_body_refs
                .get(&parent)
                .copied()
                .or_else(|| self.node_ref(parent))
                .unwrap_or(document_ref);
            let mut element = pdf.struct_element(element_ref);
            element.kind(role);
            element.parent(parent_ref);
            if let Some(page_ref) = page_ids.get(page_index).copied() {
                element
                    .children()
                    .object_ref()
                    .object(annotation)
                    .page(page_ref);
            }
            element.finish();
        }

        for (page_index, parent_ref) in self.page_parent_refs.iter().enumerate() {
            let mut by_mcid = Vec::new();
            for (id, marks) in &self.occurrences {
                for mark in marks.iter().filter(|mark| mark.page_index == page_index) {
                    if let Some(reference) = self.node_ref(*id) {
                        by_mcid.push((mark.mcid, reference));
                    }
                }
            }
            by_mcid.sort_unstable_by_key(|(mcid, _)| *mcid);
            let mut array = pdf.indirect(*parent_ref).array();
            for (_, reference) in by_mcid {
                array.item(reference);
            }
            array.finish();
        }
    }

    fn ordered_kids(
        &self,
        id: StructureId,
        form_kids: &BTreeMap<StructureId, Vec<AnnotationKid>>,
    ) -> Vec<StructureKid> {
        let Some(node) = self.structure.node(id) else {
            return Vec::new();
        };
        let forms = form_kids.get(&id).map_or(&[][..], Vec::as_slice);
        if forms.is_empty()
            && self
                .occurrences
                .get(&id)
                .is_none_or(|marks| marks.is_empty())
        {
            return node
                .children
                .iter()
                .filter_map(|child| self.node_ref(*child).map(StructureKid::Element))
                .collect();
        }
        // A form element sorts before the marked content its fragment
        // precedes, so the stable index of every other kid starts after them.
        let mut kids = Vec::new();
        for (index, form) in forms.iter().enumerate() {
            kids.push((form.order, index, StructureKid::Element(form.element)));
        }
        for (index, child) in node.children.iter().enumerate() {
            if let Some(reference) = self.node_ref(*child) {
                kids.push((
                    self.first_occurrence(*child, form_kids).unwrap_or((
                        usize::MAX,
                        i32::MAX,
                        u32::MAX,
                    )),
                    forms.len() + index,
                    StructureKid::Element(reference),
                ));
            }
        }
        let mark_offset = forms.len() + node.children.len();
        for (index, mark) in self
            .occurrences
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .enumerate()
        {
            kids.push((
                (mark.page_index, mark.mcid, u32::MAX),
                mark_offset + index,
                StructureKid::MarkedContent(mark),
            ));
        }
        kids.sort_by_key(|(order, stable_index, _)| (*order, *stable_index));
        kids.into_iter().map(|(_, _, kid)| kid).collect()
    }

    /// Where an element first shows: its first marked content, or the first
    /// field it holds, whichever comes first, its descendants included.
    fn first_occurrence(
        &self,
        id: StructureId,
        form_kids: &BTreeMap<StructureId, Vec<AnnotationKid>>,
    ) -> Option<ReadingOrder> {
        let own = self
            .occurrences
            .get(&id)
            .and_then(|marks| marks.first())
            .map(|mark| (mark.page_index, mark.mcid, u32::MAX));
        let form = form_kids
            .get(&id)
            .and_then(|kids| kids.iter().map(|kid| kid.order).min());
        let child = self
            .structure
            .node(id)?
            .children
            .iter()
            .filter_map(|child| self.first_occurrence(*child, form_kids))
            .min();
        own.into_iter().chain(form).chain(child).min()
    }
}

pub(crate) fn valid_structure(
    structure: &DocumentStructure,
    pages: &[std::sync::Arc<PageFrame>],
) -> bool {
    if structure.nodes.is_empty()
        || structure
            .nodes
            .iter()
            .enumerate()
            .any(|(index, node)| node.id.get() as usize != index + 1)
        || structure
            .node(structure.root)
            .is_none_or(|root| root.role != StructureRole::Document)
    {
        return false;
    }

    let mut parents = HashMap::new();
    for node in &structure.nodes {
        for child in &node.children {
            if structure.node(*child).is_none() || parents.insert(*child, node.id).is_some() {
                return false;
            }
        }
    }
    if parents.contains_key(&structure.root)
        || structure
            .nodes
            .iter()
            .any(|node| node.id != structure.root && !parents.contains_key(&node.id))
    {
        return false;
    }

    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![structure.root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            return false;
        }
        let Some(node) = structure.node(id) else {
            return false;
        };
        stack.extend(node.children.iter().copied());
    }
    if seen.len() != structure.nodes.len() {
        return false;
    }

    pages
        .iter()
        .all(|page| marked_ids_exist(&page.elements, structure))
}

fn marked_ids_exist(elements: &[PositionedElement], structure: &DocumentStructure) -> bool {
    elements.iter().all(|element| match element {
        PositionedElement::MarkedContent {
            structure: Some(id),
            children,
        } => structure.node(*id).is_some() && marked_ids_exist(children, structure),
        PositionedElement::MarkedContent { children, .. }
        | PositionedElement::Group(oxoffice_layout::GroupElement { children, .. })
        | PositionedElement::FormField(oxoffice_layout::FormFieldFragment { children, .. }) => {
            marked_ids_exist(children, structure)
        }
        _ => true,
    })
}

/// One page's marked content numbering, in the order the writer emits it.
struct Marks<'m> {
    page_index: usize,
    next_mcid: i32,
    next_fragment: u32,
    next_link: usize,
    occurrences: &'m mut BTreeMap<StructureId, Vec<MarkOccurrence>>,
    fragments: &'m mut Vec<FragmentMark>,
    links: &'m mut HashMap<(usize, usize), LinkMark>,
}

impl Marks<'_> {
    fn collect(&mut self, elements: &[PositionedElement], parent: Option<StructureId>) {
        for element in elements {
            match element {
                PositionedElement::MarkedContent {
                    structure: Some(id),
                    children,
                } if has_content_stream_output(children) => {
                    self.occurrences
                        .entry(*id)
                        .or_default()
                        .push(MarkOccurrence {
                            page_index: self.page_index,
                            mcid: self.next_mcid,
                        });
                    self.next_mcid += 1;
                    self.collect(children, Some(*id));
                }
                PositionedElement::MarkedContent {
                    structure,
                    children,
                } => self.collect(children, structure.or(parent)),
                PositionedElement::Group(oxoffice_layout::GroupElement { children, .. }) => {
                    self.collect(children, parent);
                }
                // Its children are drawn without marks, so they number nothing
                // but the links among them.
                PositionedElement::FormField(fragment) => {
                    self.fragments.push(FragmentMark {
                        field: fragment.field,
                        page_index: self.page_index,
                        mcid_before: self.next_mcid,
                        sequence: self.next_fragment,
                        parent,
                    });
                    self.next_fragment += 1;
                    walk(&fragment.children, &mut |element, _| {
                        if matches!(element, PositionedElement::LinkAnnotation { .. }) {
                            self.link(parent);
                        }
                    });
                }
                PositionedElement::LinkAnnotation { .. } => self.link(parent),
                _ => {}
            }
        }
    }

    /// The next link annotation of the page, in the order `walk` meets it.
    fn link(&mut self, parent: Option<StructureId>) {
        self.links.insert(
            (self.page_index, self.next_link),
            LinkMark {
                mcid_before: self.next_mcid,
                sequence: self.next_fragment,
                parent,
            },
        );
        self.next_link += 1;
        self.next_fragment += 1;
    }
}

pub(crate) fn has_content_stream_output(elements: &[PositionedElement]) -> bool {
    elements.iter().any(|element| match element {
        PositionedElement::Text(run) => !(run.text.is_empty() && run.glyph_ids.is_empty()),
        PositionedElement::MultilingualText(run) => {
            !(run.logical_text.is_empty() && run.glyph_ids.is_empty())
        }
        PositionedElement::LinkAnnotation { .. } => false,
        // Drawn in its field's appearance, or without marks on a later page.
        PositionedElement::FormField(_) => false,
        PositionedElement::Group(group) => has_content_stream_output(&group.children),
        PositionedElement::MarkedContent { children, .. } => has_content_stream_output(children),
        _ => true,
    })
}

fn parent_map(structure: &DocumentStructure) -> HashMap<StructureId, StructureId> {
    structure
        .nodes
        .iter()
        .flat_map(|node| node.children.iter().map(move |child| (*child, node.id)))
        .collect()
}

pub(crate) fn role_name(role: StructureRole) -> &'static [u8] {
    match role {
        StructureRole::Document => b"Document",
        StructureRole::Paragraph => b"P",
        StructureRole::Heading(1) => b"H1",
        StructureRole::Heading(2) => b"H2",
        StructureRole::Heading(3) => b"H3",
        StructureRole::Heading(4) => b"H4",
        StructureRole::Heading(5) => b"H5",
        StructureRole::Heading(_) => b"H6",
        StructureRole::List => b"L",
        StructureRole::ListItem => b"LI",
        StructureRole::Table => b"Table",
        StructureRole::TableRow => b"TR",
        StructureRole::TableHeaderCell => b"TH",
        StructureRole::TableCell => b"TD",
        StructureRole::Figure => b"Figure",
        _ => b"Span",
    }
}

fn pdf_role(role: StructureRole) -> StructRole {
    match role {
        StructureRole::Document => StructRole::Document,
        StructureRole::Paragraph => StructRole::P,
        StructureRole::Heading(1) => StructRole::H1,
        StructureRole::Heading(2) => StructRole::H2,
        StructureRole::Heading(3) => StructRole::H3,
        StructureRole::Heading(4) => StructRole::H4,
        StructureRole::Heading(5) => StructRole::H5,
        StructureRole::Heading(_) => StructRole::H6,
        StructureRole::List => StructRole::L,
        StructureRole::ListItem => StructRole::LI,
        StructureRole::Table => StructRole::Table,
        StructureRole::TableRow => StructRole::TR,
        StructureRole::TableHeaderCell => StructRole::TH,
        StructureRole::TableCell => StructRole::TD,
        StructureRole::Figure => StructRole::Figure,
        _ => StructRole::Span,
    }
}
