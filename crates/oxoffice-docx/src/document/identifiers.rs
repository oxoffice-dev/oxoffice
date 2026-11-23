//! The identifier registry: ids a document holds, reserving new ones and part identities.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdentifierDefinition {
    Drawing(u32),
    Bookmark(i32),
    Comment(i32),
    AbstractNumbering(u32),
    NumberingInstance(u32),
}

/// What an in-place edit added to the identifier registry and the package
/// graph, in order, so that a refusal removes it again instead of putting
/// back copies of both. It records the reservations of the part bundles,
/// relationship ids, part names, content types and numbering ids that
/// [`Document::edit_properties`] and the numbering edits make.
#[derive(Debug, Default)]
pub(crate) struct ReservationJournal {
    entries: Vec<Reservation>,
    /// Whether the edit removed a part, its relationships or its content
    /// type from the package, which undoing it cannot put back: such an
    /// edit cannot refuse after the removal.
    removed_from_package: bool,
}

#[derive(Debug)]
enum Reservation {
    PartName(String),
    PreservedPartName(String),
    ContentTypeDefault(String),
    ContentTypeOverride(String),
    RelationshipOwner(String),
    RelationshipId {
        owner: String,
        id: String,
    },
    AbstractNumberingId(u32),
    NumberingInstanceId(u32),
    /// The override stored under `stored`, and the case variants it
    /// replaced with their content types.
    PackageOverride {
        stored: String,
        replaced: Vec<(String, String)>,
    },
    PackageRelationships(String),
    /// A relationship of `owner`'s part, or of the package when `None`,
    /// added when the relationships' counter was `next_id`.
    PackageRelationship {
        owner: Option<String>,
        id: String,
        next_id: u32,
    },
}

impl ReservationJournal {
    /// Whether the edit changed the package graph.
    pub(crate) fn changed_package(&self) -> bool {
        self.removed_from_package
            || self.entries.iter().any(|entry| {
                matches!(
                    entry,
                    Reservation::PackageOverride { .. }
                        | Reservation::PackageRelationships(_)
                        | Reservation::PackageRelationship { .. }
                )
            })
    }
}

/// The journal open on a registry, if any. A copy of the registry starts
/// without one, and two registries compare equal whatever their journals:
/// the journal is not something the registry holds.
#[derive(Debug, Default)]
pub(crate) struct JournalSlot(Option<ReservationJournal>);

impl Clone for JournalSlot {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl PartialEq for JournalSlot {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for JournalSlot {}

impl DocumentIdentifiers {
    /// Start recording what the registry and the package graph gain.
    pub(crate) fn open_journal(&mut self) {
        debug_assert!(self.journal.0.is_none(), "a reservation journal is open");
        self.journal.0 = Some(ReservationJournal::default());
    }

    /// Stop recording and return what was recorded.
    pub(crate) fn close_journal(&mut self) -> ReservationJournal {
        let journal = self.journal.0.take();
        debug_assert!(journal.is_some(), "no reservation journal is open");
        journal.unwrap_or_default()
    }

    fn record(&mut self, entry: impl FnOnce() -> Reservation) {
        if let Some(journal) = &mut self.journal.0 {
            journal.entries.push(entry());
        }
    }

    pub(crate) fn record_removed_from_package(&mut self) {
        if let Some(journal) = &mut self.journal.0 {
            journal.removed_from_package = true;
        }
    }

    fn hold_part_name(&mut self, identity: String) -> bool {
        if self.part_names.contains(&identity) {
            return false;
        }
        self.record(|| Reservation::PartName(identity.clone()));
        self.part_names.insert(identity)
    }

    fn hold_relationship_id(&mut self, owner_identity: &str, id: &str) -> bool {
        if !self.relationship_ids.contains_key(owner_identity) {
            self.relationship_ids
                .insert(owner_identity.to_owned(), HashSet::new());
            self.record(|| Reservation::RelationshipOwner(owner_identity.to_owned()));
        }
        let added = self
            .relationship_ids
            .get_mut(owner_identity)
            .expect("the owner was just held")
            .insert(id.to_owned());
        if added {
            self.record(|| Reservation::RelationshipId {
                owner: owner_identity.to_owned(),
                id: id.to_owned(),
            });
        }
        added
    }

    pub(super) fn hold_abstract_numbering_id(&mut self, id: u32) {
        if self.abstract_numbering_ids.insert(id) {
            self.record(|| Reservation::AbstractNumberingId(id));
        }
    }

    pub(super) fn hold_numbering_instance_id(&mut self, id: u32) {
        if self.numbering_instance_ids.insert(id) {
            self.record(|| Reservation::NumberingInstanceId(id));
        }
    }
}

impl Document {
    /// [`ContentTypes::add_override`], recorded in the open journal.
    ///
    /// [`ContentTypes::add_override`]: oxoffice_opc::ContentTypes::add_override
    pub(crate) fn add_override_journaled(&mut self, part_name: &str, content_type: &str) {
        let overrides = &mut self.package.content_types.overrides;
        let replaced = overrides
            .iter()
            .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(part_name))
            .map(|(candidate, value)| (candidate.clone(), value.clone()))
            .collect::<Vec<_>>();
        self.package
            .content_types
            .add_override(part_name, content_type);
        let unchanged = matches!(replaced.as_slice(), [(_, value)] if value == content_type);
        if !unchanged {
            let stored = self
                .package
                .content_types
                .overrides
                .keys()
                .find(|candidate| candidate.eq_ignore_ascii_case(part_name))
                .expect("the override was just added")
                .clone();
            self.identifiers
                .record(|| Reservation::PackageOverride { stored, replaced });
        }
    }

