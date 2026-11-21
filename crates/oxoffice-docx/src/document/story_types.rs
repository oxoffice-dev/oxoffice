//! Story, item and header or footer kinds, body item references and story errors, and the methods of the story ids, content locations, item references and snapshots defined in document.rs.

use super::*;

/// One direct child of a document body, in source order.
pub enum BodyItemRef<'a> {
    /// A body paragraph.
    Paragraph(ParagraphRef<'a>),
    /// A body table.
    Table(TableRef<'a>),
    /// A body-level content control.
    ContentControl(ContentControlRef<'a>),
    /// A preserved body child that oxoffice does not model.
    UnsupportedXml(&'a [u8]),
}

/// A container in the WordprocessingML story graph.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoryKind {
    Body,
    TableCell,
    Header,
    Footer,
    Footnote,
    Endnote,
    Comment,
    TextBox,
}

/// Whether a section story is a header or a footer.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderFooterKind {
    Header,
    Footer,
}

impl HeaderFooterKind {
    pub(super) fn story_kind(self) -> StoryKind {
        match self {
            Self::Header => StoryKind::Header,
            Self::Footer => StoryKind::Footer,
        }
    }

    pub(super) fn relationship_type(self) -> &'static str {
        match self {
            Self::Header => rel_types::HEADER,
            Self::Footer => rel_types::FOOTER,
        }
    }

    pub(super) fn content_type(self) -> &'static str {
        match self {
            Self::Header => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"
            }
            Self::Footer => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"
            }
        }
    }

    pub(super) fn part_stem(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Footer => "footer",
        }
    }

    pub(super) fn is_header(self) -> bool {
        self == Self::Header
    }
}

impl SectionStory {
    /// Return the physical story owner.
    pub fn story(&self) -> &StoryId {
        &self.story
    }

    /// Return the section that supplies the effective reference.
    pub fn source_section(&self) -> usize {
        self.source_section
    }

    /// Return whether the selected section inherits this story.
    pub fn is_inherited(&self) -> bool {
        self.inherited
    }
}

/// The supported kind of one item in a story.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoryItemKind {
    Paragraph,
    Table,
    ContentControl,
    Field,
    Drawing,
    PreservedNode,
}

impl StoryId {
    /// Construct an unfingerprinted owner identity for checked lookup.
    ///
    /// Identities returned by [`Document::stories`] are required for a
    /// successful mutation. This constructor lets callers deserialize or
    /// validate external paths without granting unchecked access.
    pub fn new(kind: StoryKind, part_name: impl Into<String>, owner_index: usize) -> Self {
        Self {
            kind,
            part_name: part_name.into(),
            owner_index,
            fingerprint: 0,
        }
    }

    /// Construct the conventional main-body identity for checked lookup.
    ///
    /// Prefer an identity returned by [`Document::stories`]. This constructor
    /// is useful when validating externally retained locations and deliberately
    /// carries no structural fingerprint.
    pub fn body() -> Self {
        Self::new(StoryKind::Body, "/word/document.xml", 0)
    }

    pub fn kind(&self) -> StoryKind {
        self.kind
    }

    pub fn part_name(&self) -> &str {
        &self.part_name
    }

    pub fn owner_index(&self) -> usize {
        self.owner_index
    }

    /// Set the owner index to `owner_index`, keeping the fingerprint.
    ///
    /// The caller holds the identity of an owner whose bytes did not change
    /// while owners of the same kind were inserted before it in the same
    /// part, and `owner_index` is the index that now reaches that owner.
    #[doc(hidden)]
    pub fn set_owner_index(&mut self, owner_index: usize) {
        self.owner_index = owner_index;
    }
}

impl ContentLocation {
    pub fn new(story: StoryId, item_kind: StoryItemKind, index_path: Vec<usize>) -> Self {
        Self {
            story,
            item_kind,
            index_path,
            is_end: false,
        }
    }

