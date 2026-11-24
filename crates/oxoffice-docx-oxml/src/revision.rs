//! Tracked revision metadata and read-only content projections.

use std::sync::Arc;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::content_control::{CT_Sdt, SdtContent};
use crate::document::{BodyContent, CT_Document, CT_SectPr};
use crate::namespace::W_NS;
use crate::numbering::{parse_scoped_ppr, parse_scoped_rpr, word_prefixes_at};
use crate::properties::{CT_PPr, CT_RPr, is_word_element};
use crate::raw_xml::capture_empty_element;
use crate::table::{CT_Row, CT_Tbl, CT_TblPr, CT_Tc, CellContent};
use crate::text::{AcceptedRunPath, AcceptedRunPathSegment, CT_P, CT_R, hyperlink_revision_index};

const MAX_REVISION_NESTING_DEPTH: usize = 32;

/// Physical legacy read facts captured by the one canonical paragraph parser.
/// These are operation-local, before complex fields regroup physical runs.
#[derive(Default)]
pub(crate) struct RevisionSourceFacts {
    pub(crate) runs: Vec<CT_R>,
    pub(crate) revisions: Vec<(usize, CT_Revision)>,
    copy_nodes_bound: usize,
    copy_bytes_bound: usize,
    copy_nodes: usize,
    copy_bytes: usize,
}

impl RevisionSourceFacts {
    fn with_copy_bound(bound: (usize, usize)) -> Self {
        Self {
            copy_nodes_bound: bound.0,
            copy_bytes_bound: bound.1,
            ..Self::default()
        }
    }

    fn charge(&mut self, nodes: usize, bytes: usize) -> crate::Result<()> {
        let nodes = self.copy_nodes.checked_add(nodes).ok_or_else(|| {
            crate::OxmlError::InvalidValue("secondary source-copy visit overflow".into())
        })?;
        let bytes = self.copy_bytes.checked_add(bytes).ok_or_else(|| {
            crate::OxmlError::InvalidValue("secondary source-copy byte overflow".into())
        })?;
        if nodes > self.copy_nodes_bound || bytes > self.copy_bytes_bound {
            return Err(crate::OxmlError::InvalidValue(
                "secondary source-copy work exceeds its admitted source ancestry".into(),
            ));
        }
        self.copy_nodes = nodes;
        self.copy_bytes = bytes;
        Ok(())
    }

    pub(crate) fn push_run(&mut self, run: &CT_R, source: &[u8]) -> crate::Result<()> {
        self.charge(1, source.len())?;
        self.runs.push(run.clone());
        Ok(())
    }

    pub(crate) fn push_revision(&mut self, revision: &CT_Revision) -> crate::Result<()> {
        let mut reader = Reader::from_reader(revision.raw_xml.as_slice());
        let root = match reader.read_event()? {
            Event::Start(root) | Event::Empty(root) => root,
            _ => return Ok(()),
        };
        let prefixes = word_prefixes_at(&root, &revision.parse_word_prefixes)?;
        if optional_word_attribute(&root, b"author", &prefixes)?.is_none() {
            return Ok(());
        }
        let mut nodes = 0usize;
        let mut visible_raw_bytes = 0usize;
        let mut pending = vec![revision];
        while let Some(current) = pending.pop() {
            nodes = nodes.checked_add(1).ok_or_else(|| {
                crate::OxmlError::InvalidValue("secondary revision-copy visit overflow".into())
            })?;
            visible_raw_bytes = visible_raw_bytes
                .checked_add(current.raw_xml.len())
                .ok_or_else(|| {
                    crate::OxmlError::InvalidValue("secondary revision-copy byte overflow".into())
                })?;
            if let RevisionContent::Runs(runs) = current.content() {
                nodes = nodes.checked_add(runs.len()).ok_or_else(|| {
                    crate::OxmlError::InvalidValue("secondary run-copy visit overflow".into())
                })?;
                for run in runs {
                    for raw in &run.extra_xml {
                        visible_raw_bytes =
                            visible_raw_bytes.checked_add(raw.len()).ok_or_else(|| {
                                crate::OxmlError::InvalidValue(
                                    "secondary run-copy byte overflow".into(),
                                )
                            })?;
                    }
                    if let Some(properties) = &run.properties {
                        if let Some(change) = &properties.change {
                            visible_raw_bytes = visible_raw_bytes
                                .checked_add(change.raw_xml.len())
                                .ok_or_else(|| {
                                    crate::OxmlError::InvalidValue(
                                        "secondary typed run-property copy byte overflow".into(),
                                    )
                                })?;
                        }
                        for raw in &properties.revision_xml {
                            visible_raw_bytes =
                                visible_raw_bytes.checked_add(raw.len()).ok_or_else(|| {
                                    crate::OxmlError::InvalidValue(
                                        "secondary run-property copy byte overflow".into(),
                                    )
                                })?;
                        }
                    }
                }
            }
            let prior_raw: Vec<&[u8]> = match current.content() {
                RevisionContent::PriorRunProperties(properties) => {
                    properties.revision_xml.iter().map(Vec::as_slice).collect()
                }
                RevisionContent::PriorParagraphProperties(properties) => properties
                    .revision_xml
                    .iter()
                    .chain(&properties.numbering_revision_xml)
                    .map(Vec::as_slice)
                    .chain(
                        properties
                            .num_pr_extra_xml
                            .iter()
                            .map(|(_, _, raw)| raw.as_slice()),
                    )
                    .chain(
                        properties
                            .num_ilvl_raw
                            .iter()
                            .map(|(_, raw, _)| raw.as_slice()),
                    )
                    .chain(
                        properties
                            .num_id_raw
                            .iter()
                            .map(|(_, raw, _)| raw.as_slice()),
                    )
                    .collect(),
                RevisionContent::PriorTableProperties(properties) => properties
                    .revision_xml
                    .iter()
                    .map(Vec::as_slice)
                    .chain(properties.extra_xml.iter().map(|(_, raw)| raw.as_slice()))
                    .collect(),
                RevisionContent::PriorSectionProperties(properties) => {
                    properties.extra_xml.iter().map(Vec::as_slice).collect()
                }
                _ => Vec::new(),
            };
            for raw in prior_raw {
                visible_raw_bytes = visible_raw_bytes.checked_add(raw.len()).ok_or_else(|| {
                    crate::OxmlError::InvalidValue(
                        "secondary prior-property copy byte overflow".into(),
                    )
                })?;
            }
            pending.extend(current.nested_revisions.iter().map(|(_, nested)| nested));
        }
        // The visible retained buffers above are observations of known copies.
        // Source allowance also covers private payloads, never as a measurement.
        let copy_bound = crate::document::admit_word_xml(
            &revision.raw_xml,
            &revision.parse_word_prefixes,
            None,
        )?;
        self.charge(nodes, visible_raw_bytes.max(copy_bound.1))?;
        let mut legacy = revision.clone();
        legacy.require_author = true;
        self.revisions.push((self.runs.len(), legacy));
        Ok(())
    }

    /// Only a hyperlink subtree that the canonical reader retained opaque.
    /// Its legacy direct runs have not been parsed by the canonical route.
    pub(crate) fn append_opaque_hyperlink(
        &mut self,
        raw: &[u8],
        prefixes: &[String],
    ) -> crate::Result<()> {
        let mut reader = Reader::from_reader(raw);
        let root = match reader.read_event()? {
            Event::Start(root) => root,
            _ => return Ok(()),
        };
        let prefixes = word_prefixes_at(&root, prefixes)?;
        parse_run_content_container(
            &mut reader,
            b"hyperlink",
            &prefixes,
            &mut self.runs,
            &mut self.revisions,
            None,
        )
    }
}

/// The modeled tracked-change element kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionKind {
    Insertion,
    Deletion,
    MoveFrom,
    MoveTo,
    RunPropertyChange,
    ParagraphPropertyChange,
    TablePropertyChange,
    SectionPropertyChange,
}

impl RevisionKind {
    /// Whether the change adds or removes content: an insertion, a deletion
    /// or either end of a move, not a change of properties.
    pub fn changes_content(self) -> bool {
        matches!(
            self,
            Self::Insertion | Self::Deletion | Self::MoveFrom | Self::MoveTo
        )
    }
}

/// Existing substantive body-reader modes. Deletions need the real source
/// ownership probe, never a header-only guess.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RevisionBodyMode {
    Ordered,
    Legacy(RevisionKind),
}

impl RevisionBodyMode {
    pub(crate) fn without_source_probe(kind: RevisionKind) -> Option<Self> {
        match kind {
            RevisionKind::Insertion | RevisionKind::MoveTo => Some(Self::Ordered),
            RevisionKind::Deletion | RevisionKind::MoveFrom => None,
            _ => Some(Self::Legacy(kind)),
        }
    }