    /// Add relationship `id` to `owner`'s part, or to the package when
    /// `owner` is `None`, recorded in the open journal.
    pub(crate) fn add_relationship_journaled(
        &mut self,
        owner: Option<&str>,
        id: &str,
        rel_type: &str,
        target: &str,
    ) {
        let relationships = match owner {
            None => &mut self.package.package_rels,
            Some(owner) => {
                if self.package.get_part_rels(owner).is_none() {
                    self.identifiers
                        .record(|| Reservation::PackageRelationships(owner.to_owned()));
                }
                self.package.get_or_create_part_rels(owner)
            }
        };
        debug_assert!(
            relationships.items.iter().all(|item| item.id != id),
            "a reserved relationship id is already in the package"
        );
        let next_id = relationships.next_id();
        relationships.add_with_id(id, rel_type, target);
        self.identifiers
            .record(|| Reservation::PackageRelationship {
                owner: owner.map(str::to_owned),
                id: id.to_owned(),
                next_id,
            });
    }

    /// Remove, latest first, what `journal` recorded, leaving the registry
    /// and the package graph as they were when it was opened. Valid only
    /// before another edit reserves anything.
    pub(crate) fn undo_reservations(&mut self, journal: ReservationJournal) {
        debug_assert!(
            !journal.removed_from_package,
            "a journaled edit refused after removing a package part"
        );
        let identifiers = &mut self.identifiers;
        for entry in journal.entries.into_iter().rev() {
            match entry {
                Reservation::PartName(identity) => {
                    identifiers.part_names.remove(&identity);
                }
                Reservation::PreservedPartName(identity) => {
                    identifiers.preserved_part_names.remove(&identity);
                }
                Reservation::ContentTypeDefault(extension) => {
                    identifiers.content_type_defaults.remove(&extension);
                }
                Reservation::ContentTypeOverride(part_name) => {
                    identifiers.content_type_overrides.remove(&part_name);
                }
                Reservation::RelationshipOwner(owner) => {
                    identifiers.relationship_ids.remove(&owner);
                }
                Reservation::RelationshipId { owner, id } => {
                    if let Some(occupied) = identifiers.relationship_ids.get_mut(&owner) {
                        occupied.remove(&id);
                    }
                }
                Reservation::AbstractNumberingId(id) => {
                    identifiers.abstract_numbering_ids.remove(&id);
                }
                Reservation::NumberingInstanceId(id) => {
                    identifiers.numbering_instance_ids.remove(&id);
                }
                Reservation::PackageOverride { stored, replaced } => {
                    let overrides = &mut self.package.content_types.overrides;
                    overrides.remove(&stored);
                    overrides.extend(replaced);
                }
                Reservation::PackageRelationships(owner) => {
                    self.package.remove_part_rels(&owner);
                }
                Reservation::PackageRelationship { owner, id, next_id } => {
                    let relationships = match &owner {
                        None => Some(&mut self.package.package_rels),
                        Some(owner) => self.package.get_part_rels_mut(owner),
                    };
                    if let Some(relationships) = relationships {
                        relationships.undo_add_with_id(&id, next_id);
                    }
                }
            }
        }
    }
}

impl DocumentIdentifiers {
    pub(super) fn scan(package: &OpcPackage) -> Result<Self> {
        Self::scan_reusing(package, None, None)
    }

