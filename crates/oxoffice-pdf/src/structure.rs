//! Deterministic allocation and serialization of tagged PDF structure.

use std::collections::{BTreeMap, HashMap, HashSet};

use oxoffice_layout::{
    DocumentStructure, FormFieldId, PageFrame, Point, PositionedElement, Rect, StructureId,
    StructureRole, Transform, walk,
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
#[derive(Debug, Clone)]
struct LinkMark {
    /// Marked content sequences the page opens before the link.
    mcid_before: i32,
    /// The link's rank among the page's fragments and links.
    sequence: u32,
    /// The structure element of the marked content around the link.
    parent: Option<StructureId>,
    /// Its rectangle in page coordinates.
    rect: Rect,
    url: String,
}

/// A marked content sequence that draws text: its structure element, where
/// each of its runs starts in page coordinates, and the text it draws.
#[derive(Debug, Clone)]
struct TextMark {
    occurrence: MarkOccurrence,
    parent: StructureId,
    origins: Vec<Point>,
    text: String,
}

/// The pieces of one link: its annotations, by page and rank on the page,
/// and the marked content of its text, both in reading order.
#[derive(Debug, Default)]
struct LinkGroup {
    links: Vec<(usize, usize)>,
    marks: Vec<MarkOccurrence>,
    text: String,
}

/// Half a point: how far outside its link's rectangle a run may start.
const LINK_SLACK: f64 = 0.5;

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

/// A kid of a link element: the marked content of its text, or one of its
/// annotations with its page.
#[derive(Debug, Clone, Copy)]
enum LinkKid {
    Text(MarkOccurrence),
    Annotation(Ref, usize),
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
    /// Each tagged link, its pieces and its text.
    link_groups: Vec<LinkGroup>,
    /// The group of each link annotation in a group.
    link_group_of: HashMap<(usize, usize), usize>,
}

impl<'a> PreparedStructure<'a> {
    /// The structure of `pages`, `annotated` holding the links that get an
    /// annotation, by page and rank on the page.
    pub(crate) fn new(
        structure: &'a DocumentStructure,
        pages: &[std::sync::Arc<PageFrame>],
        annotated: &HashSet<(usize, usize)>,
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
        let mut texts = Vec::new();
        for (page_index, page) in pages.iter().enumerate() {
            let mut marks = Marks {
                page_index,
                next_mcid: 0,
                next_fragment: 0,
                next_link: 0,
                occurrences: &mut occurrences,
                fragments: &mut fragments,
                links: &mut links,
                texts: &mut texts,
            };
            marks.collect(&page.elements, None, Transform::IDENTITY);
        }
        let link_groups = group_links(&links, &texts, annotated);
        let mut link_group_of = HashMap::new();
        for (index, group) in link_groups.iter().enumerate() {
            for link in &group.links {
                link_group_of.insert(*link, index);
            }
            // The text of a link leaves its paragraph for its link element.
            for mark in &group.marks {
                for marks in occurrences.values_mut() {
                    marks.retain(|own| (own.page_index, own.mcid) != (mark.page_index, mark.mcid));
                }
            }
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
            link_groups,
            link_group_of,
        })
    }

    /// The group of a tagged link annotation, by its page and its rank among
    /// the page's link annotations: links of one group share an element.
    pub(crate) fn link_group(&self, page_index: usize, ordinal: usize) -> Option<usize> {
        self.link_group_of.get(&(page_index, ordinal)).copied()
    }

    /// The text a tagged link shows, if it shows any.
    pub(crate) fn link_text(&self, page_index: usize, ordinal: usize) -> Option<&str> {
        self.link_group(page_index, ordinal)
            .map(|group| self.link_groups[group].text.as_str())
            .filter(|text| !text.trim().is_empty())
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
        let mut placed = HashSet::new();
        for link in links {
            let mark = self.links.get(&(link.page_index, link.ordinal));
            let parent = mark
                .and_then(|mark| mark.parent)
                .filter(|parent| self.node_refs.contains_key(parent))
                .unwrap_or(self.structure.root);
            // The pieces of one link sit where its first piece does, at its
            // text or else at its annotation.
            let group = self.link_group(link.page_index, link.ordinal);
            let first_text = group
                .and_then(|group| self.link_groups[group].marks.first())
                .map(|mark| (mark.page_index, mark.mcid, 0));
            let order = first_text.unwrap_or_else(|| {
                mark.map_or((link.page_index, i32::MAX, u32::MAX), |mark| {
                    (link.page_index, mark.mcid_before, mark.sequence)
                })
            });
            if placed.insert(link.element) {
                placement
                    .kids
                    .entry(parent)
                    .or_default()
                    .push(AnnotationKid {
                        order,
                        element: link.element,
                    });
            }
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

        // A link element holds the text of its link and the annotation of
        // each of its pieces, in reading order.
        let mut link_elements: BTreeMap<Ref, Vec<(ReadingOrder, LinkKid)>> = BTreeMap::new();
        let mut link_keys: BTreeMap<Ref, i32> = BTreeMap::new();
        for link in links {
            let kids = link_elements.entry(link.element).or_default();
            link_keys.entry(link.element).or_insert(link.key);
            let order = self
                .links
                .get(&(link.page_index, link.ordinal))
                .map_or((link.page_index, i32::MAX, u32::MAX), |mark| {
                    (link.page_index, mark.mcid_before, mark.sequence)
                });
            kids.push((order, LinkKid::Annotation(link.annotation, link.page_index)));
            let group = self.link_group(link.page_index, link.ordinal);
            if kids.len() == 1
                && let Some(group) = group
            {
                for mark in &self.link_groups[group].marks {
                    kids.push(((mark.page_index, mark.mcid, u32::MAX), LinkKid::Text(*mark)));
                }
            }
        }
        let mut link_mcids: Vec<(MarkOccurrence, Ref)> = Vec::new();
        for (element_ref, kids) in &mut link_elements {
            kids.sort_by_key(|(order, _)| *order);
            let key = link_keys[element_ref];
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
            let mut element = pdf.struct_element(*element_ref);
            element.kind(StructRole::Link);
            element.parent(parent_ref);
            let mut children = element.children();
            for (_, kid) in kids.iter() {
                match *kid {
                    LinkKid::Text(mark) => {
                        let Some(page_ref) = page_ids.get(mark.page_index).copied() else {
                            continue;
                        };
                        children
                            .marked_content_ref()
                            .page(page_ref)
                            .marked_content_id(mark.mcid);
                        link_mcids.push((mark, *element_ref));
                    }
                    LinkKid::Annotation(annotation, page_index) => {
                        if let Some(page_ref) = page_ids.get(page_index).copied() {
                            children.object_ref().object(annotation).page(page_ref);
                        }
                    }
                }
            }
            children.finish();
            element.finish();
        }

        let annotations = widgets.iter().map(|widget| {
            (
                StructRole::Form,
                widget.key,
                widget.element,
                widget.widget,
                widget.page_index,
            )
        });
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
            for (mark, reference) in &link_mcids {
                if mark.page_index == page_index {
                    by_mcid.push((mark.mcid, *reference));
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
    texts: &'m mut Vec<TextMark>,
}

impl Marks<'_> {
    fn collect(
        &mut self,
        elements: &[PositionedElement],
        parent: Option<StructureId>,
        transform: Transform,
    ) {
        for element in elements {
            match element {
                PositionedElement::MarkedContent {
                    structure: Some(id),
                    children,
                } if has_content_stream_output(children) => {
                    let occurrence = MarkOccurrence {
                        page_index: self.page_index,
                        mcid: self.next_mcid,
                    };
                    self.occurrences.entry(*id).or_default().push(occurrence);
                    self.next_mcid += 1;
                    let mut text = TextMark {
                        occurrence,
                        parent: *id,
                        origins: Vec::new(),
                        text: String::new(),
                    };
                    walk(children, &mut |element, inner| {
                        let (origin, run) = match element {
                            PositionedElement::Text(run) => (run.origin, run.text.as_str()),
                            PositionedElement::MultilingualText(run) => {
                                (run.origin, run.logical_text.as_str())
                            }
                            _ => return,
                        };
                        if !run.is_empty() {
                            text.origins.push(inner.then(transform).apply(origin));
                            text.text.push_str(run);
                        }
                    });
                    if !text.origins.is_empty() {
                        self.texts.push(text);
                    }
                    self.collect(children, Some(*id), transform);
                }
                PositionedElement::MarkedContent {
                    structure,
                    children,
                } => self.collect(children, structure.or(parent), transform),
                PositionedElement::Group(group) => {
                    self.collect(&group.children, parent, group.transform.then(transform));
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
                    walk(&fragment.children, &mut |element, inner| {
                        if let PositionedElement::LinkAnnotation { rect, url } = element {
                            self.link(
                                parent,
                                inner.then(transform).transform_rect_bbox(*rect),
                                url,
                            );
                        }
                    });
                }
                PositionedElement::LinkAnnotation { rect, url } => {
                    self.link(parent, transform.transform_rect_bbox(*rect), url);
                }
                _ => {}
            }
        }
    }

    /// The next link annotation of the page, in the order `walk` meets it.
    fn link(&mut self, parent: Option<StructureId>, rect: Rect, url: &str) {
        self.links.insert(
            (self.page_index, self.next_link),
            LinkMark {
                mcid_before: self.next_mcid,
                sequence: self.next_fragment,
                parent,
                rect,
                url: url.to_owned(),
            },
        );
        self.next_link += 1;
        self.next_fragment += 1;
    }
}

/// The tagged links, each with its pieces and its text. A link's text is
/// each marked content sequence of the link's parent, on its page, whose runs
/// all start inside its rectangle. Consecutive annotations with one parent and
/// one target, with no other text of that parent between their texts, are
/// one link. Only links that get an annotation are grouped.
fn group_links(
    links: &HashMap<(usize, usize), LinkMark>,
    texts: &[TextMark],
    annotated: &HashSet<(usize, usize)>,
) -> Vec<LinkGroup> {
    let mut order: Vec<&(usize, usize)> =
        links.keys().filter(|key| annotated.contains(key)).collect();
    order.sort_unstable();
    let inside = |rect: &Rect, point: &Point| {
        point.x >= rect.x - LINK_SLACK
            && point.x <= rect.x + rect.width + LINK_SLACK
            && point.y >= rect.y - LINK_SLACK
            && point.y <= rect.y + rect.height + LINK_SLACK
    };
    // Each text sequence goes to the first link it lies in.
    let mut owner: HashMap<usize, (usize, usize)> = HashMap::new();
    for key in &order {
        let link = &links[key];
        for (index, text) in texts.iter().enumerate() {
            if text.occurrence.page_index == key.0
                && Some(text.parent) == link.parent
                && !owner.contains_key(&index)
                && text.origins.iter().all(|origin| inside(&link.rect, origin))
            {
                owner.insert(index, **key);
            }
        }
    }
    let position = |mark: &MarkOccurrence| (mark.page_index, mark.mcid);
    let own_texts = |key: &(usize, usize)| -> Vec<usize> {
        let mut own: Vec<usize> = owner
            .iter()
            .filter(|(_, link)| *link == key)
            .map(|(index, _)| *index)
            .collect();
        own.sort_unstable_by_key(|index| position(&texts[*index].occurrence));
        own
    };
    // Where a link starts and ends among the page's marked content: its
    // text, or else the place of its annotation.
    let span = |key: &(usize, usize), own: &[usize]| {
        let link = &links[key];
        match (own.first(), own.last()) {
            (Some(first), Some(last)) => (
                position(&texts[*first].occurrence),
                position(&texts[*last].occurrence),
            ),
            _ => ((key.0, link.mcid_before), (key.0, link.mcid_before - 1)),
        }
    };
    let mut groups: Vec<LinkGroup> = Vec::new();
    let mut texts_of: Vec<Vec<usize>> = Vec::new();
    let mut last: Option<((usize, usize), (usize, i32))> = None;
    for key in order {
        let link = &links[key];
        let own = own_texts(key);
        let (start, end) = span(key, &own);
        let joins = last.is_some_and(|(previous, previous_end)| {
            let before = &links[&previous];
            before.parent == link.parent
                && before.url == link.url
                && !texts.iter().enumerate().any(|(index, text)| {
                    Some(text.parent) == link.parent
                        && !owner.contains_key(&index)
                        && previous_end < position(&text.occurrence)
                        && position(&text.occurrence) < start
                })
        });
        if !joins {
            groups.push(LinkGroup::default());
            texts_of.push(Vec::new());
        }
        let group = groups.last_mut().expect("a group");
        group.links.push(*key);
        texts_of.last_mut().expect("a group").extend(own);
        last = Some((*key, end));
    }
    for (group, own) in groups.iter_mut().zip(texts_of) {
        let mut previous: Option<&TextMark> = None;
        for index in own {
            let text = &texts[index];
            group.marks.push(text.occurrence);
            // Two lines of one link read with a space between them.
            if let Some(previous) = previous
                && (previous.origins[0].y - text.origins[0].y).abs() > LINK_SLACK
                && !previous.text.ends_with(char::is_whitespace)
                && !text.text.starts_with(char::is_whitespace)
            {
                group.text.push(' ');
            }
            group.text.push_str(&text.text);
            previous = Some(text);
        }
    }
    groups
}

pub(crate) fn has_content_stream_output(elements: &[PositionedElement]) -> bool {
    elements.iter().any(|element| match element {
        PositionedElement::Text(run) => !(run.text.is_empty() && run.glyph_ids.is_empty()),
        PositionedElement::MultilingualText(run) => {
            !(run.logical_text.is_empty() && run.glyph_ids.is_empty())
        }
        PositionedElement::LinkAnnotation { .. } | PositionedElement::Destination { .. } => false,
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