    fn selected(kind: RevisionKind, ordered_owner: bool) -> Self {
        Self::without_source_probe(kind).unwrap_or(if ordered_owner {
            Self::Ordered
        } else {
            Self::Legacy(kind)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RevisionContentAction {
    Run,
    NestedRevision,
    Hyperlink,
    PriorRunProperties,
    PriorParagraphProperties,
    PriorTableProperties,
    PriorSectionProperties,
    Skip,
}

impl RevisionContentAction {
    pub(crate) fn for_names(
        kind: Option<RevisionKind>,
        local: &[u8],
        word: bool,
        empty: bool,
    ) -> Self {
        if !word {
            return Self::Skip;
        }
        if revision_kind_for_local(local).is_some() {
            return Self::NestedRevision;
        }
        if empty {
            return Self::Skip;
        }
        match local {
            b"r" => Self::Run,
            b"hyperlink" => Self::Hyperlink,
            b"rPr" if kind == Some(RevisionKind::RunPropertyChange) => Self::PriorRunProperties,
            b"pPr" if kind == Some(RevisionKind::ParagraphPropertyChange) => {
                Self::PriorParagraphProperties
            }
            b"tblPr" if kind == Some(RevisionKind::TablePropertyChange) => {
                Self::PriorTableProperties
            }
            b"sectPr" if kind == Some(RevisionKind::SectionPropertyChange) => {
                Self::PriorSectionProperties
            }
            _ => Self::Skip,
        }
    }
}

// Both the actual depth reader and source postorder fact use this exact old
// inclusive constructor boundary. It is a raw-fallback outcome, not a quota.
pub(crate) fn revision_depth_exceeds(depth: usize) -> bool {
    depth > MAX_REVISION_NESTING_DEPTH
}

pub(crate) fn revision_depth_height(parent: bool, child: usize) -> usize {
    (child + usize::from(parent)).min(MAX_REVISION_NESTING_DEPTH + 1)
}

/// The typed content projected from a preserved revision subtree.
#[derive(Debug, Clone, PartialEq)]
pub enum RevisionContent {
    Runs(Vec<CT_R>),
    Marker,
    PriorRunProperties(Box<CT_RPr>),
    PriorParagraphProperties(Box<CT_PPr>),
    PriorTableProperties(Box<CT_TblPr>),
    PriorSectionProperties(Box<CT_SectPr>),
}

/// A tracked revision whose captured subtree remains its serialization source.
#[derive(Clone)]
#[allow(non_camel_case_types)]
pub struct CT_Revision {
    kind: RevisionKind,
    id: i32,
    author: String,
    timestamp: Option<String>,
    raw_xml: Vec<u8>,
    content: RevisionContent,
    content_paragraph: Option<Arc<CT_P>>,
    nested_revisions: Vec<(usize, CT_Revision)>,
    parse_word_prefixes: Vec<String>,
    require_author: bool,
    accepted_fields_pending: bool,
    accepted_payload_pending: bool,
    read_context: Option<Arc<RevisionReadContext>>,
}

// Numeric facts belong to this successful constructor and its real typed
// clones. The weak operation anchor carries neither Work nor input ownership.
struct RevisionReadContext {
    registry: std::sync::Weak<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    raw_map: crate::document::WordSourceMap,
    maps: crate::text::WordBlockSourceMaps,
}
impl std::fmt::Debug for RevisionReadContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevisionReadContext")
            .field("raw_len", &self.raw_map.len())
            .finish_non_exhaustive()
    }
}

impl CT_Revision {
    /// The entries of the parse context that this revision's source reads:
    /// the bindings of the prefixes its captured XML names, in a stable
    /// order. The other bindings in scope belong to the part, which another
    /// producer or a save declares differently for the same revision.
    fn read_prefix_context(&self) -> Vec<&String> {
        let mut entries = match raw_name_prefixes(&self.raw_xml) {
            Some(used) => self
                .parse_word_prefixes
                .iter()
                .filter(|entry| {
                    let prefix = entry
                        .strip_prefix('\0')
                        .and_then(|entry| entry.split_once('\0'))
                        .map_or(entry.as_str(), |(prefix, _)| prefix);
                    used.contains(prefix)
                })
                .collect::<Vec<_>>(),
            None => self.parse_word_prefixes.iter().collect(),
        };
        entries.sort_unstable();
        entries
    }
}

/// The prefixes the element and attribute names of `raw` use, `""` for an
/// unprefixed element, and those its markup-compatibility attributes name in
/// their values; `None` when `raw` does not read as XML.
fn raw_name_prefixes(raw: &[u8]) -> Option<std::collections::HashSet<String>> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    let mut used = std::collections::HashSet::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element) => {
                let prefix = match element.name().prefix() {
                    Some(prefix) => std::str::from_utf8(prefix.as_ref()).ok()?.to_owned(),
                    None => String::new(),
                };
                used.insert(prefix);
                for attribute in element.attributes() {
                    let attribute = attribute.ok()?;
                    let key = attribute.key;
                    if let Some(prefix) = key.prefix()
                        && !matches!(prefix.as_ref(), b"xmlns" | b"xml")
                    {
                        used.insert(std::str::from_utf8(prefix.as_ref()).ok()?.to_owned());
                    }
                    if matches!(
                        key.local_name().as_ref(),
                        b"Requires"
                            | b"Ignorable"
                            | b"MustUnderstand"
                            | b"ProcessContent"
                            | b"PreserveElements"
                            | b"PreserveAttributes"
                    ) {
                        for token in std::str::from_utf8(&attribute.value)
                            .ok()?
                            .split_whitespace()
                        {
                            let prefix = token.split_once(':').map_or(token, |(prefix, _)| prefix);
                            used.insert(prefix.to_owned());
                        }
                    }
                }
            }
            quick_xml::events::Event::Eof => return Some(used),
            _ => {}
        }
        buffer.clear();
    }
}

// Callers compare Debug output as a signature, so the prefix context is
// written as equality reads it: the bindings the revision reads, in a
// stable order.
impl std::fmt::Debug for CT_Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parse_word_prefixes = self.read_prefix_context();
        f.debug_struct("CT_Revision")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("author", &self.author)
            .field("timestamp", &self.timestamp)
            .field("raw_xml", &self.raw_xml)
            .field("content", &self.content)
            .field("content_paragraph", &self.content_paragraph)
            .field("nested_revisions", &self.nested_revisions)
            .field("parse_word_prefixes", &parse_word_prefixes)
            .field("require_author", &self.require_author)
            .field("accepted_fields_pending", &self.accepted_fields_pending)
            .field("accepted_payload_pending", &self.accepted_payload_pending)
            .field("read_context", &self.read_context)
            .finish()
    }
}

impl PartialEq for CT_Revision {
    fn eq(&self, other: &Self) -> bool {
        if self.kind != other.kind
            || self.id != other.id
            || self.author != other.author
            || self.timestamp != other.timestamp
            || self.raw_xml != other.raw_xml
            || self.content != other.content
            || self.content_paragraph != other.content_paragraph
            || self.nested_revisions != other.nested_revisions
            || self.require_author != other.require_author
        {
            return false;
        }
        // Context enumeration order is parser bookkeeping, not revision
        // identity, and so are the bindings the captured source never reads.
        let mut left = self.parse_word_prefixes.iter().collect::<Vec<_>>();
        let mut right = other.parse_word_prefixes.iter().collect::<Vec<_>>();
        left.sort_unstable();
        right.sort_unstable();
        left == right || self.read_prefix_context() == other.read_prefix_context()
    }
}

/// The actual root decoder, shared by construction and contextual source actions.
pub(crate) struct RevisionHeader {
    pub(crate) kind: RevisionKind,
    id: i32,
    author: String,
    timestamp: Option<String>,
}

impl RevisionHeader {
    pub(crate) fn read(
        start: &quick_xml::events::BytesStart<'_>,
        prefixes: &[String],
        require_author: bool,
    ) -> crate::Result<Self> {
        let kind = revision_kind(start.name().as_ref(), prefixes).ok_or_else(|| {
            crate::OxmlError::InvalidValue("unsupported revision element".to_owned())
        })?;
        let id = required_word_attribute(start, b"id", prefixes)?.parse()?;
        let author = optional_word_attribute(start, b"author", prefixes)?;
        let author = match (author, require_author) {
            (Some(author), _) => author,
            (None, true) => required_word_attribute(start, b"author", prefixes)?,
            (None, false) => String::new(),
        };
        let timestamp = optional_word_attribute(start, b"date", prefixes)?;
        Ok(Self {
            kind,
            id,
            author,
            timestamp,
        })
    }
}

impl CT_Revision {
    /// Borrow the stored revision record without rebuilding its typed projection.
    #[doc(hidden)]
    pub fn retained_xml(&self) -> &[u8] {
        &self.raw_xml
    }

    pub(crate) fn from_raw(raw_xml: Vec<u8>, word_prefixes: &[String]) -> Option<Self> {
        Self::try_from_raw(raw_xml, word_prefixes, true, None).ok()
    }

    pub(crate) fn from_raw_with_input(
        raw_xml: Vec<u8>,
        word_prefixes: &[String],
        require_author: bool,
        input: Option<&crate::document::WordFieldInput>,
    ) -> crate::Result<Option<Self>> {
        Self::from_raw_with_read_output(raw_xml, word_prefixes, require_author, input, None)
    }