    /// Construct the insertion boundary after a story's final direct content.
    ///
    /// For the document body, this boundary remains before section properties.
    pub fn end(story: StoryId) -> Self {
        Self {
            story,
            item_kind: StoryItemKind::PreservedNode,
            index_path: Vec::new(),
            is_end: true,
        }
    }

    pub fn story(&self) -> &StoryId {
        &self.story
    }

    pub fn item_kind(&self) -> StoryItemKind {
        self.item_kind
    }

    pub fn index_path(&self) -> &[usize] {
        &self.index_path
    }

    /// Set the first step of the path to `index`.
    ///
    /// The caller holds a location whose story gained or lost items before
    /// its first step, and `index` is the step that now reaches the same
    /// item. The story fingerprint is left as it was.
    #[doc(hidden)]
    pub fn set_index_path_head(&mut self, index: usize) {
        if let Some(head) = self.index_path.first_mut() {
            *head = index;
        }
    }
}

/// One exact owner slice used for generated-table discovery and cache edits.
pub(crate) struct GeneratedStoryOwner {
    pub(crate) story: StoryId,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) xml: Vec<u8>,
    pub(crate) namespaces: BTreeMap<String, String>,
}

impl StoryItemSnapshot {
    /// Return the checked story location captured by this snapshot.
    pub fn location(&self) -> &ContentLocation {
        &self.location
    }

    /// Return the containing direct main-body child when this item has one.
    pub fn direct_body_index(&self) -> Option<usize> {
        self.direct_body_index
    }

    /// Return whether this item is a direct child of its story owner.
    ///
    /// An item nested in another item of the same owner, such as an inline
    /// content control, a field, or a drawing inside a paragraph, returns
    /// `false`. The direct items of one owner never overlap, so their text
    /// reads each part of the owner once.
    pub fn is_direct_child(&self) -> bool {
        self.direct_child
    }

    /// Return accepted-view text when the item is text bearing.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Return the exact or namespace-complete XML captured for this item.
    pub fn xml(&self) -> &[u8] {
        &self.xml
    }
}

/// Where one element of a story lies in its part, which tells what an edit
/// of that element alone renumbers: [`Document::element_extent`].
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementExtent {
    /// The element's bytes in its part, closed over the namespaces it uses.
    pub xml: Vec<u8>,
    /// The index of the first story item inside the element and how many
    /// there are, the element itself included when it is an item. With none,
    /// the index the next item would take.
    pub items: (usize, usize),
    /// The hyperlinks of the element's story in document order: the index
    /// path of the story item that holds each, its index among that item's
    /// hyperlinks, and where it lies against the element (`Less` before,
    /// `Equal` inside, `Greater` after).
    pub links: Vec<(Vec<usize>, usize, std::cmp::Ordering)>,
    /// The text box stories of the part that start before the element, and
    /// how many lie inside it.
    pub text_boxes: (usize, usize),
    /// The table cell stories of the part, counted the same way. The story
    /// an element is, when it is one, counts as neither.
    pub cell_stories: (usize, usize),
}

/// The bytes of the story owners, story items, paragraphs and hyperlinks of
/// every story, by route, each closed over the namespaces it uses:
/// [`Document::owner_element_xml`].
#[doc(hidden)]
#[derive(Debug, Default)]
#[allow(clippy::type_complexity)] // Each element family keyed by its story and route.
pub struct OwnerElementXml {
    /// Each story owner (a table cell, a text box, a header...) by its
    /// kind, part and ordinal.
    pub stories: HashMap<(StoryKind, String, usize), Vec<u8>>,
    /// Each story item by its story's kind, part and ordinal and its index
    /// path.
    pub items: HashMap<(StoryKind, String, usize, Vec<usize>), Vec<u8>>,
    /// Each paragraph of [`Document::story_owner_snapshots`], keyed the same
    /// way.
    pub paragraphs: HashMap<(StoryKind, String, usize, Vec<usize>), Vec<u8>>,
    /// Each hyperlink by its story's kind, part and ordinal, the index path
    /// of the story item that holds it and its index among that item's
    /// hyperlinks.
    pub links: HashMap<(StoryKind, String, usize, Vec<usize>, usize), Vec<u8>>,
}

