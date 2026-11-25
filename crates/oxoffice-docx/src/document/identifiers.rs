//! The identifier registry: ids a document holds, reserving new ones and part identities.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum IdentifierDefinition {
    Drawing(u32),
    Bookmark(i32),
    Comment(i32),
    AbstractNumbering(u32),
    NumberingInstance(u32),
    /// The `w:val` of a `w:id` child of a `w:sdtPr`.
    ContentControl(i32),
    /// The `w:id` of a `w:permStart`.
    Permission(i32),
}

/// What an in-place edit added to the identifier registry and the package
/// graph, in order, so that a refusal removes it again instead of putting
/// back copies of both. It records the reservations of the part bundles,
/// relationship ids, part names, content types and numbering ids that
/// [`Document::edit_properties`] and the numbering edits make.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReservationJournal {
    entries: Vec<Reservation>,
    /// Whether the edit removed a part, its relationships or its content
    /// type from the package, which undoing it cannot put back: such an
    /// edit cannot refuse after the removal.
    removed_from_package: bool,
    /// Whether the edit wrote the bytes of a package part, which undoing it
    /// cannot put back either: such an edit cannot refuse after the write.
    written_to_package: bool,
}

#[derive(Clone, Debug)]
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
    /// A comment id, and whether it became an authored one.
    CommentId {
        id: i32,
        authored: bool,
    },
    ContentControlId(i32),
    PermissionId(i32),
    /// The override stored under `stored`, and the case variants it
    /// replaced with their content types.
    PackageOverride {
        stored: String,
        replaced: Vec<(String, String)>,
    },
    PackageRelationships(String),
    /// A relationship id authored in a story part's relationships.
    AuthoredStoryRelationship {
        owner: String,
        id: String,
    },
    /// A part name and content type override an edit retired, when it
    /// removed their part: the undo holds them again.
    RetiredPart {
        identity: Option<String>,
        content_type_override: Option<String>,
    },
    /// The relationship ids of `owner` an edit retired, per set.
    RetiredRelationships {
        owner: String,
        occupied: Vec<String>,
        authored: Vec<String>,
        nested: Vec<String>,
    },
    /// A relationship of `owner`'s part, or of the package when `None`,
    /// added when the relationships' counter was `next_id`.
    PackageRelationship {
        owner: Option<String>,
        id: String,
        next_id: u32,
    },
    /// A drawing id (`wp:docPr/@id`) reserved.
    DrawingId(u32),
    /// A bookmark id reserved.
    BookmarkId(i32),
    /// A story part whose authored mark changed: `held` says whether it was
    /// marked before.
    AuthoredStoryPart {
        name: String,
        held: bool,
    },
    /// A preserved part name an edit released.
    ReleasedPreservedPartName(String),
    /// A preserved relationship id of `owner` an edit released.
    ReleasedPreservedRelationship {
        owner: String,
        id: String,
    },
    /// An authored content type default an edit removed from the package
    /// (`package`, with the stored extensions) and the registry.
    RemovedContentTypeDefault {
        extension: String,
        package: Vec<(String, String)>,
        held: bool,
    },
    /// The relationship id sets of `owner` an edit replaced, as they were.
    StoryRelationshipSets {
        owner: String,
        occupied: Option<HashSet<String>>,
        authored: Option<HashSet<String>>,
    },
    /// A content type default an edit declared: the extension the package
    /// stored, if the edit added it, and whether the registry held and
    /// authored the lowered extension before.
    AddedContentTypeDefault {
        stored: Option<String>,
        lowered: String,
        held: bool,
        authored: bool,
    },
}

impl ReservationJournal {
    /// Append `later`, a journal opened after this one closed.
    pub(crate) fn extend(&mut self, later: ReservationJournal) {
        self.entries.extend(later.entries);
        self.removed_from_package |= later.removed_from_package;
        self.written_to_package |= later.written_to_package;
    }

    /// The part writes and removals this journal records were put back by
    /// the part journal of an in-place edit, so undoing the rest is valid.
    pub(crate) fn forget_package_writes(&mut self) {
        self.removed_from_package = false;
        self.written_to_package = false;
    }

    /// Whether the edit changed the package graph.
    /// Whether the edit removed or wrote package bytes, which undoing the
    /// journal cannot put back.
    pub(crate) fn changed_package_bytes(&self) -> bool {
        self.removed_from_package || self.written_to_package
    }