    pub(super) fn scan_reusing(
        package: &OpcPackage,
        earlier: Option<&Self>,
        hinted_main: Option<&ScannedIdentifierPart>,
    ) -> Result<Self> {
        let mut identifiers = Self {
            relationship_ids: HashMap::new(),
            bookmark_ids: HashSet::new(),
            repeated_bookmark_ids: HashMap::new(),
            comment_ids: HashSet::new(),
            drawing_ids: HashSet::new(),
            repeated_drawing_ids: HashMap::new(),
            abstract_numbering_ids: HashSet::new(),
            numbering_instance_ids: HashSet::new(),
            part_names: package
                .parts
                .keys()
                .map(|part_name| part_name_identity(part_name))
                .collect(),
            content_type_defaults: package
                .content_types
                .defaults
                .keys()
                .map(|extension| extension.to_ascii_lowercase())
                .collect(),
            content_type_overrides: package
                .content_types
                .overrides
                .keys()
                .map(|part_name| part_name.to_ascii_lowercase())
                .collect(),
            authored_content_type_defaults: HashSet::new(),
            preserved_relationship_ids: HashMap::new(),
            preserved_comment_ids: HashSet::new(),
            preserved_drawing_ids: HashSet::new(),
            preserved_abstract_numbering_ids: HashSet::new(),
            preserved_numbering_instance_ids: HashSet::new(),
            preserved_part_names: HashSet::new(),
            authored_comment_ids: HashSet::new(),
            authored_toc_bookmark_ids: HashSet::new(),
            authored_bookmark_names: HashSet::new(),
            authored_story_parts: HashSet::new(),
            authored_story_relationship_ids: HashMap::new(),
            authored_nested_story_relationship_ids: HashMap::new(),
            scanned_parts: Arc::default(),
            journal: JournalSlot::default(),
        };
        identifiers.part_names.extend(
            package
                .part_rels
                .keys()
                .map(|name| part_name_identity(name)),
        );
        identifiers.part_names.extend(
            package
                .content_types
                .overrides
                .keys()
                .map(|name| part_name_identity(name)),
        );

        identifiers.scan_relationships("/", &package.package_rels.items)?;
        let mut relationship_owners = package.part_rels.iter().collect::<Vec<_>>();
        relationship_owners.sort_by_key(|(owner, _)| owner.as_str());
        for (owner, relationships) in relationship_owners {
            identifiers.scan_relationships(owner, &relationships.items)?;
        }
        let mut identity_parts: HashMap<String, HashSet<IdentifierXmlCategory>> = HashMap::new();
        if let Some(main_part) = package.main_document_part() {
            identity_parts
                .entry(part_name_identity(&main_part))
                .or_default()
                .extend([
                    IdentifierXmlCategory::Bookmark,
                    IdentifierXmlCategory::Drawing,
                ]);
            if let Some(relationships) = package.get_part_rels(&main_part) {
                for relationship in &relationships.items {
                    let categories: &[IdentifierXmlCategory] = match relationship.rel_type.as_str()
                    {
                        rel_types::HEADER
                        | rel_types::FOOTER
                        | rel_types::FOOTNOTES
                        | rel_types::ENDNOTES
                        | rel_types::GLOSSARY_DOCUMENT => &[
                            IdentifierXmlCategory::Bookmark,
                            IdentifierXmlCategory::Drawing,
                        ],
                        rel_types::COMMENTS => &[
                            IdentifierXmlCategory::Bookmark,
                            IdentifierXmlCategory::Comment,
                            IdentifierXmlCategory::Drawing,
                        ],
                        rel_types::NUMBERING => &[
                            IdentifierXmlCategory::AbstractNumbering,
                            IdentifierXmlCategory::NumberingInstance,
                        ],
                        _ => &[],
                    };
                    if !categories.is_empty() && relationship_is_internal(relationship) {
                        identity_parts
                            .entry(part_name_identity(&OpcPackage::resolve_rel_target(
                                &main_part,
                                &relationship.target,
                            )))
                            .or_default()
                            .extend(categories.iter().copied());
                    }
                }
            }
        }

        let mut parts = package.parts.iter().collect::<Vec<_>>();
        parts.sort_by_key(|(part_name, _)| part_name.as_str());
        let mut scanned_parts = HashMap::new();
        for (part_name, bytes) in parts {
            let Some(categories) = identity_parts.get(&part_name_identity(part_name)) else {
                continue;
            };
            let reused = earlier
                .and_then(|earlier| earlier.scanned_parts.get(part_name))
                .filter(|scanned| {
                    scanned.categories == *categories
                        && (Arc::ptr_eq(&scanned.bytes, bytes)
                            || scanned.bytes.as_slice() == bytes.as_slice())
                });
            let hinted = hinted_main.filter(|hint| {
                hint.categories == *categories
                    && (Arc::ptr_eq(&hint.bytes, bytes)
                        || hint.bytes.as_slice() == bytes.as_slice())
            });
            let (definitions, unreadable) = if let Some(hint) = hinted {
                (hint.definitions.clone(), None)
            } else {
                match reused {
                    Some(scanned) => (scanned.definitions.clone(), None),
                    None => {
                        if identifier_xml_is_well_formed(bytes) {
                            let mut definitions = Vec::new();
                            let unreadable =
                                Self::scan_xml_definitions(bytes, categories, &mut definitions)
                                    .err();
                            (Some(definitions), unreadable)
                        } else {
                            (None, None)
                        }
                    }
                }
            };
            if let Some(definitions) = &definitions {
                let mut part_drawing_ids = HashMap::new();
                definitions
                    .iter()
                    .try_for_each(|definition| {
                        identifiers.define(*definition, &mut part_drawing_ids)
                    })
                    .and_then(|()| unreadable.map_or(Ok(()), Err))
                    .map_err(|error| {
                        Error::Other(format!(
                            "cannot scan identifiers in XML part {part_name}: {error}"
                        ))
                    })?;
                let repeated = part_drawing_ids
                    .iter()
                    .filter(|(_, count)| **count > 1)
                    .map(|(id, count)| (*id, *count))
                    .collect::<HashMap<_, _>>();
                if !repeated.is_empty() {
                    identifiers
                        .repeated_drawing_ids
                        .insert(part_name_identity(part_name), repeated);
                }
                identifiers.drawing_ids.extend(part_drawing_ids.into_keys());
            }
            scanned_parts.insert(
                part_name.clone(),
                ScannedIdentifierPart {
                    bytes: Arc::clone(bytes),
                    categories: categories.clone(),
                    definitions,
                },
            );
        }
        identifiers.scanned_parts = Arc::new(scanned_parts);
        identifiers.preserved_relationship_ids = identifiers.relationship_ids.clone();
        identifiers.preserved_comment_ids = identifiers.comment_ids.clone();
        identifiers.preserved_drawing_ids = identifiers.drawing_ids.clone();
        identifiers.preserved_abstract_numbering_ids = identifiers.abstract_numbering_ids.clone();
        identifiers.preserved_numbering_instance_ids = identifiers.numbering_instance_ids.clone();
        identifiers.preserved_part_names = identifiers.part_names.clone();
        Ok(identifiers)
    }

    fn scan_relationships(
        &mut self,
        owner: &str,
        relationships: &[oxoffice_opc::relationship::Relationship],
    ) -> Result<()> {
        let owner_identity = relationship_owner_identity(owner);
        for relationship in relationships {
            if !self
                .relationship_ids
                .entry(owner_identity.clone())
                .or_default()
                .insert(relationship.id.clone())
            {
                return Err(Error::Other(format!(
                    "duplicate relationship id {} in owner {owner}",
                    relationship.id
                )));
            }
            if relationship_is_internal(relationship) {
                self.part_names
                    .insert(part_name_identity(&OpcPackage::resolve_rel_target(
                        owner,
                        &relationship.target,
                    )));
            }
        }
        Ok(())
    }

    fn define(
        &mut self,
        definition: IdentifierDefinition,
        part_drawing_ids: &mut HashMap<u32, usize>,
    ) -> Result<()> {
        match definition {
            IdentifierDefinition::Drawing(value) => {
                *part_drawing_ids.entry(value).or_default() += 1;
            }
            IdentifierDefinition::Bookmark(value) => {
                if !self.bookmark_ids.insert(value) {
                    *self.repeated_bookmark_ids.entry(value).or_insert(1) += 1;
                }
            }
            IdentifierDefinition::Comment(value) => {
                Self::insert_unique(&mut self.comment_ids, value, "comment")?;
            }
            IdentifierDefinition::AbstractNumbering(value) => {
                Self::insert_unique(
                    &mut self.abstract_numbering_ids,
                    value,
                    "abstract numbering",
                )?;
            }
            IdentifierDefinition::NumberingInstance(value) => {
                Self::insert_unique(
                    &mut self.numbering_instance_ids,
                    value,
                    "numbering instance",
                )?;
            }
        }
        Ok(())
    }