/// The checked Word owners of one body item, which
/// [`Document::body_item_owners`] reads from a main part whose body holds
/// only that item.
#[doc(hidden)]
#[allow(clippy::type_complexity)] // The four owned families and the item's index projections.
pub struct BodyItemOwners {
    /// The four families of [`Document::story_owner_snapshots`].
    pub snapshots: Vec<StoryItemSnapshot>,
    pub cells: Vec<(ContentLocation, usize, usize, StoryId)>,
    pub paragraphs: Vec<(ContentLocation, Vec<u8>)>,
    pub links: Vec<(ContentLocation, LinkInfo)>,
    /// The table cell story owners of the item.
    pub cell_stories: usize,
    /// The text box story owners of the item.
    pub text_boxes: usize,
    /// The body story path that [`Document::paragraph_story_locations`]
    /// gives each paragraph of the item, in [`Document::paragraphs`] order.
    pub paragraph_routes: Vec<Option<Vec<usize>>>,
    /// The tables of the item that [`Document::tables`] counts.
    pub tables: usize,
    /// [`Document::cell_paragraph_story_locations`] for those tables, keyed
    /// by table within the item, and whether it routed every table cell
    /// owner of the item, or `None` where it fails.
    pub cell_paragraphs: Option<(HashMap<(usize, usize, usize), Vec<ContentLocation>>, bool)>,
    /// Whether the item bytes contain `fldSimple` or `fldChar`.
    /// A positive result is conservative. Equal field values are not reuse proof.
    pub holds_field: bool,
}

impl BodyItemContext {
    /// The main part, which holds the body story.
    pub fn part_name(&self) -> &str {
        &self.part_name
    }

    /// Whether equal eligible body blocks have the same checked read facts.
    ///
    /// Exact model writer inputs and body declarations stay ordered. Only
    /// unique retained root declarations and effective bindings may reorder.
    /// This is not source, writer or opening identity.
    pub fn read_facts_compatible(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        if self.part_name != other.part_name
            || self.plain_local_edit_blocked != other.plain_local_edit_blocked
            || [0, 1, 3]
                .iter()
                .any(|&index| self.root[index] != other.root[index])
        {
            return false;
        }
        for context in [self, other] {
            for entries in &context.root {
                if entries.iter().enumerate().any(|(index, (name, _))| {
                    entries[..index]
                        .iter()
                        .any(|(previous, _)| previous == name)
                }) {
                    return false;
                }
            }
        }
        [2, 4].iter().all(|&index| {
            let before = &self.root[index];
            let after = &other.root[index];
            before.len() == after.len()
                && before.iter().all(|entry| {
                    (entry.0 == "xmlns"
                        || entry
                            .0
                            .strip_prefix("xmlns:")
                            .is_some_and(|prefix| !prefix.is_empty() && !prefix.contains(':')))
                        && after.contains(entry)
                })
        })
    }
}

impl<'a> StoryItemRef<'a> {
    pub fn location(&self) -> &ContentLocation {
        &self.location
    }

    pub fn kind(&self) -> StoryItemKind {
        self.location.item_kind
    }

    /// Return the containing direct main-body child when this item has one.
    ///
    /// The recursive story-item path remains unchanged. This value is the
    /// coordinate accepted by direct-body APIs such as `RunPosition`.
    pub fn direct_body_index(&self) -> Result<Option<usize>> {
        if self.location.story.kind != StoryKind::Body {
            return Ok(None);
        }
        let (source, owner) = self.document.story_source_and_owner(&self.location.story)?;
        let [index] = self.location.index_path.as_slice() else {
            return Err(StoryError::InvalidPath {
                path: self.location.index_path.clone(),
            }
            .into());
        };
        let items = scan_story_items(source.xml.as_ref(), &owner, None)?;
        let item = items.get(*index).ok_or(StoryError::OutOfBounds {
            index: *index,
            len: items.len(),
        })?;
        Ok(direct_story_content_items(source.xml.as_ref(), &owner)?
            .iter()
            .position(|candidate| {
                candidate.full.start <= item.full.start && item.full.end <= candidate.full.end
            }))
    }