    pub(crate) fn from_raw_with_read_output(
        raw_xml: Vec<u8>,
        word_prefixes: &[String],
        require_author: bool,
        input: Option<&crate::document::WordFieldInput>,
        output: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> crate::Result<Option<Self>> {
        if let Some(error) = input.and_then(crate::document::WordFieldInput::resource_failure) {
            return Err(error);
        }
        let mut staged =
            (output.is_some() || input.is_some()).then(crate::text::WordBlockSourceMaps::default);
        let parsed = Self::try_from_raw_with_read_output(
            raw_xml,
            word_prefixes,
            require_author,
            input,
            staged.as_mut(),
        );
        if let Some(error) = input.and_then(crate::document::WordFieldInput::resource_failure) {
            return Err(error);
        }
        // Only the actual producer's classified receipt makes a checked error.
        // All ordinary constructor failures retain the old None policy.
        match parsed {
            Ok(mut revision) => {
                if let (Some(input), Some(staged)) = (input, staged.as_ref()) {
                    revision.attach_read_context(input, staged.clone());
                }
                if let (Some(output), Some(staged)) = (output, staged) {
                    *output = staged;
                }
                Ok(Some(revision))
            }
            Err(_) => Ok(None),
        }
    }

    /// Whether a preserved wrapper is admitted by the complete typed parser.
    #[doc(hidden)]
    pub fn story_raw_is_typed(raw_xml: &[u8], word_prefixes: &[String]) -> bool {
        Self::try_from_raw(raw_xml.to_vec(), word_prefixes, true, None).is_ok()
    }

    pub(crate) fn into_raw_xml(self) -> Vec<u8> {
        self.raw_xml
    }

    /// Original retained wrapper bytes still owned by this revision.
    pub(crate) fn retained_source_len(&self) -> usize {
        self.raw_xml.len()
    }

    fn try_from_raw(
        raw_xml: Vec<u8>,
        word_prefixes: &[String],
        require_author: bool,
        input: Option<&crate::document::WordFieldInput>,
    ) -> crate::Result<Self> {
        Self::try_from_raw_with_read_output(raw_xml, word_prefixes, require_author, input, None)
    }

    fn try_from_raw_with_read_output(
        raw_xml: Vec<u8>,
        word_prefixes: &[String],
        require_author: bool,
        input: Option<&crate::document::WordFieldInput>,
        output: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> crate::Result<Self> {
        validate_revision_nesting_depth(&raw_xml, word_prefixes)?;
        let mut reader = Reader::from_reader(raw_xml.as_slice());
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let (kind, prefixes) = loop {
            let event = reader.read_event_into(&mut buffer)?;
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(start) | Event::Empty(start) => {
                    let prefixes = word_prefixes_at(&start, word_prefixes)?;
                    let RevisionHeader {
                        kind,
                        id,
                        author,
                        timestamp,
                    } = RevisionHeader::read(&start, &prefixes, require_author)?;
                    if !empty {
                        break ((kind, id, author, timestamp), prefixes);
                    }
                    return Ok(Self {
                        kind,
                        id,
                        author,
                        timestamp,
                        raw_xml,
                        content: RevisionContent::Marker,
                        content_paragraph: None,
                        nested_revisions: Vec::new(),
                        parse_word_prefixes: word_prefixes.to_vec(),
                        require_author,
                        accepted_fields_pending: false,
                        accepted_payload_pending: false,
                        read_context: None,
                    });
                }
                Event::Eof => {
                    return Err(crate::OxmlError::MissingElement(
                        "revision element".to_owned(),
                    ));
                }
                _ => {}
            }
            buffer.clear();
        };
        let (kind, id, author, timestamp) = kind;
        let (content, content_paragraph, nested_revisions) = if matches!(
            RevisionBodyMode::selected(
                kind,
                RevisionBodyMode::without_source_probe(kind).is_none()
                    && crate::text::source_has_ordered_field_owner(&raw_xml, &prefixes)?
            ),
            RevisionBodyMode::Ordered
        ) {
            let copy_bound = crate::document::admit_revision_field_input(
                &raw_xml,
                word_prefixes,
                require_author,
                input.map(crate::document::WordFieldInput::registry),
            )?;
            let (paragraph, facts) =
                parse_revision_content_with_facts(&raw_xml, &prefixes, copy_bound, input, output)?;
            let content = if facts.runs.is_empty() {
                RevisionContent::Marker
            } else {
                RevisionContent::Runs(facts.runs)
            };
            (content, Some(Arc::new(paragraph)), facts.revisions)
        } else {
            let cursor = input.map(|input| input.cursor(&raw_xml)).transpose()?;
            let (content, nested_revisions) =
                parse_content(&mut reader, kind, &prefixes, cursor.as_ref())?;
            (content, None, nested_revisions)
        };
        Ok(Self {
            kind,
            id,
            author,
            timestamp,
            raw_xml,
            content,
            content_paragraph,
            nested_revisions,
            parse_word_prefixes: word_prefixes.to_vec(),
            require_author,
            accepted_fields_pending: false,
            accepted_payload_pending: false,
            read_context: None,
        })
    }

    fn attach_read_context(
        &mut self,
        input: &crate::document::WordFieldInput,
        maps: crate::text::WordBlockSourceMaps,
    ) {
        let mut raw_map = input.projection_parts().0.clone();
        raw_map.freeze_publication();
        self.read_context = Some(Arc::new(RevisionReadContext {
            registry: Arc::downgrade(input.registry()),
            raw_map,
            maps,
        }));
    }

    fn read_context_for(
        &self,
        work: &crate::text::WordOperationWork,
    ) -> Option<Arc<RevisionReadContext>> {
        let context = self.read_context.as_ref()?;
        let registry = context.registry.upgrade()?;
        Arc::ptr_eq(&registry, &work.field_origins).then(|| Arc::clone(context))
    }

    pub(crate) fn validate_template_install_context(
        &self,
        work: &crate::text::WordOperationWork,
        required: bool,
    ) -> crate::Result<()> {
        if self.read_context_for(work).is_none() && required {
            return Err(crate::OxmlError::InvalidValue(
                "revision has no successful scoped read".into(),
            ));
        }
        Ok(())
    }

    pub fn kind(&self) -> RevisionKind {
        self.kind
    }

    pub fn id(&self) -> i32 {
        self.id
    }

    pub fn author(&self) -> &str {
        &self.author
    }

    pub fn timestamp(&self) -> Option<&str> {
        self.timestamp.as_deref()
    }

    pub fn content(&self) -> &RevisionContent {
        &self.content
    }

    /// Return the paragraph projection for an insertion, when it has inline wrappers.
    #[doc(hidden)]
    pub fn content_paragraph(&self) -> Option<&CT_P> {
        self.content_paragraph.as_deref()
    }

    /// The content an insertion or a move destination adds to the accepted
    /// view, its simple fields included, as a reader sees it: the stored
    /// paragraph. A deletion, a move source or a marker adds none.
    #[doc(hidden)]
    pub fn accepted_content(&self) -> Option<&CT_P> {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return None;
        }
        self.content_paragraph.as_deref()
    }

    /// Mutate physical read-projection metadata without rewriting revision XML.
    /// Used on layout-only clones before selecting the accepted revision view.
    /// The original raw XML remains the serializer's authority.
    #[doc(hidden)]
    pub fn physical_content_paragraph_mut(&mut self) -> crate::Result<Option<&mut CT_P>> {
        if !matches!(
            self.kind,
            RevisionKind::Insertion
                | RevisionKind::Deletion
                | RevisionKind::MoveFrom
                | RevisionKind::MoveTo
        ) {
            return Ok(None);
        }
        if self.content_paragraph.is_none() {
            // This is the same successful revision reader's actual produced map.
            // Forward its live registry, never register the view as new ingress.
            let input = self.read_context.as_ref().and_then(|context| {
                context.registry.upgrade().map(|registry| {
                    crate::document::WordFieldInput::produced(context.raw_map.clone(), registry)
                })
            });
            self.content_paragraph = Some(Arc::new(parse_accepted_revision_content(
                &self.raw_xml,
                &self.parse_word_prefixes,
                input.as_ref(),
            )?));
        }
        self.read_context = None;
        Ok(self.content_paragraph.as_mut().map(Arc::make_mut))
    }

    /// Return nested revision wrappers at their direct-run boundaries.
    #[doc(hidden)]
    pub fn nested_revisions(&self) -> &[(usize, CT_Revision)] {
        &self.nested_revisions
    }