    fn scan_xml_definitions(
        xml: &[u8],
        categories: &HashSet<IdentifierXmlCategory>,
        definitions: &mut Vec<IdentifierDefinition>,
    ) -> Result<()> {
        let mut reader = NsReader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
                Ok(value) => value,
                Err(error) => return Err(Error::Other(error.to_string())),
            };
            match event {
                Event::Start(ref element) | Event::Empty(ref element) => {
                    let local = element.local_name();
                    let is_drawing = matches!(
                        &namespace,
                        ResolveResult::Bound(namespace)
                            if namespace.as_ref() == drawing_ns::WP.as_bytes()
                    );
                    let is_word = matches!(
                        &namespace,
                        ResolveResult::Bound(namespace)
                            if namespace.as_ref() == WORD_NAMESPACE.as_bytes()
                    );
                    let category = if is_drawing && local.as_ref() == b"docPr" {
                        Some(IdentifierXmlCategory::Drawing)
                    } else if is_word && local.as_ref() == b"bookmarkStart" {
                        Some(IdentifierXmlCategory::Bookmark)
                    } else if is_word && local.as_ref() == b"comment" {
                        Some(IdentifierXmlCategory::Comment)
                    } else if is_word && local.as_ref() == b"abstractNum" {
                        Some(IdentifierXmlCategory::AbstractNumbering)
                    } else if is_word && local.as_ref() == b"num" {
                        Some(IdentifierXmlCategory::NumberingInstance)
                    } else {
                        None
                    };
                    if let Some(category) = category.filter(|value| categories.contains(value))
                        && let Some(definition) = Self::scan_xml_id(&reader, element, category)?
                    {
                        definitions.push(definition);
                    }
                }
                Event::Eof => return Ok(()),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn scan_xml_id(
        reader: &NsReader<&[u8]>,
        element: &BytesStart<'_>,
        category: IdentifierXmlCategory,
    ) -> Result<Option<IdentifierDefinition>> {
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            let accepted = match category {
                IdentifierXmlCategory::Drawing => {
                    matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"id"
                }
                IdentifierXmlCategory::Bookmark | IdentifierXmlCategory::Comment => {
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes())
                        && local.as_ref() == b"id"
                }
                IdentifierXmlCategory::AbstractNumbering => {
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes())
                        && local.as_ref() == b"abstractNumId"
                }
                IdentifierXmlCategory::NumberingInstance => {
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes())
                        && local.as_ref() == b"numId"
                }
            };
            if !accepted {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| Error::Other(error.to_string()))?;
            let definition = match category {
                IdentifierXmlCategory::Drawing => {
                    let value = value
                        .parse::<u32>()
                        .map_err(|_| Error::Other(format!("invalid drawing id {value}")))?;
                    IdentifierDefinition::Drawing(value)
                }
                IdentifierXmlCategory::Bookmark => {
                    let value = value
                        .parse::<i32>()
                        .map_err(|_| Error::Other(format!("invalid bookmark id {value}")))?;
                    IdentifierDefinition::Bookmark(value)
                }
                IdentifierXmlCategory::Comment => {
                    let value = value
                        .parse::<i32>()
                        .map_err(|_| Error::Other(format!("invalid comment id {value}")))?;
                    IdentifierDefinition::Comment(value)
                }
                IdentifierXmlCategory::AbstractNumbering => {
                    let value = value.parse::<u32>().map_err(|_| {
                        Error::Other(format!("invalid abstract numbering id {value}"))
                    })?;
                    IdentifierDefinition::AbstractNumbering(value)
                }
                IdentifierXmlCategory::NumberingInstance => {
                    let value = value.parse::<u32>().map_err(|_| {
                        Error::Other(format!("invalid numbering instance id {value}"))
                    })?;
                    IdentifierDefinition::NumberingInstance(value)
                }
            };
            return Ok(Some(definition));
        }
        Ok(None)
    }

    /// Refuse a `wp:docPr/@id` occurrence that a staged edit added to an id
    /// its part already uses. Repetitions the source document had are kept.
    pub(super) fn reject_new_repeated_drawing_ids(&self, source: &Self) -> Result<()> {
        let mut added = self
            .repeated_drawing_ids
            .iter()
            .flat_map(|(part_name, repeated)| {
                repeated.iter().filter_map(move |(id, count)| {
                    let kept = source
                        .repeated_drawing_ids
                        .get(part_name)
                        .and_then(|repeated| repeated.get(id))
                        .copied()
                        .unwrap_or(1);
                    (*count > kept).then_some((part_name, *id))
                })
            })
            .collect::<Vec<_>>();
        added.sort_unstable();
        match added.first() {
            Some((part_name, id)) => Err(Error::Other(format!(
                "duplicate drawing id {id} in authored XML of part {part_name}"
            ))),
            None => Ok(()),
        }
    }

    pub(super) fn reject_new_repeated_bookmark_ids(&self, source: &Self) -> Result<()> {
        let added = self
            .repeated_bookmark_ids
            .iter()
            .filter_map(|(id, count)| {
                (*count > source.repeated_bookmark_ids.get(id).copied().unwrap_or(1)).then_some(*id)
            })
            .min();
        match added {
            Some(id) => Err(Error::Other(format!(
                "duplicate bookmark id {id} in authored XML"
            ))),
            None => Ok(()),
        }
    }

    fn insert_unique<T: std::hash::Hash + Eq + std::fmt::Display + Copy>(
        occupied: &mut HashSet<T>,
        value: T,
        category: &str,
    ) -> Result<()> {
        if occupied.insert(value) {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "duplicate {category} id {value} in imported or preserved XML"
            )))
        }
    }

    pub(crate) fn holds_bookmark_id(&self, id: i32) -> bool {
        self.bookmark_ids.contains(&id)
    }

    pub(crate) fn holds_drawing_id(&self, id: u32) -> bool {
        self.drawing_ids.contains(&id)
    }

    /// Take ids that content about to be written keeps, so that no fresh id
    /// reserved for the same content repeats one.
    pub(crate) fn hold_kept_ids(&mut self, bookmark_ids: &[i32], drawing_ids: &[u32]) {
        self.bookmark_ids.extend(bookmark_ids);
        self.drawing_ids.extend(drawing_ids);
    }

    pub(crate) fn reserve_bookmark_id(&mut self) -> Result<i32> {
        reserve_lowest_i32(&mut self.bookmark_ids, 0, "bookmark")
    }

    pub(crate) fn reserve_preferred_bookmark_id(&mut self, preferred: i32) -> Result<i32> {
        if preferred >= 0 && self.bookmark_ids.insert(preferred) {
            Ok(preferred)
        } else {
            reserve_lowest_i32(&mut self.bookmark_ids, 0, "bookmark")
        }
    }

    pub(crate) fn restore_authored_bookmark(&mut self, id: i32, name: &str) {
        self.authored_toc_bookmark_ids.insert(id);
        self.authored_bookmark_names.insert(name.to_owned());
    }

    pub(crate) fn is_authored_bookmark(&self, id: i32) -> bool {
        self.authored_toc_bookmark_ids.contains(&id)
    }

    pub(crate) fn authored_bookmark_names(&self) -> &HashSet<String> {
        &self.authored_bookmark_names
    }

    pub(crate) fn reserve_comment_id(&mut self) -> Result<i32> {
        let id = reserve_i32(&mut self.comment_ids, 0, "comment")?;
        self.authored_comment_ids.insert(id);
        Ok(id)
    }

    pub(crate) fn reserve_drawing_id(&mut self) -> Result<u32> {
        reserve_u32(&mut self.drawing_ids, 1, "drawing")
    }

    /// Edits giving every `wp:docPr` of `xml` a drawing id reserved here, by
    /// the rule of [`Self::reserve_drawing_id`], for content an edit puts in
    /// a part: the save keeps the ids it finds. `scope` binds the prefixes
    /// `xml` uses without declaring them.
    pub(crate) fn fresh_drawing_id_edits(
        &mut self,
        xml: &[u8],
        scope: &BTreeMap<String, String>,
    ) -> Result<Vec<(usize, usize, Vec<u8>)>> {
        if !xml.windows(5).any(|window| window == b"docPr") {
            return Ok(Vec::new());
        }
        let mut wrapped = b"<oxoffice-scope".to_vec();
        for (prefix, namespace) in scope {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            let namespace = namespace
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('"', "&quot;");
            wrapped.extend_from_slice(format!(" {name}=\"{namespace}\"").as_bytes());
        }
        wrapped.push(b'>');
        let offset = wrapped.len();
        wrapped.extend_from_slice(xml);
        wrapped.extend_from_slice(b"</oxoffice-scope>");
        Ok(authored_doc_pr_edits(&wrapped, &mut self.drawing_ids)?
            .into_iter()
            .map(|(start, end, replacement)| (start - offset, end - offset, replacement))
            .collect())
    }

    pub(super) fn reserve_abstract_numbering_id(&mut self) -> Result<u32> {
        let id = reserve_u32(&mut self.abstract_numbering_ids, 0, "abstract numbering")?;
        self.record(|| Reservation::AbstractNumberingId(id));
        Ok(id)
    }

    pub(super) fn reserve_numbering_instance_id(&mut self) -> Result<u32> {
        let id = reserve_u32(&mut self.numbering_instance_ids, 1, "numbering instance")?;
        self.record(|| Reservation::NumberingInstanceId(id));
        Ok(id)
    }

    pub(crate) fn reserve_relationship_id_checked(&mut self, owner: &str) -> Result<String> {
        self.ensure_relationship_capacity(owner, 1)?;
        let owner = relationship_owner_identity(owner);
        let maximum = self
            .relationship_ids
            .get(&owner)
            .into_iter()
            .flatten()
            .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
            .max();
        let mut candidate = maximum.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or_else(|| Error::Other("relationship id range is exhausted".to_owned()))
        })?;
        loop {
            let id = format!("rId{candidate}");
            if self.hold_relationship_id(&owner, &id) {
                return Ok(id);
            }
            candidate = candidate
                .checked_add(1)
                .ok_or_else(|| Error::Other("relationship id range is exhausted".to_owned()))?;
        }
    }

    pub(crate) fn reserve_requested_relationship_id_checked(
        &mut self,
        owner: &str,
        requested: &str,
    ) -> Result<String> {
        let owner = relationship_owner_identity(owner);
        let is_available = !self
            .relationship_ids
            .get(&owner)
            .is_some_and(|occupied| occupied.contains(requested));
        if is_available {
            self.hold_relationship_id(&owner, requested);
            Ok(requested.to_owned())
        } else {
            self.reserve_relationship_id_checked(&owner)
        }
    }

    pub(super) fn ensure_relationship_capacity(
        &self,
        owner: &str,
        additional: usize,
    ) -> Result<()> {
        let owner = relationship_owner_identity(owner);
        let maximum = self
            .relationship_ids
            .get(&owner)
            .into_iter()
            .flatten()
            .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
            .max();
        ensure_numeric_relationship_capacity(maximum, additional)
    }

    pub(super) fn reserve_part_name(
        &mut self,
        directory: &str,
        stem: &str,
        extension: &str,
    ) -> Result<String> {
        let mut namer = oxoffice_media::MediaNamer::scan(
            directory,
            stem,
            self.part_names.iter().map(String::as_str),
        );
        loop {
            let part_name = namer.next_part_name(extension);
            if self.hold_part_name(part_name_identity(&part_name)) {
                return Ok(part_name);
            }
        }
    }

    pub(crate) fn reserve_preferred_part_name(&mut self, preferred: &str) -> Result<String> {
        if !preferred.starts_with('/') {
            return Err(Error::Other(format!(
                "preferred part name {preferred} is not absolute"
            )));
        }
        let (directory, filename) = preferred.rsplit_once('/').ok_or_else(|| {
            Error::Other(format!("preferred part name {preferred} has no directory"))
        })?;
        let (name, extension) = filename.rsplit_once('.').ok_or_else(|| {
            Error::Other(format!("preferred part name {preferred} has no extension"))
        })?;
        if name.is_empty() || extension.is_empty() {
            return Err(Error::Other(format!(
                "preferred part name {preferred} has an empty stem or extension"
            )));
        }
        let digit_start = name
            .bytes()
            .rposition(|byte| !byte.is_ascii_digit())
            .map_or(0, |index| index + 1);
        let suffix = &name[digit_start..];
        let stem = suffix
            .parse::<usize>()
            .ok()
            .filter(|suffix| *suffix > 0)
            .map_or(name, |_| &name[..digit_start]);
        if stem.is_empty() {
            return Err(Error::Other(format!(
                "preferred part name {preferred} has no family stem"
            )));
        }
        if self.hold_part_name(part_name_identity(preferred)) {
            return Ok(preferred.to_owned());
        }
        self.reserve_part_name(directory, stem, extension)
    }

    pub(super) fn register_content_type_default(&mut self, extension: &str) {
        let extension = extension.to_ascii_lowercase();
        self.content_type_defaults.insert(extension.clone());
        self.authored_content_type_defaults.insert(extension);
    }

    pub(crate) fn register_content_type_override(&mut self, part_name: &str) {
        let part_name = part_name.to_ascii_lowercase();
        if !self.content_type_overrides.contains(&part_name) {
            self.record(|| Reservation::ContentTypeOverride(part_name.clone()));
            self.content_type_overrides.insert(part_name);
        }
    }

    pub(super) fn register_nested_story_relationship(
        &mut self,
        story: &StoryId,
        relationship_id: String,
    ) {
        self.authored_nested_story_relationship_ids
            .entry(relationship_owner_identity(&story.part_name))
            .or_default()
            .insert(relationship_id);
    }

    pub(crate) fn retire_authored_story_relationships(
        &mut self,
        owner: &str,
        relationship_ids: impl IntoIterator<Item = String>,
    ) {
        let owner = relationship_owner_identity(owner);
        let preserved = self
            .preserved_relationship_ids
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let removed = relationship_ids
            .into_iter()
            .filter(|id| !preserved.contains(id))
            .collect::<HashSet<_>>();
        if let Some(occupied) = self.relationship_ids.get_mut(&owner) {
            occupied.retain(|id| !removed.contains(id));
            if occupied.is_empty() {
                self.relationship_ids.remove(&owner);
            }
        }
        if let Some(authored) = self.authored_story_relationship_ids.get_mut(&owner) {
            authored.retain(|id| !removed.contains(id));
            if authored.is_empty() {
                self.authored_story_relationship_ids.remove(&owner);
            }
        }
        self.authored_nested_story_relationship_ids
            .retain(|nested_owner, authored| {
                if nested_owner == &owner {
                    authored.retain(|id| !removed.contains(id));
                }
                !authored.is_empty()
            });
    }

    pub(crate) fn retire_authored_part(&mut self, part_name: &str) {
        let identity = part_name_identity(part_name);
        if !self.preserved_part_names.contains(&identity) {
            self.part_names.remove(&identity);
        }
        self.content_type_overrides
            .remove(&part_name.to_ascii_lowercase());
    }

    pub(crate) fn retire_authored_comment_ids(
        &mut self,
        comment_ids: impl IntoIterator<Item = i32>,
    ) {
        for id in comment_ids {
            if self.authored_comment_ids.remove(&id) && !self.preserved_comment_ids.contains(&id) {
                self.comment_ids.remove(&id);
            }
        }
    }

    pub(crate) fn relationship_is_preserved(&self, owner: &str, id: &str) -> bool {
        let owner = relationship_owner_identity(owner);
        self.preserved_relationship_ids
            .get(&owner)
            .is_some_and(|ids| ids.contains(id))
    }

    pub(super) fn part_is_preserved(&self, part_name: &str) -> bool {
        self.preserved_part_names
            .contains(&part_name_identity(part_name))
    }

    pub(crate) fn reserve_fragment_part_name(&mut self, preferred: &str) -> Result<String> {
        if self.hold_part_name(part_name_identity(preferred)) {
            return Ok(preferred.to_owned());
        }
        let (stem, extension) = preferred
            .rsplit_once('.')
            .map_or((preferred, ""), |(stem, extension)| (stem, extension));
        let mut ordinal = 1u64;
        loop {
            let candidate = if extension.is_empty() {
                format!("{stem}-merge-{ordinal}")
            } else {
                format!("{stem}-merge-{ordinal}.{extension}")
            };
            if self.part_names.insert(part_name_identity(&candidate)) {
                return Ok(candidate);
            }
            ordinal = ordinal.checked_add(1).ok_or_else(|| {
                Error::Other("rich mail merge part-name range is exhausted".to_owned())
            })?;
        }
    }

    pub(crate) fn preserve_fragment_part(&mut self, part_name: &str) {
        self.preserved_part_names
            .insert(part_name_identity(part_name));
    }

    /// Hold a part name the package graph names, as preserved when it is new.
    fn observe_part_name(&mut self, identity: String) {
        if self.part_names.contains(&identity) {
            return;
        }
        self.hold_part_name(identity.clone());
        if !self.preserved_part_names.contains(&identity) {
            self.record(|| Reservation::PreservedPartName(identity.clone()));
            self.preserved_part_names.insert(identity);
        }
    }

    pub(crate) fn observe_package_graph(&mut self, package: &OpcPackage) -> Result<()> {
        for part_name in package
            .parts
            .keys()
            .chain(package.part_rels.keys())
            .chain(package.content_types.overrides.keys())
        {
            self.observe_part_name(part_name_identity(part_name));
        }
        for extension in package.content_types.defaults.keys() {
            let extension = extension.to_ascii_lowercase();
            if !self.content_type_defaults.contains(&extension) {
                self.record(|| Reservation::ContentTypeDefault(extension.clone()));
                self.content_type_defaults.insert(extension);
            }
        }
        for part_name in package.content_types.overrides.keys() {
            self.register_content_type_override(part_name);
        }

        let mut owners = std::iter::once(("/", &package.package_rels))
            .chain(
                package
                    .part_rels
                    .iter()
                    .map(|(owner, relationships)| (owner.as_str(), relationships)),
            )
            .collect::<Vec<_>>();
        owners.sort_by_key(|(owner, _)| *owner);
        for (owner, relationships) in owners {
            let owner_identity = relationship_owner_identity(owner);
            let mut seen = HashSet::new();
            for relationship in &relationships.items {
                if !seen.insert(relationship.id.clone()) {
                    return Err(Error::Other(format!(
                        "duplicate relationship id {} in owner {owner}",
                        relationship.id
                    )));
                }
                self.hold_relationship_id(&owner_identity, &relationship.id);
                if relationship_is_internal(relationship) {
                    self.observe_part_name(part_name_identity(&OpcPackage::resolve_rel_target(
                        owner,
                        &relationship.target,
                    )));
                }
            }
        }
        Ok(())
    }

    pub(super) fn reconcile_provenance(&mut self, source: &Self) {
        self.repeated_bookmark_ids
            .clone_from(&source.repeated_bookmark_ids);
        self.repeated_drawing_ids
            .clone_from(&source.repeated_drawing_ids);
        self.preserved_relationship_ids = intersect_relationship_registry(
            &source.preserved_relationship_ids,
            &self.relationship_ids,
        );
        self.authored_story_relationship_ids = intersect_relationship_registry(
            &source.authored_story_relationship_ids,
            &self.relationship_ids,
        );
        self.authored_nested_story_relationship_ids = source
            .authored_nested_story_relationship_ids
            .iter()
            .filter_map(|(owner, ids)| {
                let occupied = self.relationship_ids.get(owner)?;
                let retained = ids.intersection(occupied).cloned().collect::<HashSet<_>>();
                (!retained.is_empty()).then(|| (owner.clone(), retained))
            })
            .collect();
        self.preserved_comment_ids = source
            .preserved_comment_ids
            .intersection(&self.comment_ids)
            .copied()
            .collect();
        self.preserved_drawing_ids = source
            .preserved_drawing_ids
            .intersection(&self.drawing_ids)
            .copied()
            .collect();
        self.preserved_abstract_numbering_ids = source
            .preserved_abstract_numbering_ids
            .intersection(&self.abstract_numbering_ids)
            .copied()
            .collect();
        self.preserved_numbering_instance_ids = source
            .preserved_numbering_instance_ids
            .intersection(&self.numbering_instance_ids)
            .copied()
            .collect();
        self.preserved_part_names = source
            .preserved_part_names
            .intersection(&self.part_names)
            .cloned()
            .collect();
        self.authored_comment_ids = source
            .authored_comment_ids
            .intersection(&self.comment_ids)
            .copied()
            .collect();
        self.authored_toc_bookmark_ids = source
            .authored_toc_bookmark_ids
            .intersection(&self.bookmark_ids)
            .copied()
            .collect();
        self.authored_bookmark_names = source.authored_bookmark_names.clone();
        self.authored_story_parts = source
            .authored_story_parts
            .iter()
            .filter(|part_name| self.part_names.contains(&part_name_identity(part_name)))
            .cloned()
            .collect();
        self.authored_content_type_defaults = source
            .authored_content_type_defaults
            .intersection(&self.content_type_defaults)
            .cloned()
            .collect();
    }
}