    pub fn text(&self) -> Result<Option<String>> {
        let (source, item) = self.document.story_item_source(&self.location)?;
        story_item_text(source.xml.as_ref(), &item, &[], 0..source.xml.len(), 0)
    }

    /// Return modeled hyperlinks in source order with story-scoped targets.
    pub fn links(&self) -> Result<Vec<LinkInfo>> {
        let (source, item) = self.document.story_item_source(&self.location)?;
        let item_scopes = story_namespace_scopes_at(source.xml.as_ref(), [item.scan.start])?;
        let item_scope = item_scopes.get(&item.scan.start).ok_or_else(|| {
            Error::Other("story item namespace scope was not inventoried".to_owned())
        })?;
        let links = scan_story_item_links_with_scope(source.xml.as_ref(), &item, item_scope)?;
        let link_scopes = story_namespace_scopes_at(
            source.xml.as_ref(),
            links.iter().map(|link| link.full.start),
        )?;
        links
            .into_iter()
            .map(|link| {
                let scope = link_scopes.get(&link.full.start).ok_or_else(|| {
                    Error::Other("story hyperlink namespace scope was not inventoried".to_owned())
                })?;
                self.document.story_link_info(
                    &self.location.story,
                    source.xml.as_ref(),
                    link,
                    scope,
                )
            })
            .collect()
    }

    /// Returns the XML used by this traversal.
    ///
    /// Typed body and comment sources can return owned subtree bytes. Other
    /// package-backed sources can return a borrowed exact slice, which can rely
    /// on namespace declarations from ancestors outside that slice. Complex
    /// fields return an owned, standalone paragraph with the in-scope namespace
    /// declarations materialized.
    pub fn xml(&self) -> Result<Cow<'a, [u8]>> {
        let (source, item) = self.document.story_item_source(&self.location)?;
        if item.complex_field {
            return Ok(Cow::Owned(complex_story_field_xml(
                source.xml.as_ref(),
                &item,
            )?));
        }
        match source.xml {
            Cow::Borrowed(xml) => Ok(Cow::Borrowed(&xml[item.full])),
            Cow::Owned(xml) => Ok(Cow::Owned(xml[item.full].to_vec())),
        }
    }
}

/// A location resolution failure from the container-neutral story API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoryError {
    OwnerNotFound {
        story: StoryId,
    },
    WrongOwner {
        expected: StoryKind,
        actual: StoryKind,
    },
    Stale {
        story: StoryId,
    },
    InvalidPath {
        path: Vec<usize>,
    },
    OutOfBounds {
        index: usize,
        len: usize,
    },
    KindMismatch {
        expected: StoryItemKind,
        actual: StoryItemKind,
    },
    NotTextBearing {
        kind: StoryItemKind,
    },
}

impl fmt::Display for StoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OwnerNotFound { story } => write!(
                formatter,
                "story owner {:?} at {}[{}] was not found",
                story.kind, story.part_name, story.owner_index
            ),
            Self::WrongOwner { expected, actual } => {
                write!(
                    formatter,
                    "story owner kind is {actual:?}, expected {expected:?}"
                )
            }
            Self::Stale { story } => write!(
                formatter,
                "story location for {:?} at {}[{}] is stale",
                story.kind, story.part_name, story.owner_index
            ),
            Self::InvalidPath { path } => write!(formatter, "invalid story item path {path:?}"),
            Self::OutOfBounds { index, len } => {
                write!(
                    formatter,
                    "story item index {index} is out of bounds for {len} items"
                )
            }
            Self::KindMismatch { expected, actual } => write!(
                formatter,
                "story item kind is {actual:?}, expected {expected:?}"
            ),
            Self::NotTextBearing { kind } => {
                write!(formatter, "story item kind {kind:?} has no editable text")
            }
        }
    }
}

impl std::error::Error for StoryError {}