    pub(crate) fn append_accepted_run_paths(
        &self,
        prefix: &mut Vec<AcceptedRunPathSegment>,
        output: &mut Vec<AcceptedRunPath>,
    ) {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return;
        }
        if let Some(paragraph) = self.content_paragraph.as_deref() {
            super::text::append_accepted_paragraph_run_paths(paragraph, prefix, output);
        }
    }

    pub(crate) fn accepted_run_segments(&self, path: &[AcceptedRunPathSegment]) -> Option<&CT_R> {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return None;
        }
        self.content_paragraph
            .as_deref()?
            .accepted_run_segments(path)
    }

    pub(crate) fn template_text_mut(
        &mut self,
        path: &[AcceptedRunPathSegment],
        content: usize,
    ) -> Option<&mut String> {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return None;
        }
        // This read never makes a shared paragraph owned just to move text.
        let paragraph = Arc::get_mut(self.content_paragraph.as_mut()?)?;
        crate::placeholder::owned_paragraph_text_mut(paragraph, path, content)
    }

    pub(crate) fn accepted_run_segments_mut(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> Option<&mut CT_R> {
        self.read_context = None;
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return None;
        }
        self.accepted_fields_pending = true;
        Arc::make_mut(self.content_paragraph.as_mut()?).accepted_run_segments_mut(path)
    }

    pub(crate) fn ordered_paragraph_mut(&mut self) -> Option<&mut CT_P> {
        self.read_context = None;
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return None;
        }
        self.accepted_fields_pending = true;
        self.accepted_payload_pending = true;
        Some(Arc::make_mut(self.content_paragraph.as_mut()?))
    }

    pub(crate) fn replace_ordered_result_slot(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: Vec<CT_R>,
    ) -> crate::Result<bool> {
        self.read_context = None;
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return Ok(false);
        }
        let mut reader = Reader::from_reader(self.raw_xml.as_slice());
        let root = match reader.read_event()? {
            Event::Start(root) => root,
            _ => {
                return Err(crate::OxmlError::MissingElement(
                    "revision source root".into(),
                ));
            }
        };
        let local = crate::text::raw_namespace_declarations(&root)?;
        let mut context = crate::text::field_scope_bindings(&self.parse_word_prefixes);
        for (prefix, uri) in &local {
            context.retain(|(known, _)| *known != prefix);
            context.push((prefix, uri));
        }
        let Some(paragraph) = self.content_paragraph.as_mut().map(Arc::make_mut) else {
            return Ok(false);
        };
        self.accepted_fields_pending = true;
        self.accepted_payload_pending = true;
        paragraph.replace_ordered_result_slot(path, replacement, Some(&context))
    }

    pub(crate) fn normalize_pending_fields_in(
        &mut self,
        work: &mut crate::text::WordOperationWork,
    ) -> crate::Result<()> {
        let Some(paragraph) = self.content_paragraph.as_deref() else {
            return Ok(());
        };
        if !self.accepted_fields_pending
            && !self.accepted_payload_pending
            && !paragraph.has_changed_field_owners()
        {
            return Ok(());
        }
        if !self.accepted_payload_pending && !paragraph.has_changed_field_owners() {
            self.accepted_fields_pending = false;
            return Ok(());
        }
        let mut candidate = self.clone();
        Arc::make_mut(
            candidate
                .content_paragraph
                .as_mut()
                .expect("stored paragraph"),
        )
        .normalize_field_owners_in(work)?;
        candidate.refresh_accepted_content_in(work, None, false, None)?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn replace_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
    ) -> crate::Result<bool> {
        self.replace_accepted_run_segments_in(path, replacement, None, None, false)
    }

    pub(crate) fn replace_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
        work: Option<&mut crate::text::WordOperationWork>,
        emission: Option<&crate::text::FieldXmlEmission>,
        prepared_fields: bool,
    ) -> crate::Result<bool> {
        if !prepared_fields {
            return self.edit_accepted_paragraph_in(work, emission, |paragraph, work| {
                paragraph.replace_accepted_run_segments_in(path, replacement, work, emission, false)
            });
        }
        let work = work.ok_or_else(|| {
            crate::OxmlError::InvalidValue(
                "prepared Revision install requires its operation".into(),
            )
        })?;
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return Ok(false);
        }
        ensure_revision_word_prefix_is_mutable(&self.raw_xml)?;
        let mut candidate = self.clone();
        let Some(paragraph) = candidate.content_paragraph.as_mut().map(Arc::make_mut) else {
            return Ok(false);
        };
        if !paragraph.replace_accepted_run_segments_in(
            path,
            replacement,
            Some(work),
            emission,
            true,
        )? {
            return Ok(false);
        }
        candidate.refresh_accepted_content_in(
            work,
            emission,
            true,
            self.content_paragraph.as_deref(),
        )?;
        *self = candidate;
        Ok(true)
    }

    pub(crate) fn split_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        offset: usize,
        work: Option<&mut crate::text::WordOperationWork>,
        emission: Option<&crate::text::FieldXmlEmission>,
        edge: crate::text::SplitEdge,
    ) -> crate::Result<bool> {
        self.edit_accepted_paragraph_in(work, emission, |paragraph, work| {
            paragraph.split_accepted_run_segments_in(path, offset, work, emission, edge)
        })
    }

    /// Remove an accepted-view run of an insertion. Returns `None` for a
    /// stale path, and otherwise whether nothing is left in the wrapper, which
    /// is then left unchanged for its owner to remove. A move destination is
    /// refused, since its source would no longer match it.
    pub(crate) fn remove_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> crate::Result<Option<bool>> {
        if self.kind == RevisionKind::MoveTo {
            return Err(crate::OxmlError::InvalidValue(
                "it is part of a tracked move destination, whose source would no longer match it"
                    .to_owned(),
            ));
        }
        let mut outcome = None;
        self.edit_accepted_paragraph(|paragraph| {
            if !paragraph.remove_accepted_run_segments(path)? {
                return Ok(false);
            }
            let emptied = paragraph.has_no_children();
            outcome = Some(emptied);
            Ok(!emptied)
        })?;
        Ok(outcome)
    }

    /// The paragraph of an insertion or a move destination, when
    /// [`Self::edit_accepted_paragraph`] can write an edit of it back.
    pub(crate) fn editable_accepted_paragraph(&self) -> Option<&CT_P> {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo)
            || ensure_revision_word_prefix_is_mutable(&self.raw_xml).is_err()
        {
            return None;
        }
        self.content_paragraph.as_deref()
    }

    /// Apply `edit` to the paragraph of an insertion or a move destination
    /// and, when it reports a change, write its runs back inside the
    /// wrapper, whose start tag keeps its id, author and date. Returns false
    /// for any other revision.
    pub(crate) fn edit_accepted_paragraph(
        &mut self,
        edit: impl FnOnce(&mut CT_P) -> crate::Result<bool>,
    ) -> crate::Result<bool> {
        self.edit_accepted_paragraph_in(None, None, |paragraph, _| edit(paragraph))
    }

    pub(crate) fn edit_accepted_paragraph_in(
        &mut self,
        mut work: Option<&mut crate::text::WordOperationWork>,
        emission: Option<&crate::text::FieldXmlEmission>,
        edit: impl FnOnce(&mut CT_P, Option<&mut crate::text::WordOperationWork>) -> crate::Result<bool>,
    ) -> crate::Result<bool> {
        if !matches!(self.kind, RevisionKind::Insertion | RevisionKind::MoveTo) {
            return Ok(false);
        }
        ensure_revision_word_prefix_is_mutable(&self.raw_xml)?;
        let mut candidate = self.clone();
        candidate.read_context = None;
        let Some(paragraph) = candidate.content_paragraph.as_mut().map(Arc::make_mut) else {
            return Ok(false);
        };
        if !edit(paragraph, work.as_deref_mut())? {
            return Ok(false);
        }
        if let Some(work) = work {
            candidate.refresh_accepted_content_in(work, emission, true, None)?;
        } else {
            candidate.refresh_accepted_content()?;
        }
        *self = candidate;
        Ok(true)
    }

    /// Move the actual retained owners out of a discarded checked candidate.
    pub(crate) fn into_discard_parts(
        self,
    ) -> (RevisionContent, Option<Arc<CT_P>>, Vec<CT_Revision>) {
        (
            self.content,
            self.content_paragraph,
            self.nested_revisions
                .into_iter()
                .map(|(_, revision)| revision)
                .collect(),
        )
    }

    pub(crate) fn shared_content_paragraph(&self) -> Option<Arc<CT_P>> {
        self.content_paragraph.clone()
    }

    // The caller retained this exact fresh parser occurrence before identity
    // rebinding. Legacy caches were cloned from it by RevisionSourceFacts.
    // Transfer only those proven construction aliases, not matching text/IDs.
    pub(crate) fn synchronize_source_fact_aliases(
        &mut self,
        before: &Arc<CT_P>,
    ) -> crate::Result<()> {
        let Some(after) = &mut self.content_paragraph else {
            return Ok(());
        };
        if Arc::ptr_eq(before, after) {
            return Ok(());
        }
        let after = Arc::make_mut(after);
        if before.revisions.len() != after.revisions.len() {
            return Err(crate::OxmlError::InvalidValue(
                "revision source-fact occurrences changed".into(),
            ));
        }
        for ((old_run, old_raw, old), (new_run, new_raw, new)) in
            before.revisions.iter().zip(&mut after.revisions)
        {
            if (old_run, old_raw) != (new_run, new_raw) || old.raw_xml != new.raw_xml {
                return Err(crate::OxmlError::InvalidValue(
                    "revision source-fact physical slot changed".into(),
                ));
            }
            if let Some(source) = &old.content_paragraph {
                new.synchronize_source_fact_aliases(source)?;
            }
        }
        for (_, legacy) in &mut self.nested_revisions {
            let Some(source) = &legacy.content_paragraph else {
                continue;
            };
            let mut matches =
                before
                    .revisions
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, _, canonical))| {
                        canonical
                            .content_paragraph
                            .as_ref()
                            .is_some_and(|paragraph| Arc::ptr_eq(source, paragraph))
                    });
            let Some((index, (_, _, original))) = matches.next() else {
                continue;
            };
            if matches.next().is_some() || legacy.raw_xml != original.raw_xml {
                return Err(crate::OxmlError::InvalidValue(
                    "revision source-fact alias is ambiguous".into(),
                ));
            }
            legacy.adopt_source_fact_aliases(&after.revisions[index].2)?;
        }
        Ok(())
    }

    fn adopt_source_fact_aliases(&mut self, current: &Self) -> crate::Result<()> {
        if self.raw_xml != current.raw_xml
            || self.nested_revisions.len() != current.nested_revisions.len()
        {
            return Err(crate::OxmlError::InvalidValue(
                "revision source-fact cache occurrence changed".into(),
            ));
        }
        self.content_paragraph = current.content_paragraph.clone();
        for ((old_slot, old), (new_slot, new)) in self
            .nested_revisions
            .iter_mut()
            .zip(&current.nested_revisions)
        {
            if old_slot != new_slot {
                return Err(crate::OxmlError::InvalidValue(
                    "revision source-fact cache slot changed".into(),
                ));
            }
            old.adopt_source_fact_aliases(new)?;
        }
        Ok(())
    }

    fn refresh_accepted_content(&mut self) -> crate::Result<()> {
        let paragraph = self.content_paragraph.as_deref().ok_or_else(|| {
            crate::OxmlError::MissingElement("accepted revision paragraph".to_owned())
        })?;
        let mut work =
            crate::text::WordOperationWork::model_fields(paragraph.writer_field_roots())?;
        self.refresh_accepted_content_in(&mut work, None, false, None)
    }

    fn refresh_accepted_content_in(
        &mut self,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
        scoped: bool,
        clone_from: Option<&CT_P>,
    ) -> crate::Result<()> {
        let paragraph = self.content_paragraph.as_deref().ok_or_else(|| {
            crate::OxmlError::MissingElement("accepted revision paragraph".to_owned())
        })?;
        let mut reader = Reader::from_reader(self.raw_xml.as_slice());
        let own_prefixes = match reader.read_event()? {
            Event::Start(root) => word_prefixes_at(&root, &self.parse_word_prefixes)?,
            _ => {
                return Err(crate::OxmlError::MissingElement(
                    "revision source root".into(),
                ));
            }
        };
        let context = if scoped {
            self.read_context_for(work)
        } else {
            None
        };
        let (texts, raws) = if let Some(context) = &context {
            crate::document::returned_block_inputs(
                crate::document::WordOwner::Revision(self),
                &context.maps,
            )?
        } else {
            (Vec::new(), Vec::new())
        };
        let emit = |active_work: &mut crate::text::WordOperationWork| -> crate::Result<_> {
            let clone_lease = clone_from
                .map(|previous| {
                    crate::text::TemplateParagraphCloneLease::new(previous, paragraph, active_work)
                })
                .transpose()?
                .flatten();
            let (bytes, map) = if scoped {
                let (bytes, map) = crate::text::paragraph_content_mapped_in(
                    paragraph,
                    Some(&crate::text::field_scope_bindings(&own_prefixes)),
                    active_work,
                    None,
                    emission,
                    None,
                )?;
                (bytes, Some(map))
            } else {
                (
                    crate::text::ordered_paragraph_xml_in(
                        paragraph,
                        Some(&crate::text::field_scope_bindings(&own_prefixes)),
                        active_work,
                        None,
                    )?,
                    None,
                )
            };
            let raw_map = if scoped {
                Some(if let Some(context) = &context {
                    context.raw_map.clone()
                } else {
                    active_work.template_word_raw_map(&self.raw_xml)?
                })
            } else {
                None
            };
            drop(clone_lease);
            Ok((bytes, map, raw_map))
        };
        let (paragraph_xml, paragraph_map, original_map) = if context.is_some() {
            let lease = work.template_revision_read_context(&texts, &raws)?;
            emit(lease.work)?
        } else {
            emit(work)?
        };
        let (content_start, content_end) = revision_content_bounds(&self.raw_xml)?;
        let capacity = self
            .raw_xml
            .len()
            .checked_sub(content_end - content_start)
            .and_then(|bytes| bytes.checked_add(paragraph_xml.len()))
            .ok_or_else(|| {
                crate::OxmlError::InvalidValue("revision refresh source byte overflow".into())
            })?;
        if let Some(emission) = emission {
            emission.reserve_inner_xml(capacity)?;
        }
        let mut updated = Vec::with_capacity(capacity);
        updated.extend_from_slice(&self.raw_xml[..content_start]);
        updated.extend_from_slice(&paragraph_xml);
        updated.extend_from_slice(&self.raw_xml[content_end..]);
        let prefixes = self.parse_word_prefixes.clone();
        let require_author = self.require_author;
        let mut read_maps = crate::text::WordBlockSourceMaps::default();
        let mapped = if let (Some(body_map), Some(original_map)) = (paragraph_map, original_map) {
            let map = crate::document::WordSourceMap::concat(&[
                original_map.slice(0..content_start)?,
                body_map,
                original_map.slice(content_end..self.raw_xml.len())?,
            ])?;
            Some(crate::document::WordFieldInput::produced(
                map,
                Arc::clone(&work.field_origins),
            ))
        } else {
            None
        };
        let mut normalized = Self::try_from_raw_with_read_output(
            updated,
            &prefixes,
            require_author,
            mapped.as_ref(),
            mapped.as_ref().map(|_| &mut read_maps),
        )?;
        if let Some(mapped) = &mapped {
            normalized.attach_read_context(mapped, read_maps);
        }
        let parsed_source = normalized.content_paragraph.clone();
        if let Some(current) = normalized.content_paragraph.as_mut() {
            if scoped {
                crate::text::retain_ordered_field_source_identities_in(
                    paragraph,
                    Arc::make_mut(current),
                    Some(&work.field_origins),
                )?;
            } else {
                crate::text::retain_ordered_field_source_identities(
                    paragraph,
                    Arc::make_mut(current),
                )?;
            }
        }
        if let Some(source) = parsed_source {
            normalized.synchronize_source_fact_aliases(&source)?;
        }
        // Rebinding follows current source slots and does not create a new edit.
        normalized.accepted_fields_pending = false;
        normalized.accepted_payload_pending = false;
        *self = normalized;
        Ok(())
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut quick_xml::Writer<W>,
        role: crate::text::XmlWriteRole,
        _source_context: Option<&[(&str, &str)]>,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> crate::Result<()> {
        if role == crate::text::XmlWriteRole::Source {
            let _transport = work.retained_revision_transport(&self.raw_xml)?;
            let map = if work.template_word_capture().is_some() {
                self.read_context_for(work)
                    .map(|context| context.raw_map.clone())
            } else {
                None
            };
            let _mapped = map
                .map(|map| work.mapped_source_record(&self.raw_xml, map))
                .transpose()?;
            writer.get_mut().write_all(&self.raw_xml)?;
            Ok(())
        } else {
            let destination = crate::text::emitted_bindings();
            let bindings = destination.bindings_for(&self.raw_xml);
            let budget =
                crate::text::retained_owner_work(self.raw_xml.len(), &self.parse_word_prefixes, 0)?;
            let plan = crate::text::retained_root_plan(
                &self.raw_xml,
                &self.parse_word_prefixes,
                &bindings,
                budget,
            )?;
            let map = if work.template_word_capture().is_some() {
                Some(if let Some(context) = self.read_context_for(work) {
                    context.raw_map.clone()
                } else {
                    work.template_word_raw_map(&self.raw_xml)?
                })
            } else {
                None
            };
            let (produced, prepaid, mapped) = crate::text::emit_retained_roots_mapped_in(
                &self.raw_xml,
                plan,
                emission,
                map.as_ref(),
            )?;
            let _transport = emission
                .filter(|_| prepaid)
                .map(|emission| emission.prepaid_xml_transport(&produced))
                .transpose()?;
            let _mapped = mapped
                .map(|map| work.mapped_source_record(&produced, map))
                .transpose()?;
            writer.get_mut().write_all(&produced)?;
            Ok(())
        }
    }

    pub(crate) fn write_xml_with_role_override<W: std::io::Write>(
        &self,
        writer: &mut quick_xml::Writer<W>,
        foreign_word_namespace: Option<&str>,
        role: crate::text::XmlWriteRole,
        _source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> crate::Result<()> {
        if role == crate::text::XmlWriteRole::Source {
            return crate::text::write_raw_with_word_override_in(
                writer,
                &self.raw_xml,
                foreign_word_namespace,
                emission,
            );
        }

        let destination = crate::text::emitted_bindings();
        let mut bindings = destination.bindings_for(&self.raw_xml);
        if let Some(namespace) = foreign_word_namespace {
            bindings.retain(|(prefix, _)| *prefix != "w");
            bindings.push(("w", namespace));
        }
        let budget =
            crate::text::retained_owner_work(self.raw_xml.len(), &self.parse_word_prefixes, 0)?;
        let plan = crate::text::retained_root_plan(
            &self.raw_xml,
            &self.parse_word_prefixes,
            &bindings,
            budget,
        )?;
        let (produced, prepaid) =
            crate::text::emit_retained_roots_in(&self.raw_xml, plan, emission)?;
        let _transport = emission
            .filter(|_| prepaid)
            .map(|emission| emission.prepaid_xml_transport(&produced))
            .transpose()?;
        writer.get_mut().write_all(&produced)?;
        Ok(())
    }
}

fn validate_revision_nesting_depth(raw_xml: &[u8], word_prefixes: &[String]) -> crate::Result<()> {
    let mut reader = Reader::from_reader(raw_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scopes = vec![(Arc::new(word_prefixes.to_vec()), false)];
    let mut revision_depth = 0usize;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let inherited = &scopes.last().expect("root namespace scope").0;
                let prefixes = revision_scope_at(&start, inherited)?;
                let is_revision = revision_kind(start.name().as_ref(), &prefixes).is_some();
                if is_revision {
                    revision_depth = revision_depth_height(true, revision_depth);
                    if revision_depth_exceeds(revision_depth) {
                        return Err(crate::OxmlError::InvalidValue(
                            "revision nesting depth exceeds reader limit".to_owned(),
                        ));
                    }
                }
                scopes.push((prefixes, is_revision));
            }
            Event::Empty(start) => {
                let inherited = &scopes.last().expect("root namespace scope").0;
                let prefixes = revision_scope_at(&start, inherited)?;
                if revision_kind(start.name().as_ref(), &prefixes).is_some()
                    && revision_depth_exceeds(revision_depth_height(true, revision_depth))
                {
                    return Err(crate::OxmlError::InvalidValue(
                        "revision nesting depth exceeds reader limit".to_owned(),
                    ));
                }
            }
            Event::End(_) => {
                let (_, was_revision) = scopes.pop().expect("balanced namespace scope");
                if was_revision {
                    revision_depth -= 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    Ok(())
}

impl CT_Document {
    /// Return every valid modeled main-document revision in document order.
    pub fn revisions(&self) -> Vec<&CT_Revision> {
        let mut revisions = Vec::new();
        for content in &self.body.content {
            collect_body_content(content, &mut revisions);
        }
        if let Some(section) = &self.body.sect_pr {
            collect_section_properties(section, &mut revisions);
        }
        revisions
    }
}

fn collect_body_content<'a>(content: &'a BodyContent, revisions: &mut Vec<&'a CT_Revision>) {
    match content {
        BodyContent::Paragraph(paragraph) => collect_paragraph(paragraph, revisions),
        BodyContent::Table(table) => collect_table(table, revisions),
        BodyContent::ContentControl(control) => collect_control(control, revisions),
        BodyContent::RawXml(_) => {}
    }
}

fn collect_paragraph<'a>(paragraph: &'a CT_P, revisions: &mut Vec<&'a CT_Revision>) {
    if let Some(properties) = &paragraph.properties {
        if let Some(revision) = &properties.numbering_revision {
            revisions.push(revision);
        }
        if let Some(run_properties) = &properties.rpr {
            collect_run_properties(run_properties, revisions);
        }
        if let Some(section) = &properties.sect_pr {
            collect_section_properties(section, revisions);
        }
        if let Some(change) = &properties.change {
            revisions.push(change);
        }
    }

    for boundary in 0..=paragraph.runs.len() {
        let raw_count = paragraph
            .extra_xml
            .iter()
            .filter(|(at, _)| *at == boundary)
            .count();
        for raw_index in 0..=raw_count {
            for (_, _, _, control) in
                paragraph
                    .content_controls
                    .iter()
                    .filter(|(at, raw_before, _, _)| {
                        *at == boundary && (*raw_before).min(raw_count) == raw_index
                    })
            {
                collect_control(control, revisions);
            }
            for (_, _, revision) in paragraph.revisions.iter().filter(|(at, raw_before, _)| {
                *at == boundary
                    && hyperlink_revision_index(*raw_before).is_none()
                    && (*raw_before).min(raw_count) == raw_index
            }) {
                collect_revision(revision, revisions);
            }
            for (hyperlink_index, _hyperlink) in
                paragraph
                    .hyperlinks
                    .iter()
                    .enumerate()
                    .filter(|(_, hyperlink)| {
                        hyperlink.run_start == boundary
                            && hyperlink.run_end == boundary
                            && hyperlink.preserved_raw_before == Some(raw_index)
                    })
            {
                for (_, _, revision) in paragraph.revisions.iter().filter(|(at, slot, _)| {
                    *at == boundary && hyperlink_revision_index(*slot) == Some(hyperlink_index)
                }) {
                    collect_revision(revision, revisions);
                }
            }
        }
        for (_, _, revision) in paragraph.revisions.iter().filter(|(at, slot, _)| {
            *at == boundary
                && hyperlink_revision_index(*slot).is_some_and(|index| {
                    paragraph
                        .hyperlinks
                        .get(index)
                        .is_none_or(|hyperlink| hyperlink.preserved_raw_before.is_none())
                })
        }) {
            collect_revision(revision, revisions);
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            collect_run(run, revisions);
        }
    }
}

fn collect_run<'a>(run: &'a CT_R, revisions: &mut Vec<&'a CT_Revision>) {
    for content in &run.content {
        if let crate::text::RunContent::Field(field) = content {
            collect_field(field, revisions);
        }
    }
    if let Some(properties) = &run.properties {
        collect_run_properties(properties, revisions);
    }
}