fn intersect_relationship_registry(
    source: &HashMap<String, HashSet<String>>,
    occupied: &HashMap<String, HashSet<String>>,
) -> HashMap<String, HashSet<String>> {
    source
        .iter()
        .filter_map(|(owner, ids)| {
            let owner = relationship_owner_identity(owner);
            let retained = ids
                .intersection(occupied.get(&owner)?)
                .cloned()
                .collect::<HashSet<_>>();
            (!retained.is_empty()).then_some((owner, retained))
        })
        .collect()
}

pub(super) fn read_child_canonical_facts(
    xml: &[u8],
    child_start: usize,
    child_end: usize,
    frame: &CanonicalFrameProof,
) -> Option<CanonicalChildFacts> {
    if child_start > child_end || child_end > xml.len() {
        return None;
    }
    let child = child_start..child_end;
    let bound =
        xml_relationship_ids_in_collection_range(xml, &frame.body_bindings, Some(child.clone()))
            .ok()?;
    let plain = if frame.body_bindings.is_empty() {
        bound.clone()
    } else {
        xml_relationship_ids_in_collection_range(xml, &[], Some(child)).ok()?
    };
    Some(CanonicalChildFacts {
        root_declarations: frame.root_declarations.clone(),
        body_declarations: frame.body_declarations.clone(),
        body_bindings: frame.body_bindings.clone(),
        head: Arc::clone(&frame.head),
        tail: Arc::clone(&frame.tail),
        bound_relationship_ids: bound,
        plain_relationship_ids: plain,
    })
}