    pub(crate) fn changed_package(&self) -> bool {
        self.removed_from_package
            || self.written_to_package
            || self.entries.iter().any(|entry| {
                matches!(
                    entry,
                    Reservation::PackageOverride { .. }
                        | Reservation::PackageRelationships(_)
                        | Reservation::PackageRelationship { .. }
                        | Reservation::RemovedContentTypeDefault { .. }
                        | Reservation::AddedContentTypeDefault {
                            stored: Some(_),
                            ..
                        }
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

    pub(crate) fn record_written_to_package(&mut self) {
        if let Some(journal) = &mut self.journal.0 {
            journal.written_to_package = true;
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

    /// Run `reserve`, which may add drawing ids to the occupied set (above
    /// its maximum, as [`reserve_u32`] does), and journal the ids it added,
    /// whether it succeeds or not.
    pub(super) fn reserve_drawing_ids<R>(
        &mut self,
        reserve: impl FnOnce(&mut HashSet<u32>) -> Result<R>,
    ) -> Result<R> {
        if self.journal.0.is_none() {
            return reserve(&mut self.drawing_ids);
        }
        let ceiling = self.drawing_ids.iter().copied().max();
        let result = reserve(&mut self.drawing_ids);
        let added = self
            .drawing_ids
            .iter()
            .copied()
            .filter(|id| ceiling.is_none_or(|ceiling| *id > ceiling))
            .collect::<Vec<_>>();
        for id in added {
            self.record(|| Reservation::DrawingId(id));
        }
        result
    }

    /// Mark `part_name` as a story part this document authored.
    pub(super) fn author_story_part(&mut self, part_name: &str) {
        if self.authored_story_parts.insert(part_name.to_owned()) {
            self.record(|| Reservation::AuthoredStoryPart {
                name: part_name.to_owned(),
                held: false,
            });
        }
    }

    /// Drop the authored mark of `part_name`.
    pub(super) fn release_story_part(&mut self, part_name: &str) {
        if self.authored_story_parts.remove(part_name) {
            self.record(|| Reservation::AuthoredStoryPart {
                name: part_name.to_owned(),
                held: true,
            });
        }
    }

    /// Drop the preserved mark of the part name `identity`.
    pub(super) fn release_preserved_part_name(&mut self, identity: &str) {
        if self.preserved_part_names.remove(identity) {
            self.record(|| Reservation::ReleasedPreservedPartName(identity.to_owned()));
        }
    }

    /// Drop the preserved mark of relationship `id` of `owner_identity`.
    pub(super) fn release_preserved_relationship(&mut self, owner_identity: &str, id: &str) {
        if self
            .preserved_relationship_ids
            .get_mut(owner_identity)
            .is_some_and(|preserved| preserved.remove(id))
        {
            self.record(|| Reservation::ReleasedPreservedRelationship {
                owner: owner_identity.to_owned(),
                id: id.to_owned(),
            });
        }
    }

    /// Replace the relationship ids of the story part `owner_identity` with
    /// `ids`, all authored, as a copied story part holds them.
    pub(super) fn replace_story_relationship_sets(
        &mut self,
        owner_identity: &str,
        ids: HashSet<String>,
    ) {
        let occupied = self
            .relationship_ids
            .insert(owner_identity.to_owned(), ids.clone());
        let authored = if ids.is_empty() {
            self.authored_story_relationship_ids
                .get(owner_identity)
                .cloned()
        } else {
            self.authored_story_relationship_ids
                .insert(owner_identity.to_owned(), ids)
        };
        self.record(|| Reservation::StoryRelationshipSets {
            owner: owner_identity.to_owned(),
            occupied,
            authored,
        });
    }

    /// Make `ids` the relationship ids authored in `owner_identity`'s story
    /// relationships (none when empty), recorded in the open journal.
    pub(super) fn set_authored_story_relationships(
        &mut self,
        owner_identity: &str,
        ids: HashSet<String>,
    ) {
        let before = self
            .authored_story_relationship_ids
            .get(owner_identity)
            .cloned();
        let after = (!ids.is_empty()).then_some(ids);
        if before == after {
            return;
        }
        let occupied = self.relationship_ids.get(owner_identity).cloned();
        self.record(|| Reservation::StoryRelationshipSets {
            owner: owner_identity.to_owned(),
            occupied,
            authored: before,
        });
        match after {
            Some(ids) => {
                self.authored_story_relationship_ids
                    .insert(owner_identity.to_owned(), ids);
            }
            None => {
                self.authored_story_relationship_ids.remove(owner_identity);
            }
        }
    }
}

impl Document {
    /// [`ContentTypes::add_override`], recorded in the open journal.
    ///
    /// [`ContentTypes::add_override`]: oxoffice_opc::ContentTypes::add_override
    pub(crate) fn add_override_journaled(&mut self, part_name: &str, content_type: &str) {
        debug_assert!(
            !self
                .package
                .part_journal()
                .is_some_and(|journal| journal.removed_override(part_name)),
            "an override removed through the part journal is added again through the \
             reservation journal, whose undo would remove the one the part journal puts back"
        );
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

    /// Declare `content_type` as the default of `extension` in the package
    /// when it has none, and as an authored default in the registry,
    /// recorded in the open journal.
    pub(crate) fn add_content_type_default_journaled(
        &mut self,
        extension: &str,
        content_type: &str,
    ) {
        let added = !self.package.content_types.contains_default(extension);
        self.package
            .content_types
            .add_default(extension, content_type);
        let lowered = extension.to_ascii_lowercase();
        let held = self.identifiers.content_type_defaults.contains(&lowered);
        let authored = self
            .identifiers
            .authored_content_type_defaults
            .contains(&lowered);
        self.identifiers.register_content_type_default(extension);
        if added || !held || !authored {
            self.identifiers
                .record(|| Reservation::AddedContentTypeDefault {
                    stored: added.then(|| extension.to_owned()),
                    lowered,
                    held,
                    authored,
                });
        }
    }

    /// Remove the content type default of `extension` this document
    /// authored, from the package and the registry, recorded in the open
    /// journal. Nothing when the registry did not author it.
    pub(crate) fn remove_authored_content_type_default(&mut self, extension: &str) {
        if !self
            .identifiers
            .authored_content_type_defaults
            .remove(extension)
        {
            return;
        }
        let package = self
            .package
            .content_types
            .defaults
            .iter()
            .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(extension))
            .map(|(candidate, value)| (candidate.clone(), value.clone()))
            .collect::<Vec<_>>();
        self.package.content_types.remove_default(extension);
        let held = self.identifiers.content_type_defaults.remove(extension);
        self.identifiers
            .record(|| Reservation::RemovedContentTypeDefault {
                extension: extension.to_owned(),
                package,
                held,
            });
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
        let covered = owner.is_some_and(|owner| self.cover_relationship_add(owner, id));
        let relationships = match owner {
            None => &mut self.package.package_rels,
            Some(owner) => {
                if !covered && self.package.get_part_rels(owner).is_none() {
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
        if !covered {
            self.identifiers
                .record(|| Reservation::PackageRelationship {
                    owner: owner.map(str::to_owned),
                    id: id.to_owned(),
                    next_id,
                });
        }
    }

    /// Add the external relationship `id` to `owner`'s part, recorded in the
    /// open journal at the cost of the edge, not of the owner's
    /// relationships.
    pub(crate) fn add_external_relationship_journaled(
        &mut self,
        owner: &str,
        id: &str,
        rel_type: &str,
        target: &str,
    ) {
        let covered = self.cover_relationship_add(owner, id);
        if !covered && self.package.get_part_rels(owner).is_none() {
            self.identifiers
                .record(|| Reservation::PackageRelationships(owner.to_owned()));
        }
        let relationships = self.package.get_or_create_part_rels(owner);
        debug_assert!(
            relationships.items.iter().all(|item| item.id != id),
            "a reserved relationship id is already in the package"
        );
        let next_id = relationships.next_id();
        relationships
            .items
            .push(oxoffice_opc::relationship::Relationship {
                id: id.to_owned(),
                rel_type: rel_type.to_owned(),
                target: target.to_owned(),
                target_mode: Some("External".to_owned()),
            });
        if !covered {
            self.identifiers
                .record(|| Reservation::PackageRelationship {
                    owner: Some(owner.to_owned()),
                    id: id.to_owned(),
                    next_id,
                });
        }
    }

    /// Whether the part journal undoes the add of relationship `id` to
    /// `owner`, so that the reservation journal records nothing of it.
    ///
    /// An undo runs the part journal first, then the reservation journal
    /// (`undo_edit`), which removes by id and removes an owner's
    /// relationships it created. When the part journal recorded the owner's
    /// relationships whole, its copy puts them back as they were, and a
    /// reservation undo after it would remove what the copy restored (the
    /// owner's relationships, or an id the copy holds). When the part
    /// journal removed `id` one by one, the reservation undo would remove
    /// the item the part journal reinserts; the owner is then recorded whole
    /// here, after that removal, and the copy covers the add.
    fn cover_relationship_add(&mut self, owner: &str, id: &str) -> bool {
        if self
            .package
            .part_journal()
            .is_some_and(|journal| journal.removed_relationship(owner, id))
        {
            self.package.record_part_rels(owner);
        }
        self.package.part_journal_records_rels(owner)
    }

    /// Remove, latest first, what `journal` recorded, leaving the registry
    /// and the package graph as they were when it was opened. Valid only
    /// before another edit reserves anything.
    pub(crate) fn undo_reservations(&mut self, journal: ReservationJournal) {
        debug_assert!(
            !journal.removed_from_package,
            "a journaled edit refused after removing a package part"
        );
        debug_assert!(
            !journal.written_to_package,
            "a journaled edit refused after writing a package part"
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
                Reservation::CommentId { id, authored } => {
                    identifiers.comment_ids.remove(&id);
                    if authored {
                        identifiers.authored_comment_ids.remove(&id);
                    }
                }
                Reservation::ContentControlId(id) => {
                    identifiers.content_control_ids.remove(&id);
                }
                Reservation::PermissionId(id) => {
                    identifiers.permission_ids.remove(&id);
                }
                Reservation::PackageOverride { stored, replaced } => {
                    let overrides = &mut self.package.content_types.overrides;
                    overrides.remove(&stored);
                    overrides.extend(replaced);
                }
                Reservation::PackageRelationships(owner) => {
                    self.package.remove_part_rels(&owner);
                }
                Reservation::AuthoredStoryRelationship { owner, id } => {
                    if let Some(authored) =
                        identifiers.authored_story_relationship_ids.get_mut(&owner)
                    {
                        authored.remove(&id);
                        if authored.is_empty() {
                            identifiers.authored_story_relationship_ids.remove(&owner);
                        }
                    }
                }
                Reservation::RetiredPart {
                    identity,
                    content_type_override,
                } => {
                    identifiers.part_names.extend(identity);
                    identifiers
                        .content_type_overrides
                        .extend(content_type_override);
                }
                Reservation::RetiredRelationships {
                    owner,
                    occupied,
                    authored,
                    nested,
                } => {
                    for (sets, ids) in [
                        (&mut identifiers.relationship_ids, occupied),
                        (&mut identifiers.authored_story_relationship_ids, authored),
                        (
                            &mut identifiers.authored_nested_story_relationship_ids,
                            nested,
                        ),
                    ] {
                        if !ids.is_empty() {
                            sets.entry(owner.clone()).or_default().extend(ids);
                        }
                    }
                }
                Reservation::DrawingId(id) => {
                    identifiers.drawing_ids.remove(&id);
                }
                Reservation::BookmarkId(id) => {
                    identifiers.bookmark_ids.remove(&id);
                }
                Reservation::AuthoredStoryPart { name, held } => {
                    if held {
                        identifiers.authored_story_parts.insert(name);
                    } else {
                        identifiers.authored_story_parts.remove(&name);
                    }
                }
                Reservation::ReleasedPreservedPartName(identity) => {
                    identifiers.preserved_part_names.insert(identity);
                }
                Reservation::ReleasedPreservedRelationship { owner, id } => {
                    identifiers
                        .preserved_relationship_ids
                        .entry(owner)
                        .or_default()
                        .insert(id);
                }
                Reservation::RemovedContentTypeDefault {
                    extension,
                    package,
                    held,
                } => {
                    self.package.content_types.defaults.extend(package);
                    identifiers
                        .authored_content_type_defaults
                        .insert(extension.clone());
                    if held {
                        identifiers.content_type_defaults.insert(extension);
                    }
                }
                Reservation::AddedContentTypeDefault {
                    stored,
                    lowered,
                    held,
                    authored,
                } => {
                    if let Some(stored) = stored {
                        self.package.content_types.defaults.remove(&stored);
                    }
                    if !held {
                        identifiers.content_type_defaults.remove(&lowered);
                    }
                    if !authored {
                        identifiers.authored_content_type_defaults.remove(&lowered);
                    }
                }
                Reservation::StoryRelationshipSets {
                    owner,
                    occupied,
                    authored,
                } => {
                    match occupied {
                        Some(ids) => identifiers.relationship_ids.insert(owner.clone(), ids),
                        None => identifiers.relationship_ids.remove(&owner),
                    };
                    match authored {
                        Some(ids) => identifiers
                            .authored_story_relationship_ids
                            .insert(owner, ids),
                        None => identifiers.authored_story_relationship_ids.remove(&owner),
                    };
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
            content_control_ids: HashSet::new(),
            permission_ids: HashSet::new(),
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
                .extend(STORY_CATEGORIES);
            if let Some(relationships) = package.get_part_rels(&main_part) {
                for relationship in &relationships.items {
                    let categories: &[IdentifierXmlCategory] = match relationship.rel_type.as_str()
                    {
                        rel_types::HEADER
                        | rel_types::FOOTER
                        | rel_types::FOOTNOTES
                        | rel_types::ENDNOTES => &STORY_CATEGORIES,
                        // The glossary is no story: its controls and
                        // permission ranges take no id a story's would.
                        rel_types::GLOSSARY_DOCUMENT => &[
                            IdentifierXmlCategory::Bookmark,
                            IdentifierXmlCategory::Drawing,
                        ],
                        rel_types::COMMENTS => &[
                            IdentifierXmlCategory::Bookmark,
                            IdentifierXmlCategory::Comment,
                            IdentifierXmlCategory::Drawing,
                            IdentifierXmlCategory::ContentControl,
                            IdentifierXmlCategory::Permission,
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
            // Producers repeat both, and the calls that take a new one
            // only need it free: a repetition is no error.
            IdentifierDefinition::ContentControl(value) => {
                self.content_control_ids.insert(value);
            }
            IdentifierDefinition::Permission(value) => {
                self.permission_ids.insert(value);
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
        // Per open element, whether it is a `w:sdtPr`: a control's id is
        // the `w:id` child of one.
        let mut open_sdt_properties = Vec::<bool>::new();
        loop {
            let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
                Ok(value) => value,
                Err(error) => return Err(Error::Other(error.to_string())),
            };
            match event {
                Event::End(_) => {
                    open_sdt_properties.pop();
                }
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
                    } else if is_word && local.as_ref() == b"permStart" {
                        Some(IdentifierXmlCategory::Permission)
                    } else if is_word
                        && local.as_ref() == b"id"
                        && open_sdt_properties.last() == Some(&true)
                    {
                        Some(IdentifierXmlCategory::ContentControl)
                    } else {
                        None
                    };
                    if matches!(event, Event::Start(_)) {
                        open_sdt_properties.push(is_word && local.as_ref() == b"sdtPr");
                    }
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
                IdentifierXmlCategory::Bookmark
                | IdentifierXmlCategory::Comment
                | IdentifierXmlCategory::Permission => {
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
                IdentifierXmlCategory::ContentControl => {
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes())
                        && local.as_ref() == b"val"
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
                // An id that is no number takes no number a new control or
                // range could take: it is left out, as the calls that add
                // one always did.
                IdentifierXmlCategory::ContentControl => match value.parse::<i32>() {
                    Ok(value) => IdentifierDefinition::ContentControl(value),
                    Err(_) => return Ok(None),
                },
                IdentifierXmlCategory::Permission => match value.parse::<i32>() {
                    Ok(value) => IdentifierDefinition::Permission(value),
                    Err(_) => return Ok(None),
                },
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
        for &id in bookmark_ids {
            if self.bookmark_ids.insert(id) {
                self.record(|| Reservation::BookmarkId(id));
            }
        }
        for &id in drawing_ids {
            if self.drawing_ids.insert(id) {
                self.record(|| Reservation::DrawingId(id));
            }
        }
    }

    pub(crate) fn reserve_bookmark_id(&mut self) -> Result<i32> {
        let id = reserve_lowest_i32(&mut self.bookmark_ids, 0, "bookmark")?;
        self.record(|| Reservation::BookmarkId(id));
        Ok(id)
    }

    pub(crate) fn reserve_preferred_bookmark_id(&mut self, preferred: i32) -> Result<i32> {
        if preferred >= 0 && self.bookmark_ids.insert(preferred) {
            self.record(|| Reservation::BookmarkId(preferred));
            Ok(preferred)
        } else {
            self.reserve_bookmark_id()
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
        let authored = self.authored_comment_ids.insert(id);
        self.record(|| Reservation::CommentId { id, authored });
        Ok(id)
    }

    pub(crate) fn reserve_drawing_id(&mut self) -> Result<u32> {
        let id = reserve_u32(&mut self.drawing_ids, 1, "drawing")?;
        self.record(|| Reservation::DrawingId(id));
        Ok(id)
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
        Ok(self
            .reserve_drawing_ids(|occupied| authored_doc_pr_edits(&wrapped, occupied))?
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

    /// The smallest content control id from 1 no story holds, held until
    /// the next reopen and given back by the undo of an in-place edit.
    pub(crate) fn reserve_content_control_id(&mut self) -> Result<i32> {
        let id = reserve_lowest_i32(&mut self.content_control_ids, 1, "content control")?;
        self.record(|| Reservation::ContentControlId(id));
        Ok(id)
    }

    /// The id [`Self::reserve_permission_id`] would reserve now.
    pub(crate) fn lowest_free_permission_id(&self) -> Option<i32> {
        (0..=i32::MAX).find(|id| !self.permission_ids.contains(id))
    }

    /// The smallest permission range id from 0 no story holds, held until
    /// the next reopen and given back by the undo of an in-place edit.
    pub(crate) fn reserve_permission_id(&mut self) -> Result<i32> {
        let id = reserve_lowest_i32(&mut self.permission_ids, 0, "permission range")?;
        self.record(|| Reservation::PermissionId(id));
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

    /// Record `id` as authored in `owner`'s story relationships.
    pub(crate) fn author_story_relationship(&mut self, owner: &str, id: &str) {
        let owner = relationship_owner_identity(owner);
        if self
            .authored_story_relationship_ids
            .entry(owner.clone())
            .or_default()
            .insert(id.to_owned())
        {
            self.record(|| Reservation::AuthoredStoryRelationship {
                owner,
                id: id.to_owned(),
            });
        }
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
        if self.journal.0.is_some() {
            let held = |sets: &HashMap<String, HashSet<String>>| {
                sets.get(&owner).map_or_else(Vec::new, |set| {
                    set.iter()
                        .filter(|id| removed.contains(*id))
                        .cloned()
                        .collect()
                })
            };
            let entry = Reservation::RetiredRelationships {
                owner: owner.clone(),
                occupied: held(&self.relationship_ids),
                authored: held(&self.authored_story_relationship_ids),
                nested: held(&self.authored_nested_story_relationship_ids),
            };
            self.record(|| entry);
        }
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
        let identity = (!self.preserved_part_names.contains(&identity)
            && self.part_names.remove(&identity))
        .then_some(identity);
        let lowered = part_name.to_ascii_lowercase();
        let content_type_override = self
            .content_type_overrides
            .remove(&lowered)
            .then_some(lowered);
        if identity.is_some() || content_type_override.is_some() {
            self.record(|| Reservation::RetiredPart {
                identity,
                content_type_override,
            });
        }
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
            if self.hold_part_name(part_name_identity(&candidate)) {
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
    ContentControl,
    Permission,
}

/// What the scan reads in the main part, a header, a footer and the notes.
pub(super) const STORY_CATEGORIES: [IdentifierXmlCategory; 4] = [
    IdentifierXmlCategory::Bookmark,
    IdentifierXmlCategory::Drawing,
    IdentifierXmlCategory::ContentControl,
    IdentifierXmlCategory::Permission,
];

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A document whose body and header hold content controls and
    /// permission ranges: control ids 1 and 3 and one that is no number in
    /// the body, 2 in the header; permission ids 0 and 2 in the body, 1 in
    /// the header.
    fn document() -> Document {
        let mut document = Document::new();
        document.set_header("header");
        let bytes = document.to_bytes().expect("a new document saves");
        let mut package =
            OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("the package opens");
        let main = package.main_document_part().expect("a main document part");
        let xml = String::from_utf8(package.get_part(&main).expect("main part").to_vec())
            .expect("the main part is UTF-8");
        let start = xml.find("<w:body>").expect("a body") + "<w:body>".len();
        let body = concat!(
            r#"<w:sdt><w:sdtPr><w:id w:val="1"/></w:sdtPr><w:sdtContent><w:p/></w:sdtContent></w:sdt>"#,
            r#"<w:p><w:permStart w:id="0" w:edGrp="everyone"/><w:sdt><w:sdtPr><w:id w:val="3"/></w:sdtPr>"#,
            r#"<w:sdtContent><w:r><w:t>inline</w:t></w:r></w:sdtContent></w:sdt><w:permEnd w:id="0"/></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:id w:val="none"/></w:sdtPr><w:sdtContent><w:p/></w:sdtContent></w:sdt>"#,
            r#"<w:p><w:permStart w:id="2" w:ed="someone"/><w:r><w:t>x</w:t></w:r><w:permEnd w:id="2"/></w:p>"#,
        );
        package.set_part(
            &main,
            format!("{}{body}{}", &xml[..start], &xml[start..]).into_bytes(),
        );
        let header = package
            .parts
            .keys()
            .find(|name| name.contains("header"))
            .expect("a header part")
            .clone();
        let xml = String::from_utf8(package.get_part(&header).expect("header").to_vec())
            .expect("the header is UTF-8");
        let end = xml.rfind("</w:hdr>").expect("a header root");
        let added = concat!(
            r#"<w:p><w:permStart w:id="1" w:edGrp="everyone"/><w:sdt><w:sdtPr><w:id w:val="2"/></w:sdtPr>"#,
            r#"<w:sdtContent><w:r><w:t>h</w:t></w:r></w:sdtContent></w:sdt><w:permEnd w:id="1"/></w:p>"#,
        );
        package.set_part(
            &header,
            format!("{}{added}{}", &xml[..end], &xml[end..]).into_bytes(),
        );
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("the package writes");
        Document::from_bytes(&out.into_inner()).expect("the document opens")
    }

    #[test]
    fn open_reads_the_control_and_permission_ids_of_the_body_and_a_header() {
        let document = document();
        assert_eq!(
            document.identifiers.content_control_ids,
            HashSet::from([1, 2, 3])
        );
        assert_eq!(
            document.identifiers.permission_ids,
            HashSet::from([0, 1, 2])
        );
    }

    #[test]
    fn a_reservation_takes_the_smallest_free_id() {
        let mut document = document();
        assert_eq!(
            document.identifiers.reserve_content_control_id().unwrap(),
            4
        );
        assert_eq!(
            document.identifiers.reserve_content_control_id().unwrap(),
            5
        );
        assert_eq!(document.identifiers.reserve_permission_id().unwrap(), 3);
        let mut empty = Document::new();
        assert_eq!(empty.identifiers.reserve_content_control_id().unwrap(), 1);
        assert_eq!(empty.identifiers.reserve_permission_id().unwrap(), 0);
    }

    #[test]
    fn a_refused_in_place_edit_gives_its_control_and_permission_ids_back() {
        let mut document = document();
        let before = document.identifiers.clone();
        let result: Result<()> = document.edit_in_place(&[], |document| {
            assert_eq!(document.identifiers.reserve_content_control_id()?, 4);
            assert_eq!(document.identifiers.reserve_permission_id()?, 3);
            Err(Error::Other("refused".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(document.identifiers, before);
        assert_eq!(
            document.identifiers.reserve_content_control_id().unwrap(),
            4
        );
        assert_eq!(document.identifiers.reserve_permission_id().unwrap(), 3);
    }

    #[test]
    fn a_new_control_takes_the_smallest_id_the_registry_holds_free() {
        let mut document = document();
        let story = document
            .stories()
            .unwrap()
            .into_iter()
            .find(|story| story.kind() == StoryKind::Body)
            .expect("a body story");
        let index = document
            .story_items(&story)
            .unwrap()
            .iter()
            .position(|item| item.kind() == StoryItemKind::Paragraph)
            .expect("a body paragraph");
        let destination = ContentLocation::new(story, StoryItemKind::Paragraph, vec![index]);
        document
            .insert_content_control(
                &destination,
                crate::content_control::ContentControlSpec {
                    placement: crate::content_control::ContentControlPlacement::Block,
                    kind: oxoffice_docx_oxml::content_control::SdtType::RichText,
                    tag: Some("new".to_owned()),
                    alias: None,
                    lock: None,
                    appearance: None,
                    placeholder: None,
                },
            )
            .expect("a control is inserted");
        assert!(document.identifiers.content_control_ids.contains(&4));
        assert_eq!(
            document.identifiers.reserve_content_control_id().unwrap(),
            5
        );
    }
}