fn collect_field<'a>(field: &'a crate::text::Field, revisions: &mut Vec<&'a CT_Revision>) {
    if let Some(paragraph) = field.stored_ordered_result() {
        collect_paragraph(paragraph, revisions);
    } else {
        for child in field.cached_fields_in_source_order() {
            collect_field(child, revisions);
        }
    }
}

fn collect_run_properties<'a>(properties: &'a CT_RPr, revisions: &mut Vec<&'a CT_Revision>) {
    revisions.extend(properties.revision_markers.iter());
    if let Some(change) = &properties.change {
        revisions.push(change);
    }
}

fn collect_section_properties<'a>(properties: &'a CT_SectPr, revisions: &mut Vec<&'a CT_Revision>) {
    if let Some(change) = &properties.change {
        revisions.push(change);
    }
}

fn collect_table<'a>(table: &'a CT_Tbl, revisions: &mut Vec<&'a CT_Revision>) {
    if let Some(change) = table
        .properties
        .as_ref()
        .and_then(|properties| properties.change.as_ref())
    {
        revisions.push(change);
    }
    for boundary in 0..=table.rows.len() {
        collect_controls_at_table_boundary(table, boundary, revisions);
        if let Some(row) = table.rows.get(boundary) {
            collect_row(row, revisions);
        }
    }
}