/// Drawing facts on a frame stay on the whole scanner. A relationship id does
/// not.
pub(super) fn frame_drawings_are_absent(xml: &[u8]) -> bool {
    let (drawings, error) = drawing_occurrences(xml);
    if error.is_some() || !drawings.is_empty() {
        return false;
    }
    drawing_ids_in_xml(xml)
        .ok()
        .is_some_and(|ids| ids.is_empty())
}

/// First-seen relationship ids of `shell`, split at `head_len`.
///
/// The shell scan is the old validator. The head scan uses that same function
/// and the same bindings. Head ids must be that shell result's prefix, and the
/// tail is the remaining suffix. A head-scan error or a prefix mismatch
/// declines. It does not publish a second error.
pub(super) fn split_relationship_ids_at_head(
    shell: &[u8],
    head_len: usize,
    bindings: &[(String, String)],
) -> Option<(Vec<String>, Vec<String>)> {
    let head = shell.get(..head_len)?;
    let shell_ids = xml_relationship_ids_in_order_with_bindings(shell, bindings).ok()?;
    let head_ids = xml_relationship_ids_in_order_with_bindings(head, bindings).ok()?;
    let tail_start = head_ids.len();
    if shell_ids.get(..tail_start) != Some(head_ids.as_slice()) {
        return None;
    }
    let tail_len = shell_ids.len() - tail_start;
    let tail_ids = shell_ids[tail_start..tail_start + tail_len].to_vec();
    Some((head_ids, tail_ids))
}

pub(super) fn scan_identifier_definitions(
    xml: &[u8],
    categories: &HashSet<IdentifierXmlCategory>,
) -> Option<Vec<IdentifierDefinition>> {
    if !identifier_xml_is_well_formed(xml) {
        return None;
    }
    let mut definitions = Vec::new();
    DocumentIdentifiers::scan_xml_definitions(xml, categories, &mut definitions)
        .ok()
        .map(|()| definitions)
}

pub(super) fn identifier_xml_is_well_formed(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((_, Event::Eof)) => return true,
            Ok(_) => buffer.clear(),
            Err(_) => return false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum IdentifierXmlCategory {
    Drawing,
    Bookmark,
    Comment,
    AbstractNumbering,
    NumberingInstance,
}

pub(super) fn reserve_lowest_i32(
    occupied: &mut HashSet<i32>,
    minimum: i32,
    category: &str,
) -> Result<i32> {
    let candidate = (minimum..=i32::MAX)
        .find(|value| !occupied.contains(value))
        .ok_or_else(|| Error::Other(format!("{category} id range is exhausted")))?;
    if !occupied.insert(candidate) {
        return Err(Error::Other(format!(
            "pending {category} id collision for {candidate}"
        )));
    }
    Ok(candidate)
}

fn reserve_i32(occupied: &mut HashSet<i32>, minimum: i32, category: &str) -> Result<i32> {
    let candidate = occupied
        .iter()
        .copied()
        .max()
        .and_then(|value| value.checked_add(1))
        .filter(|value| *value >= minimum)
        .or_else(|| (minimum..=i32::MAX).find(|value| !occupied.contains(value)))
        .ok_or_else(|| Error::Other(format!("{category} id range is exhausted")))?;
    if !occupied.insert(candidate) {
        return Err(Error::Other(format!(
            "pending {category} id collision for {candidate}"
        )));
    }
    Ok(candidate)
}

pub(super) fn reserve_u32(
    occupied: &mut HashSet<u32>,
    minimum: u32,
    category: &str,
) -> Result<u32> {
    let candidate = occupied
        .iter()
        .copied()
        .max()
        .and_then(|value| value.checked_add(1))
        .filter(|value| *value >= minimum)
        .or_else(|| occupied.is_empty().then_some(minimum))
        .ok_or_else(|| Error::Other(format!("{category} id range is exhausted")))?;
    if !occupied.insert(candidate) {
        return Err(Error::Other(format!(
            "pending {category} id collision for {candidate}"
        )));
    }
    Ok(candidate)
}

/// [`reserve_u32`], or the lowest free identifier at or above `minimum`
/// when the one after the highest occupied overflows: content can name the
/// last identifier for a list the document does not define.
pub(super) fn reserve_u32_or_lowest_free(
    occupied: &mut HashSet<u32>,
    minimum: u32,
    category: &str,
) -> Result<u32> {
    let after_highest = occupied
        .iter()
        .copied()
        .max()
        .is_some_and(|highest| highest.checked_add(1).is_some());
    if occupied.is_empty() || after_highest {
        return reserve_u32(occupied, minimum, category);
    }
    let free = (minimum..=u32::MAX)
        .find(|id| !occupied.contains(id))
        .ok_or_else(|| Error::Other(format!("{category} id range is exhausted")))?;
    occupied.insert(free);
    Ok(free)
}

fn ensure_numeric_relationship_capacity(maximum: Option<u32>, required: usize) -> Result<()> {
    let available = maximum.map_or(u64::from(u32::MAX) + 1, |value| u64::from(u32::MAX - value));
    if u64::try_from(required).map_or(true, |required| required > available) {
        Err(Error::Other(
            "relationship id range is exhausted".to_owned(),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn part_name_identity(part_name: &str) -> String {
    part_name.to_ascii_lowercase()
}

pub(super) fn relationship_owner_identity(owner: &str) -> String {
    if owner == "/" {
        "/".to_owned()
    } else {
        part_name_identity(owner)
    }
}