fn collect_controls_at_table_boundary<'a>(
    table: &'a CT_Tbl,
    boundary: usize,
    revisions: &mut Vec<&'a CT_Revision>,
) {
    let raw_count = table
        .extra_xml
        .iter()
        .filter(|(at, _)| *at == boundary)
        .count();
    for raw_index in 0..=raw_count {
        for (_, _, control) in table.content_controls.iter().filter(|(at, raw_before, _)| {
            *at == boundary && (*raw_before).min(raw_count) == raw_index
        }) {
            collect_control(control, revisions);
        }
    }
}

fn collect_row<'a>(row: &'a CT_Row, revisions: &mut Vec<&'a CT_Revision>) {
    if let Some(properties) = &row.properties {
        revisions.extend(properties.revision_markers.iter());
    }
    for boundary in 0..=row.cells.len() {
        let raw_count = row
            .extra_xml
            .iter()
            .filter(|(at, _)| *at == boundary)
            .count();
        for raw_index in 0..=raw_count {
            for (_, _, control) in row.content_controls.iter().filter(|(at, raw_before, _)| {
                *at == boundary && (*raw_before).min(raw_count) == raw_index
            }) {
                collect_control(control, revisions);
            }
        }
        if let Some(cell) = row.cells.get(boundary) {
            collect_cell(cell, revisions);
        }
    }
}

fn collect_cell<'a>(cell: &'a CT_Tc, revisions: &mut Vec<&'a CT_Revision>) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => collect_paragraph(paragraph, revisions),
            CellContent::Table(table) => collect_table(table, revisions),
            CellContent::ContentControl(control) => collect_control(control, revisions),
        }
    }
}

fn collect_control<'a>(control: &'a CT_Sdt, revisions: &mut Vec<&'a CT_Revision>) {
    for (content_index, content) in control.content.iter().enumerate() {
        for (_, revision) in control
            .revisions
            .iter()
            .filter(|(at, _)| *at == content_index)
        {
            collect_revision(revision, revisions);
        }
        match content {
            SdtContent::Paragraph(paragraph) => collect_paragraph(paragraph, revisions),
            SdtContent::Table(table) => collect_table(table, revisions),
            SdtContent::Row(row) => collect_row(row, revisions),
            SdtContent::Cell(cell) => collect_cell(cell, revisions),
            SdtContent::Run(run) => collect_run(run, revisions),
            SdtContent::ContentControl(control) => collect_control(control, revisions),
            SdtContent::RawXml(_) => {}
        }
    }
}

fn collect_revision<'a>(revision: &'a CT_Revision, revisions: &mut Vec<&'a CT_Revision>) {
    revisions.push(revision);
    if let Some(paragraph) = revision.content_paragraph() {
        collect_paragraph(paragraph, revisions);
        return;
    }
    if let RevisionContent::Runs(runs) = revision.content() {
        for boundary in 0..=runs.len() {
            for (_, nested) in revision
                .nested_revisions
                .iter()
                .filter(|(at, _)| *at == boundary)
            {
                collect_revision(nested, revisions);
            }
            if let Some(run) = runs.get(boundary) {
                collect_run(run, revisions);
            }
        }
    } else {
        for (_, nested) in &revision.nested_revisions {
            collect_revision(nested, revisions);
        }
    }
}

pub(crate) fn prepare_semantic_paragraph_projection(value: &mut CT_P) -> crate::Result<()> {
    for (_, _, revision) in &mut value.revisions {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for (_, _, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for run in &mut value.runs {
        prepare_semantic_run_projection(run)?;
    }
    Ok(())
}

fn prepare_semantic_run_projection(value: &mut CT_R) -> crate::Result<()> {
    for drawing in value
        .content
        .iter_mut()
        .filter_map(|content| {
            if let crate::text::RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .chain(value.alt_drawings.iter_mut())
    {
        let shape = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.shape.as_mut());
        if let Some(shape) = shape {
            if let Some(body) = &mut shape.text_body {
                prepare_semantic_body_projection(&mut body.content)?;
            } else {
                for paragraph in &mut shape.text {
                    prepare_semantic_paragraph_projection(paragraph)?;
                }
            }
        }
    }
    Ok(())
}

fn prepare_semantic_control_projection(value: &mut CT_Sdt) -> crate::Result<()> {
    for (_, revision) in value.physical_revisions_mut() {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for child in &mut value.content {
        match child {
            SdtContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            SdtContent::Table(t) => prepare_semantic_table_projection(t)?,
            SdtContent::Row(r) => prepare_semantic_row_projection(r)?,
            SdtContent::Cell(c) => prepare_semantic_cell_projection(c)?,
            SdtContent::Run(r) => prepare_semantic_run_projection(r)?,
            SdtContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn prepare_semantic_table_projection(value: &mut CT_Tbl) -> crate::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for row in &mut value.rows {
        prepare_semantic_row_projection(row)?;
    }
    Ok(())
}

fn prepare_semantic_row_projection(value: &mut CT_Row) -> crate::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for cell in &mut value.cells {
        prepare_semantic_cell_projection(cell)?;
    }
    Ok(())
}

fn prepare_semantic_cell_projection(value: &mut CT_Tc) -> crate::Result<()> {
    for content in &mut value.content {
        match content {
            CellContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            CellContent::Table(t) => prepare_semantic_table_projection(t)?,
            CellContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
        }
    }
    Ok(())
}

fn prepare_semantic_body_projection(values: &mut [BodyContent]) -> crate::Result<()> {
    for value in values {
        match value {
            BodyContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            BodyContent::Table(t) => prepare_semantic_table_projection(t)?,
            BodyContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            BodyContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn parse_accepted_revision_content(
    raw_xml: &[u8],
    word_prefixes: &[String],
    input: Option<&crate::document::WordFieldInput>,
) -> crate::Result<CT_P> {
    // The successful original revision reader owns these boundaries and scope.
    // This later semantic view is checked separately from its raw admission.
    let (content_start, content_end) = revision_content_bounds(raw_xml)?;
    let mut root_reader = Reader::from_reader(raw_xml);
    let prefixes = match root_reader.read_event()? {
        Event::Start(root) => word_prefixes_at(&root, word_prefixes)?,
        _ => {
            return Err(crate::OxmlError::MissingElement(
                "revision source root".into(),
            ));
        }
    };
    let bindings = crate::text::field_scope_bindings(&prefixes);
    let prefix = bindings
        .iter()
        .find_map(|(prefix, uri)| (*uri == W_NS).then_some(*prefix))
        .ok_or_else(|| crate::OxmlError::MissingElement("revision Word namespace".into()))?;
    let name = if prefix.is_empty() {
        "p".to_owned()
    } else {
        format!("{prefix}:p")
    };
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let chunks = [
        opening.as_bytes(),
        &raw_xml[content_start..content_end],
        closing.as_bytes(),
    ];
    // Same actual paragraph and field constructor roles, before payload growth.
    crate::document::admit_word_field_input(
        crate::document::WordXmlInput::pieces(&chunks),
        &prefixes,
        input.map(crate::document::WordFieldInput::registry),
    )?;
    let paragraph_input = input
        .map(|input| {
            input
                .slice(content_start..content_end)?
                .with_shell(opening.len(), closing.len())
        })
        .transpose()?;
    let bytes = chunks
        .iter()
        .try_fold(0usize, |sum, chunk| sum.checked_add(chunk.len()))
        .ok_or_else(|| {
            crate::OxmlError::InvalidValue("semantic revision source overflow".into())
        })?;
    let mut paragraph_xml = Vec::with_capacity(bytes);
    for chunk in chunks {
        paragraph_xml.extend_from_slice(chunk);
    }
    let mut reader = Reader::from_reader(paragraph_xml.as_slice());
    let start = match reader.read_event()? {
        Event::Start(start) => start,
        _ => {
            return Err(crate::OxmlError::MissingElement(
                "semantic revision paragraph".into(),
            ));
        }
    };
    let mut cursor = paragraph_input
        .as_ref()
        .map(|input| input.cursor(&paragraph_xml))
        .transpose()?;
    let mut paragraph = CT_P::from_xml_with_revision_facts(
        &mut reader,
        &prefixes,
        Some(&start),
        None,
        cursor.as_mut(),
        None,
        None,
    )?;
    prepare_semantic_paragraph_projection(&mut paragraph)?;
    Ok(paragraph)
}

fn parse_revision_content_with_facts(
    raw_xml: &[u8],
    word_prefixes: &[String],
    copy_bound: (usize, usize),
    input: Option<&crate::document::WordFieldInput>,
    output: Option<&mut crate::text::WordBlockSourceMaps>,
) -> crate::Result<(CT_P, RevisionSourceFacts)> {
    let (content_start, content_end) = revision_content_bounds(raw_xml)?;
    let paragraph_input = input
        .map(|input| input.slice(content_start..content_end)?.with_shell(5, 6))
        .transpose()?;
    let mut paragraph_xml = Vec::from(b"<w:p>".as_slice());
    paragraph_xml.extend_from_slice(&raw_xml[content_start..content_end]);
    paragraph_xml.extend_from_slice(b"</w:p>");
    let mut paragraph_controls = paragraph_input
        .as_ref()
        .map(|input| input.cursor(&paragraph_xml))
        .transpose()?;
    let mut paragraph_reader = Reader::from_reader(paragraph_xml.as_slice());
    let mut paragraph_buffer = Vec::new();
    match paragraph_reader.read_event_into(&mut paragraph_buffer)? {
        Event::Start(start) => {
            let mut facts = RevisionSourceFacts::with_copy_bound(copy_bound);
            let mut extras = Vec::new();
            let mut texts = Vec::new();
            let mut nested = crate::text::WordBlockSourceMaps::default();
            let paragraph = CT_P::from_xml_with_read_output(
                &mut paragraph_reader,
                word_prefixes,
                Some(&start),
                paragraph_controls.as_mut(),
                crate::text::WordParagraphReadOutput {
                    source_facts: Some(&mut facts),
                    extra_maps: output.as_ref().map(|_| &mut extras),
                    text_facts: output.as_ref().map(|_| &mut texts),
                    nested_maps: output.as_ref().map(|_| &mut nested),
                    ..Default::default()
                },
            )?;
            if let (Some(output), Some(input)) = (output, paragraph_input.as_ref()) {
                nested.paragraph(extras);
                nested.word_texts.push((
                    Vec::new(),
                    Some(crate::text::WordParagraphReadFacts {
                        source_map: Arc::new(input.projection_parts().0.clone()),
                        texts,
                        nested: Default::default(),
                    }),
                ));
                output.append(crate::text::WordBlockComponent::RevisionParagraph, nested);
            }
            Ok((paragraph, facts))
        }
        _ => Err(crate::OxmlError::MissingElement(
            "insertion content".to_owned(),
        )),
    }
}

fn revision_content_bounds(raw_xml: &[u8]) -> crate::Result<(usize, usize)> {
    let mut reader = Reader::from_reader(raw_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let content_start = loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) => break reader.buffer_position() as usize,
            Event::Eof => {
                return Err(crate::OxmlError::MissingElement(
                    "insertion element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };
    let content_end = raw_xml
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, byte)| (*byte == b'<').then_some(index))
        .ok_or_else(|| crate::OxmlError::MissingElement("insertion close tag".to_owned()))?;
    Ok((content_start, content_end))
}

fn ensure_revision_word_prefix_is_mutable(raw_xml: &[u8]) -> crate::Result<()> {
    let mut reader = Reader::from_reader(raw_xml);
    let mut buffer = Vec::new();
    let root = match reader.read_event_into(&mut buffer)? {
        Event::Start(root) => root,
        _ => {
            return Err(crate::OxmlError::MissingElement(
                "revision start".to_owned(),
            ));
        }
    };
    for attribute in root.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() != b"xmlns:w" {
            continue;
        }
        let namespace =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())?;
        if namespace.as_ref() != W_NS {
            return Err(crate::OxmlError::InvalidValue(
                "revision binds the reserved w prefix to a foreign namespace".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn revision_kind(name: &[u8], prefixes: &[String]) -> Option<RevisionKind> {
    let local = match name.iter().position(|byte| *byte == b':') {
        Some(colon) => &name[colon + 1..],
        None => name,
    };
    is_word_element(name, local, prefixes)
        .then(|| revision_kind_for_local(local))
        .flatten()
}

/// Whether preserved XML is a tracked insertion, deletion or move, by the
/// local name of its root element: a preserved child does not always declare
/// the prefix it was read with, and one the reader could not type (without
/// an author, say) counts too.
#[doc(hidden)]
pub fn raw_changes_content(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                return revision_kind_for_local(element.local_name().as_ref())
                    .is_some_and(RevisionKind::changes_content);
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

pub(crate) fn revision_kind_for_local(local: &[u8]) -> Option<RevisionKind> {
    [
        (b"ins".as_slice(), RevisionKind::Insertion),
        (b"del".as_slice(), RevisionKind::Deletion),
        (b"moveFrom".as_slice(), RevisionKind::MoveFrom),
        (b"moveTo".as_slice(), RevisionKind::MoveTo),
        (b"rPrChange".as_slice(), RevisionKind::RunPropertyChange),
        (
            b"pPrChange".as_slice(),
            RevisionKind::ParagraphPropertyChange,
        ),
        (b"tblPrChange".as_slice(), RevisionKind::TablePropertyChange),
        (
            b"sectPrChange".as_slice(),
            RevisionKind::SectionPropertyChange,
        ),
    ]
    .into_iter()
    .find_map(|(name, kind)| (name == local).then_some(kind))
}

fn parse_content(
    reader: &mut Reader<&[u8]>,
    kind: RevisionKind,
    word_prefixes: &[String],
    cursor: Option<&crate::document::ControlSourceCursor<'_>>,
) -> crate::Result<(RevisionContent, Vec<(usize, CT_Revision)>)> {
    let mut runs = Vec::new();
    let mut nested_revisions = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let event_start = reader.buffer_position();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                let name = start.name();
                let local = start.local_name();
                let action = RevisionContentAction::for_names(
                    Some(kind),
                    local.as_ref(),
                    is_word_element(name.as_ref(), local.as_ref(), &prefixes),
                    false,
                );
                if action == RevisionContentAction::Run {
                    runs.push(CT_R::from_xml_with_prefixes_and_root_in(
                        reader,
                        &prefixes,
                        Some(&start),
                        None,
                        cursor,
                    )?);
                } else if action == RevisionContentAction::NestedRevision {
                    let raw = crate::raw_xml::capture_element(reader, &start)?;
                    let mapped = cursor.and_then(|cursor| {
                        cursor.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    if let Some(revision) =
                        CT_Revision::from_raw_with_input(raw, &prefixes, true, mapped.as_ref())?
                    {
                        nested_revisions.push((runs.len(), revision));
                    }
                } else if action == RevisionContentAction::Hyperlink {
                    parse_run_content_container(
                        reader,
                        b"hyperlink",
                        &prefixes,
                        &mut runs,
                        &mut nested_revisions,
                        cursor,
                    )?;
                } else if action == RevisionContentAction::PriorRunProperties {
                    let raw = crate::raw_xml::capture_element(reader, &start)?;
                    return Ok((
                        RevisionContent::PriorRunProperties(Box::new(parse_scoped_rpr(
                            &raw,
                            word_prefixes,
                        )?)),
                        nested_revisions,
                    ));
                } else if action == RevisionContentAction::PriorParagraphProperties {
                    let raw = crate::raw_xml::capture_element(reader, &start)?;
                    return Ok((
                        RevisionContent::PriorParagraphProperties(Box::new(parse_scoped_ppr(
                            &raw,
                            word_prefixes,
                        )?)),
                        nested_revisions,
                    ));
                } else if action == RevisionContentAction::PriorTableProperties {
                    let owner_bindings =
                        crate::numbering::local_namespace_overrides(&start, word_prefixes)?;
                    return Ok((
                        RevisionContent::PriorTableProperties(Box::new(
                            CT_TblPr::from_xml_with_prefixes_and_owner_bindings(
                                reader,
                                &prefixes,
                                &owner_bindings,
                            )?,
                        )),
                        nested_revisions,
                    ));
                } else if action == RevisionContentAction::PriorSectionProperties {
                    let owner_bindings =
                        crate::numbering::local_namespace_overrides(&start, word_prefixes)?;
                    return Ok((
                        RevisionContent::PriorSectionProperties(Box::new(
                            CT_SectPr::from_xml_with_prefixes_owner_bindings_and_root(
                                reader,
                                &prefixes,
                                &owner_bindings,
                                Some(&start),
                            )?,
                        )),
                        nested_revisions,
                    ));
                } else {
                    reader.read_to_end_into(name, &mut Vec::new())?;
                }
            }
            Event::Empty(start) => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                let local = start.local_name();
                if RevisionContentAction::for_names(
                    Some(kind),
                    local.as_ref(),
                    is_word_element(start.name().as_ref(), local.as_ref(), &prefixes),
                    true,
                ) == RevisionContentAction::NestedRevision
                {
                    let raw = capture_empty_element(&start)?;
                    let mapped = cursor.and_then(|cursor| {
                        cursor.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    if let Some(revision) =
                        CT_Revision::from_raw_with_input(raw, &prefixes, true, mapped.as_ref())?
                    {
                        nested_revisions.push((runs.len(), revision));
                    }
                }
            }
            Event::End(_) | Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if runs.is_empty() {
        Ok((RevisionContent::Marker, nested_revisions))
    } else {
        Ok((RevisionContent::Runs(runs), nested_revisions))
    }
}

struct RunContainerFrame {
    closing_local: Vec<u8>,
    prefixes: Arc<Vec<String>>,
}

// No declaration means the actual decoder would retain exactly this scope.
// Stop at the first declaration so its decoder/error order stays authoritative.
fn revision_scope_at(
    start: &quick_xml::events::BytesStart<'_>,
    inherited: &Arc<Vec<String>>,
) -> crate::Result<Arc<Vec<String>>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            return Ok(Arc::new(word_prefixes_at(start, inherited)?.into_owned()));
        }
    }
    Ok(Arc::clone(inherited))
}

fn parse_run_content_container(
    reader: &mut Reader<&[u8]>,
    closing_local: &[u8],
    word_prefixes: &[String],
    runs: &mut Vec<CT_R>,
    nested_revisions: &mut Vec<(usize, CT_Revision)>,
    cursor: Option<&crate::document::ControlSourceCursor<'_>>,
) -> crate::Result<()> {
    let mut frames = vec![RunContainerFrame {
        closing_local: closing_local.to_vec(),
        prefixes: Arc::new(word_prefixes.to_vec()),
    }];
    let mut buffer = Vec::new();
    loop {
        let event_start = reader.buffer_position();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let prefixes =
                    revision_scope_at(&start, &frames.last().expect("container frame").prefixes)?;
                let name = start.name();
                let local = start.local_name();
                let action = RevisionContentAction::for_names(
                    None,
                    local.as_ref(),
                    is_word_element(name.as_ref(), local.as_ref(), &prefixes),
                    false,
                );
                if action == RevisionContentAction::Run {
                    runs.push(CT_R::from_xml_with_prefixes_and_root_in(
                        reader,
                        &prefixes,
                        Some(&start),
                        None,
                        cursor,
                    )?);
                } else if action == RevisionContentAction::NestedRevision {
                    let raw = crate::raw_xml::capture_element(reader, &start)?;
                    let mapped = cursor.and_then(|cursor| {
                        cursor.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    if let Some(revision) =
                        CT_Revision::from_raw_with_input(raw, &prefixes, true, mapped.as_ref())?
                    {
                        nested_revisions.push((runs.len(), revision));
                    }
                } else if action == RevisionContentAction::Hyperlink {
                    frames.push(RunContainerFrame {
                        closing_local: b"hyperlink".to_vec(),
                        prefixes,
                    });
                } else {
                    reader.read_to_end_into(name, &mut Vec::new())?;
                }
            }
            Event::Empty(start) => {
                let prefixes =
                    revision_scope_at(&start, &frames.last().expect("container frame").prefixes)?;
                let local = start.local_name();
                if RevisionContentAction::for_names(
                    None,
                    local.as_ref(),
                    is_word_element(start.name().as_ref(), local.as_ref(), &prefixes),
                    true,
                ) == RevisionContentAction::NestedRevision
                {
                    let raw = capture_empty_element(&start)?;
                    let mapped = cursor.and_then(|cursor| {
                        cursor.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    if let Some(revision) =
                        CT_Revision::from_raw_with_input(raw, &prefixes, true, mapped.as_ref())?
                    {
                        nested_revisions.push((runs.len(), revision));
                    }
                }
            }
            Event::End(end)
                if frames.last().is_some_and(|frame| {
                    is_word_element(end.name().as_ref(), &frame.closing_local, &frame.prefixes)
                }) =>
            {
                frames.pop();
                if frames.is_empty() {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn required_word_attribute(
    start: &quick_xml::events::BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> crate::Result<String> {
    optional_word_attribute(start, local, prefixes)?.ok_or_else(|| {
        crate::OxmlError::MissingElement(format!("w:{} attribute", String::from_utf8_lossy(local)))
    })
}

fn optional_word_attribute(
    start: &quick_xml::events::BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> crate::Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if key.get(separator + 1..) == Some(local)
            && prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &key[..separator])
        {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inserted_ruby_in_a_run_holds_its_base_runs() {
        let xml = format!(
            concat!(
                r#"<w:document xmlns:w="{w}"><w:body><w:p>"#,
                r#"<w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:ruby>"#,
                r#"<w:rt><w:r><w:t>grnd</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>Ground</w:t></w:r>"#,
                r#"</w:rubyBase></w:ruby></w:r></w:ins></w:p></w:body></w:document>"#,
            ),
            w = crate::namespace::W_NS,
        );
        let document = crate::CT_Document::from_xml(xml.as_bytes()).unwrap();
        let Some(crate::document::BodyContent::Paragraph(paragraph)) =
            document.body.content.first()
        else {
            panic!("one paragraph");
        };
        let revision = &paragraph.revisions.first().expect("one revision").2;
        let RevisionContent::Runs(runs) = revision.content() else {
            panic!("the insertion holds runs");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text(), "Ground");
    }

    /// The revision of a one-paragraph body whose root binds `x` to `uri`.
    fn revision(uri: &str, choice: &str) -> CT_Revision {
        let xml = format!(
            concat!(
                r#"<w:document xmlns:w="{w}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x="{uri}"><w:body>"#,
                r#"<w:p><w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r>"#,
                r#"<mc:AlternateContent><mc:Choice Requires="{choice}"><w:t>a</w:t></mc:Choice>"#,
                r#"<mc:Fallback><w:t>b</w:t></mc:Fallback></mc:AlternateContent></w:r></w:ins></w:p>"#,
                r#"</w:body></w:document>"#,
            ),
            w = crate::namespace::W_NS,
            uri = uri,
            choice = choice,
        );
        let document = crate::CT_Document::from_xml(xml.as_bytes()).unwrap();
        let Some(crate::document::BodyContent::Paragraph(paragraph)) =
            document.body.content.first()
        else {
            panic!("one paragraph");
        };
        paragraph.revisions.first().expect("one revision").2.clone()
    }

    #[test]
    fn a_revision_is_read_under_the_bindings_it_names_only() {
        // A binding the revision never names is the part's, not the revision's.
        assert_eq!(revision("urn:a", "mc"), revision("urn:b", "mc"));
        // A prefix named in a markup-compatibility value is read.
        assert_ne!(revision("urn:a", "x"), revision("urn:b", "x"));
    }
}
