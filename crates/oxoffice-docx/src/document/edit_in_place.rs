//! Edits that run on the live document and undo themselves on a refusal.
//!
//! See `specs/2026-11-24-staged-edits-in-place.md`. An edit names the units
//! it may change. Each unit is copied before the edit runs, at the cost of
//! the unit; the parts the edit writes are journaled by the package, and the
//! reservations by the identifier registry (#600). Any exit that is not a
//! success puts them back.

use super::identifiers::ReservationJournal;
use super::*;

/// A part of the document an in-place edit may change.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditUnit {
    /// The footnotes model and its part.
    Footnotes,
    /// The endnotes part, which has no model: the part journal undoes it.
    Endnotes,
    /// The comments and extended comments models and their parts.
    Comments,
    /// The styles model.
    Styles,
    /// The settings model and its part name.
    Settings,
    /// Package parts the edit writes outside the models of its other units
    /// (custom XML items, recipient data): their bytes, the relationships
    /// and content type overrides the edit records, by the part journal
    /// alone.
    Parts,
    /// The `count` direct body blocks from `slot` (0 for an insertion at
    /// `slot`): the edit may replace them with any number of blocks, and
    /// change no other block. They are copied, at the cost of these blocks.
    /// An edit may name several, disjoint and in slot order: then only the
    /// last may change its number of blocks, each other one keeps it.
    BodyBlocks { slot: usize, count: usize },
    /// The font table model, its part name and its dirty flag.
    FontTable,
    /// The section properties (`w:sectPr`) of section `index`, in document
    /// order: the one in a body paragraph's properties for an earlier
    /// section, the body's final one for the last. Only that `CT_SectPr`
    /// is copied; the paragraph that holds it keeps every other property,
    /// and no body block is added or removed. An index past the
    /// paragraph-held sections names the final one, absent or not.
    SectionProperties { index: usize },
    /// The main document's page background (`w:background`, with the
    /// foreign background children kept beside it): only that element is
    /// copied, never the body.
    Background,
}

/// What an edit did to the direct body blocks of its `BodyBlocks` unit: the
/// `removed` blocks from `slot` gave way to `inserted` blocks.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyBlocksChange {
    pub slot: usize,
    pub removed: usize,
    pub inserted: usize,
}

/// What an in-place edit changed, for the caller that publishes it.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditReport {
    /// Each unit the edit named, and whether it changed.
    pub units: Vec<(EditUnit, bool)>,
    /// The blocks of the `BodyBlocks` unit, when the edit named one and it
    /// changed.
    pub body: Option<BodyBlocksChange>,
    /// The blocks of each `BodyBlocks` unit that changed, in slot order.
    pub body_blocks: Vec<BodyBlocksChange>,
}

impl EditReport {
    /// Whether the edit changed anything.
    pub fn changed(&self) -> bool {
        self.units.iter().any(|(_, changed)| *changed)
    }

    /// The units the edit changed.
    pub fn changed_units(&self) -> impl Iterator<Item = &EditUnit> {
        self.units
            .iter()
            .filter(|(_, changed)| *changed)
            .map(|(unit, _)| unit)
    }
}

#[derive(Clone, Debug)]
struct FootnotesCopy {
    footnotes: oxoffice_docx_oxml::footnotes::CT_Footnotes,
    part_name: Option<String>,
    root_namespace_declarations: Vec<(String, String)>,
    dirty: bool,
    /// The part's bytes, for the no-op answer: the model holds paragraphs
    /// alone, and a table added to a note changes the part only.
    part_bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct CommentsCopy {
    comments: Option<oxoffice_docx_oxml::comments::CT_Comments>,
    part_name: Option<String>,
    dirty: bool,
    extended: Option<oxoffice_docx_oxml::comments_extended::CT_CommentsEx>,
    extended_part_name: Option<String>,
    owned: bool,
    extended_owned: bool,
}

#[derive(Clone, Debug)]
struct BodyBlocksCopy {
    slot: usize,
    blocks: Vec<oxoffice_docx_oxml::document::BodyContent>,
    length: usize,
    unsavable: Option<Option<String>>,
}

/// The ranges of the body that hold what the edit left in place of each
/// copy, now that the body has `length` blocks: each unit but the last kept
/// its length, the last takes the change of the body's length.
fn body_ranges(copies: &[BodyBlocksCopy], length: usize) -> Vec<std::ops::Range<usize>> {
    let last = copies.len().saturating_sub(1);
    copies
        .iter()
        .enumerate()
        .map(|(index, copy)| {
            if index < last {
                return copy.slot..copy.slot + copy.blocks.len();
            }
            let end = (copy.slot + copy.blocks.len() + length).saturating_sub(copy.length);
            copy.slot..end.max(copy.slot)
        })
        .collect()
}

#[derive(Clone, Debug)]
struct FontTableCopy {
    font_table: Option<FontTable>,
    part_name: Option<String>,
    dirty: bool,
}

/// Where a section's properties live in the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SectionOwner {
    /// In the properties of the paragraph at this body slot.
    Paragraph(usize),
    /// The body's final section properties.
    Final,
}

#[derive(Clone, Debug)]
struct SectionCopy {
    index: usize,
    owner: SectionOwner,
    properties: Option<CT_SectPr>,
}

#[derive(Clone, Debug, PartialEq)]
struct BackgroundCopy {
    xml: Option<Vec<u8>>,
    extra: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct SettingsCopy {
    settings: Option<CT_Settings>,
    part_name: Option<String>,
    owned: bool,
}

#[derive(Clone)]
struct StylesCopy {
    styles: Arc<CT_Styles>,
    part_cache: Option<StylesPartCache>,
    part_name: Option<String>,
}

/// The scalar state every edit may touch. None of it grows with the body.
#[derive(Clone, Debug)]
struct ScalarCopy {
    embedded_invalidated_signatures: HashSet<(String, String)>,
    freed_relationships: Vec<(String, String)>,
    package_signatures_invalidated: bool,
}

/// What puts the units of an in-place edit back: their copies and the
/// journals of the parts and reservations the edit wrote. Dropping it keeps
/// the edit; [`Document::undo_edit`] undoes it.
#[doc(hidden)]
#[derive(Clone)]
pub struct EditUndo {
    units: Vec<EditUnit>,
    footnotes: Option<FootnotesCopy>,
    comments: Option<CommentsCopy>,
    styles: Option<StylesCopy>,
    settings: Option<SettingsCopy>,
    /// One copy per `BodyBlocks` unit, in slot order.
    body: Vec<BodyBlocksCopy>,
    /// The endnotes part's bytes before the edit, for its no-op answer.
    endnotes: Option<Option<Vec<u8>>>,
    font_table: Option<FontTableCopy>,
    sections: Vec<SectionCopy>,
    background: Option<BackgroundCopy>,
    scalars: ScalarCopy,
    parts: oxoffice_opc::PartJournal,
    reservations: ReservationJournal,
    report: EditReport,
}

impl EditUndo {
    /// What the edit changed.
    pub fn report(&self) -> &EditReport {
        &self.report
    }

    /// The direct body blocks the `BodyBlocks` unit copied before the edit,
    /// from its slot on, when the edit named one such unit (as
    /// [`EditReport::body`]).
    pub fn previous_body_blocks(&self) -> Option<&[oxoffice_docx_oxml::document::BodyContent]> {
        match self.body.as_slice() {
            [copy] => Some(copy.blocks.as_slice()),
            _ => None,
        }
    }
}

/// The in-place edit open on a document and the undo its caller holds. A
/// copy of a document starts with neither.
#[derive(Default)]
pub(super) struct InPlaceState {
    open: bool,
    /// The units of the open call, which a nested call must stay within.
    #[cfg(debug_assertions)]
    open_units: Vec<EditUnit>,
    hold: bool,
    held: Option<EditUndo>,
    /// Whether the last outermost in-place edit changed its units.
    last_changed: bool,
}

impl Clone for InPlaceState {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Document {
    fn copy_units(&self, units: &[EditUnit]) -> EditUndo {
        let mut undo = EditUndo {
            units: units.to_vec(),
            footnotes: None,
            comments: None,
            styles: None,
            settings: None,
            body: Vec::new(),
            endnotes: None,
            font_table: None,
            sections: Vec::new(),
            background: None,
            scalars: ScalarCopy {
                embedded_invalidated_signatures: self.embedded_invalidated_signatures.clone(),
                freed_relationships: self.freed_relationships.clone(),
                package_signatures_invalidated: self.package_signatures_invalidated,
            },
            parts: oxoffice_opc::PartJournal::default(),
            reservations: ReservationJournal::default(),
            report: EditReport::default(),
        };
        for unit in units {
            match unit {
                EditUnit::Footnotes => {
                    undo.footnotes = Some(FootnotesCopy {
                        footnotes: self.footnotes.clone(),
                        part_name: self.footnotes_part_name.clone(),
                        root_namespace_declarations: self
                            .footnotes_root_namespace_declarations
                            .clone(),
                        dirty: self.footnotes_dirty,
                        part_bytes: self.footnotes_part_bytes(),
                    });
                }
                EditUnit::Comments => {
                    undo.comments = Some(CommentsCopy {
                        comments: self.comments.clone(),
                        part_name: self.comments_part_name.clone(),
                        dirty: self.comments_dirty,
                        extended: self.comments_extended.clone(),
                        extended_part_name: self.comments_extended_part_name.clone(),
                        owned: self.comments_owned,
                        extended_owned: self.comments_extended_owned,
                    });
                }
                EditUnit::Endnotes => undo.endnotes = Some(self.endnotes_part_bytes()),
                EditUnit::Styles => {
                    undo.styles = Some(StylesCopy {
                        styles: Arc::clone(&self.styles),
                        part_cache: self.styles_part_cache.clone(),
                        part_name: self.styles_part_name.clone(),
                    });
                }
                EditUnit::Settings => {
                    undo.settings = Some(SettingsCopy {
                        settings: self.settings.clone(),
                        part_name: self.settings_part_name.clone(),
                        owned: self.settings_owned,
                    });
                }
                EditUnit::Parts => {}
                EditUnit::FontTable => {
                    undo.font_table = Some(FontTableCopy {
                        font_table: self.font_table.clone(),
                        part_name: self.font_table_part_name.clone(),
                        dirty: self.font_table_dirty,
                    });
                }
                EditUnit::SectionProperties { index } => {
                    if undo.sections.iter().all(|copy| copy.index != *index) {
                        let owner = self.section_owner(*index);
                        undo.sections.push(SectionCopy {
                            index: *index,
                            owner,
                            properties: self.section_properties_at(owner).cloned(),
                        });
                    }
                }
                EditUnit::Background => undo.background = Some(self.background_copy()),
                EditUnit::BodyBlocks { slot, count } => {
                    let content = &self.document.body.content;
                    let end = (*slot + *count).min(content.len());
                    debug_assert!(
                        undo.body
                            .last()
                            .is_none_or(|copy| copy.slot + copy.blocks.len() <= *slot),
                        "body block units are disjoint and in slot order"
                    );
                    undo.body.push(BodyBlocksCopy {
                        slot: *slot,
                        blocks: content[(*slot).min(end)..end].to_vec(),
                        length: content.len(),
                        unsavable: self.unsavable_body_edit_cache.get().cloned(),
                    });
                }
            }
        }
        undo
    }

    /// Put back what `undo` copied and journaled.
    #[doc(hidden)]
    pub fn undo_edit(&mut self, undo: EditUndo) {
        let EditUndo {
            footnotes,
            comments,
            styles,
            settings,
            body,
            font_table,
            sections,
            background,
            scalars,
            parts,
            reservations,
            ..
        } = undo;
        // The part journal first, then the reservation journal, which
        // removes by id or by name. So the reservation journal records no
        // add the part journal undoes: a relationship added to an owner the
        // part journal recorded whole, or whose id it removed one by one
        // (the owner is then recorded whole), is left to the part journal's
        // copy (`cover_relationship_add`); an override removed through the
        // part journal is never added again in the same call (checked in
        // debug builds by `add_override_journaled`).
        self.package.undo_part_journal(parts);
        // The part journal put back the bytes the edit wrote or removed.
        let mut reservations = reservations;
        reservations.forget_package_writes();
        self.undo_reservations(reservations);
        if let Some(copy) = footnotes {
            self.footnotes = copy.footnotes;
            self.footnotes_part_name = copy.part_name;
            self.footnotes_root_namespace_declarations = copy.root_namespace_declarations;
            self.footnotes_dirty = copy.dirty;
        }
        if let Some(copy) = comments {
            self.comments = copy.comments;
            self.comments_part_name = copy.part_name;
            self.comments_dirty = copy.dirty;
            self.comments_extended = copy.extended;
            self.comments_extended_part_name = copy.extended_part_name;
            self.comments_owned = copy.owned;
            self.comments_extended_owned = copy.extended_owned;
        }
        if let Some(copy) = styles {
            self.styles = copy.styles;
            self.styles_part_cache = copy.part_cache;
            self.styles_part_name = copy.part_name;
        }
        if let Some(copy) = settings {
            self.settings = copy.settings;
            self.settings_part_name = copy.part_name;
            self.settings_owned = copy.owned;
        }
        if let Some(copy) = font_table {
            self.font_table = copy.font_table;
            self.font_table_part_name = copy.part_name;
            self.font_table_dirty = copy.dirty;
        }
        for copy in sections.into_iter().rev() {
            self.restore_section_properties(copy.owner, copy.properties);
        }
        if let Some(copy) = background {
            self.document.background_xml = copy.xml;
            self.document.background_extra_xml = copy.extra;
        }
        self.embedded_invalidated_signatures = scalars.embedded_invalidated_signatures;
        self.freed_relationships = scalars.freed_relationships;
        self.package_signatures_invalidated = scalars.package_signatures_invalidated;
        // Only a body unit changes what a save of the body answers.
        let unsavable = match body.first() {
            Some(copy) => copy.unsavable.clone(),
            None => self.unsavable_body_edit_cache.get().cloned(),
        };
        // Each unit's range is where its copy goes back: the units before
        // the last kept their lengths, so no splice moves another's range.
        let ranges = body_ranges(&body, self.document.body.content.len());
        for (copy, range) in body.into_iter().zip(ranges).rev() {
            self.document.body.content.splice(range, copy.blocks);
        }
        self.invalidate_layout();
        if let Some(unsavable) = unsavable {
            self.unsavable_body_edit_cache = std::sync::OnceLock::from(unsavable);
        }
    }

    /// Whether the units of `undo` differ from what the document holds now.
    fn report_changes(&self, undo: &EditUndo) -> EditReport {
        let content = &self.document.body.content;
        let ranges = body_ranges(&undo.body, content.len());
        // One flag per copy, in unit order: two units may start at one slot
        // (an insertion before a replaced block).
        let changed: Vec<bool> = undo
            .body
            .iter()
            .zip(&ranges)
            .map(|(copy, range)| content.get(range.clone()) != Some(&copy.blocks[..]))
            .collect();
        let body_blocks: Vec<BodyBlocksChange> = undo
            .body
            .iter()
            .zip(&ranges)
            .zip(&changed)
            .filter(|(_, changed)| **changed)
            .map(|((copy, range), _)| BodyBlocksChange {
                slot: copy.slot,
                removed: copy.blocks.len(),
                inserted: range.len(),
            })
            .collect();
        let mut body_changed = changed.into_iter();
        EditReport {
            units: undo
                .units
                .iter()
                .map(|unit| {
                    let changed = match unit {
                        EditUnit::Footnotes => undo.footnotes.as_ref().is_some_and(|copy| {
                            copy.footnotes != self.footnotes
                                || copy.part_name != self.footnotes_part_name
                                || copy.part_bytes != self.footnotes_part_bytes()
                        }),
                        EditUnit::Comments => undo.comments.as_ref().is_some_and(|copy| {
                            copy.comments != self.comments
                                || copy.extended != self.comments_extended
                                || copy.part_name != self.comments_part_name
                                || copy.extended_part_name != self.comments_extended_part_name
                        }),
                        EditUnit::Endnotes => undo
                            .endnotes
                            .as_ref()
                            .is_some_and(|before| *before != self.endnotes_part_bytes()),
                        EditUnit::Styles => undo
                            .styles
                            .as_ref()
                            .is_some_and(|copy| *copy.styles != *self.styles),
                        EditUnit::Settings => undo.settings.as_ref().is_some_and(|copy| {
                            copy.settings != self.settings
                                || copy.part_name != self.settings_part_name
                        }),
                        EditUnit::Parts => self.package.part_journal_changed(&undo.parts),
                        EditUnit::FontTable => undo.font_table.as_ref().is_some_and(|copy| {
                            copy.font_table != self.font_table
                                || copy.part_name != self.font_table_part_name
                        }),
                        EditUnit::SectionProperties { index } => undo
                            .sections
                            .iter()
                            .find(|copy| copy.index == *index)
                            .is_some_and(|copy| {
                                copy.properties.as_ref() != self.section_properties_at(copy.owner)
                            }),
                        EditUnit::Background => undo
                            .background
                            .as_ref()
                            .is_some_and(|copy| *copy != self.background_copy()),
                        EditUnit::BodyBlocks { .. } => body_changed.next().unwrap_or(false),
                    };
                    (unit.clone(), changed)
                })
                .collect(),
            body: match undo.body.len() {
                1 => body_blocks.first().copied(),
                _ => None,
            },
            body_blocks,
        }
    }

    /// Where the properties of section `index` live: the paragraph that
    /// holds the `index`-th paragraph-held section properties, else the
    /// body's final ones. It walks the body blocks, comparing each, without
    /// copying any.
    fn section_owner(&self, index: usize) -> SectionOwner {
        self.document
            .body
            .content
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                matches!(
                    block,
                    BodyContent::Paragraph(paragraph)
                        if paragraph
                            .properties
                            .as_ref()
                            .is_some_and(|properties| properties.sect_pr.is_some())
                )
            })
            .nth(index)
            .map_or(SectionOwner::Final, |(slot, _)| {
                SectionOwner::Paragraph(slot)
            })
    }

    fn section_properties_at(&self, owner: SectionOwner) -> Option<&CT_SectPr> {
        match owner {
            SectionOwner::Final => self.document.body.sect_pr.as_ref(),
            SectionOwner::Paragraph(slot) => match self.document.body.content.get(slot) {
                Some(BodyContent::Paragraph(paragraph)) => paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref()),
                _ => None,
            },
        }
    }

    fn restore_section_properties(&mut self, owner: SectionOwner, properties: Option<CT_SectPr>) {
        if self.section_properties_at(owner) == properties.as_ref() {
            return;
        }
        match owner {
            SectionOwner::Final => self.document.body.sect_pr = properties,
            SectionOwner::Paragraph(slot) => {
                if let Some(BodyContent::Paragraph(paragraph)) =
                    self.document.body.content.get_mut(slot)
                {
                    paragraph.properties.get_or_insert_default().sect_pr = properties;
                }
            }
        }
    }

    /// The page background of the main document, as a unit copies it.
    fn background_copy(&self) -> BackgroundCopy {
        BackgroundCopy {
            xml: self.document.background_xml.clone(),
            extra: self.document.background_extra_xml.clone(),
        }
    }

    /// The bytes of the footnotes part, if the document has one.
    fn footnotes_part_bytes(&self) -> Option<Vec<u8>> {
        self.footnotes_part_name
            .as_deref()
            .and_then(|name| self.package.get_part(name).map(<[u8]>::to_vec))
    }

    /// The bytes of the endnotes part, if the document has one.
    fn endnotes_part_bytes(&self) -> Option<Vec<u8>> {
        self.note_part_name(StoryKind::Endnote)
            .ok()
            .flatten()
            .and_then(|name| self.package.get_part(&name).map(<[u8]>::to_vec))
    }

    /// Run `edit` on the live document, allowed to change `units` alone.
    ///
    /// A refusal, or the unwind of a panic, puts the units back and undoes
    /// the reservations and part writes the edit made. A success commits,
    /// unless the caller holds the edit ([`Self::hold_edit_undo`]): the
    /// undo then waits in [`Self::take_edit_undo`]. An edit opened inside
    /// an open one joins it.
    pub(crate) fn edit_in_place<T>(
        &mut self,
        units: &[EditUnit],
        edit: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if self.in_place.open {
            // A nested call (a staged helper the route reuses) joins the open
            // one, whose copies and journals already cover it: its units must
            // be among the open call's, and the open call's undo oracle proves
            // that it wrote nothing else.
            #[cfg(debug_assertions)]
            assert!(
                units
                    .iter()
                    .all(|unit| self.in_place.open_units.contains(unit)),
                "a nested in-place edit names units {units:?} outside the open call's {:?}",
                self.in_place.open_units
            );
            return edit(self);
        }
        struct Guard<'a> {
            document: &'a mut Document,
            undo: Option<EditUndo>,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.document.in_place.open = false;
                if let Some(mut undo) = self.undo.take() {
                    undo.parts = self.document.package.close_part_journal();
                    undo.reservations = self.document.identifiers.close_journal();
                    self.document.undo_edit(undo);
                }
            }
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let undo = self.copy_units(units);
        self.package.open_part_journal();
        self.identifiers.open_journal();
        self.in_place.open = true;
        #[cfg(debug_assertions)]
        {
            self.in_place.open_units = units.to_vec();
        }
        let mut guard = Guard {
            document: self,
            undo: Some(undo),
        };
        let value = edit(guard.document)?;
        let mut undo = guard.undo.take().expect("the edit is open");
        let document = &mut *guard.document;
        undo.parts = document.package.close_part_journal();
        undo.reservations = document.identifiers.close_journal();
        document.in_place.open = false;
        undo.report = document.report_changes(&undo);
        let signed = document
            .package
            .package_rels
            .items
            .iter()
            .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN);
        if signed
            && (undo.reservations.changed_package()
                || document.package.part_journal_changed(&undo.parts))
        {
            document.package_signatures_invalidated = true;
        }
        document.in_place.last_changed = undo.report.changed();
        if undo.report.changed() {
            if undo
                .units
                .iter()
                .all(|unit| matches!(unit, EditUnit::Settings | EditUnit::Parts))
            {
                document.invalidate_layout_keeping_body_answer();
            } else {
                document.invalidate_layout();
            }
        }
        #[cfg(debug_assertions)]
        document.assert_undo_restores(before, &undo);
        if document.in_place.hold {
            match &mut document.in_place.held {
                Some(held) => held.follow(undo),
                held @ None => *held = Some(undo),
            }
        }
        Ok(value)
    }

    /// Whether the last outermost in-place edit changed what it named: the
    /// no-op answer of a call that returns one.
    pub(crate) fn take_last_edit_changed(&mut self) -> bool {
        std::mem::take(&mut self.in_place.last_changed)
    }

    /// Whether an in-place edit is open: an edit that keeps its own journal
    /// joins it instead, and the open edit undoes it.
    pub(super) fn in_place_is_open(&self) -> bool {
        self.in_place.open
    }

    /// Keep the undo of the next in-place edits for [`Self::take_edit_undo`]
    /// instead of committing them.
    #[doc(hidden)]
    pub fn hold_edit_undo(&mut self) {
        debug_assert!(self.in_place.held.is_none(), "an edit undo is held");
        self.in_place.hold = true;
    }

    /// The undo of the edits made since [`Self::hold_edit_undo`], if any
    /// changed the document in place, and stop holding.
    #[doc(hidden)]
    pub fn take_edit_undo(&mut self) -> Option<EditUndo> {
        self.in_place.hold = false;
        self.in_place.held.take()
    }
}

impl EditUndo {
    /// Make this undo also undo `later`, an edit made after it.
    fn follow(&mut self, later: EditUndo) {
        let EditUndo {
            units,
            footnotes,
            comments,
            styles,
            settings,
            body,
            endnotes,
            font_table,
            sections,
            background,
            parts,
            reservations,
            report,
            ..
        } = later;
        for unit in units {
            if !self.units.contains(&unit) {
                self.units.push(unit);
            }
        }
        self.footnotes = self.footnotes.take().or(footnotes);
        self.comments = self.comments.take().or(comments);
        self.styles = self.styles.take().or(styles);
        self.settings = self.settings.take().or(settings);
        self.font_table = self.font_table.take().or(font_table);
        self.background = self.background.take().or(background);
        for copy in sections {
            if self.sections.iter().all(|known| known.index != copy.index) {
                self.sections.push(copy);
            }
        }
        debug_assert!(
            self.body.is_empty() || body.is_empty(),
            "two held edits name body blocks"
        );
        if self.body.is_empty() {
            self.body = body;
            self.report.body = report.body;
            self.report.body_blocks = report.body_blocks;
        }
        self.endnotes = self.endnotes.take().or(endnotes);
        self.parts.extend(parts);
        self.reservations.extend(reservations);
        for (unit, changed) in report.units {
            match self
                .report
                .units
                .iter_mut()
                .find(|(known, _)| *known == unit)
            {
                Some((_, known)) => *known |= changed,
                None => self.report.units.push((unit, changed)),
            }
        }
    }
}

#[cfg(debug_assertions)]
impl Document {
    /// Debug proof that an edit wrote inside its units and journals alone:
    /// its undo, applied to a copy of the edited document, gives back the
    /// document before the edit.
    fn assert_undo_restores(&self, mut before: Self, undo: &EditUndo) {
        let mut restored = self.clone_for_staging();
        restored.undo_edit(undo.clone());
        assert!(
            restored.package.same_package_as(&before.package),
            "an in-place edit wrote a part or relationship outside its journals ({:?})",
            undo.units
        );
        assert_eq!(
            restored.identifiers, before.identifiers,
            "an in-place edit reserved outside its journal ({:?})",
            undo.units
        );
        assert!(
            restored.footnotes == before.footnotes
                && restored.footnotes_part_name == before.footnotes_part_name
                && restored.comments == before.comments
                && restored.comments_extended == before.comments_extended
                && restored.comments_part_name == before.comments_part_name
                && *restored.styles == *before.styles
                && restored.settings == before.settings
                && restored.settings_part_name == before.settings_part_name
                && restored.settings_owned == before.settings_owned
                && restored.font_table == before.font_table
                && restored.font_table_part_name == before.font_table_part_name
                && restored.document.body.sect_pr == before.document.body.sect_pr
                && restored.document.background_xml == before.document.background_xml
                && restored.document.background_extra_xml == before.document.background_extra_xml,
            "an in-place edit changed a model outside its units ({:?})",
            undo.units
        );
        assert_eq!(
            restored.to_bytes().map_err(|error| error.to_string()),
            before.to_bytes().map_err(|error| error.to_string()),
            "an in-place edit wrote outside its units ({:?})",
            undo.units
        );
    }

    /// Debug proof that an in-place edit made what `staged`, today's staged
    /// route run on `before` (a copy taken before the edit), makes: the same
    /// answer or error text, and the same saved bytes; a refusal leaves the
    /// document as it was.
    pub(crate) fn assert_in_place_matches_staged<T: PartialEq + std::fmt::Debug>(
        &self,
        before: Self,
        result: &Result<T>,
        staged: impl FnOnce(&mut Self) -> Result<T>,
    ) {
        self.assert_in_place_matches_staged_by(before, result, staged, |live, staged| {
            assert_eq!(
                live, staged,
                "in-place edit saves other bytes than the staged edit"
            );
        });
    }

    /// [`Self::assert_in_place_matches_staged`] for an in-place edit that
    /// writes its element from the model while the staged route writes it
    /// from a fragment that declares its namespaces again: the two packages
    /// hold the same parts, and the same bytes once the namespace
    /// declarations, and the whitespace a producer writes between the XML
    /// declaration and the root element (which an in-place edit keeps and a
    /// staged copy drops), are set aside.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_in_place_matches_staged_declarations_aside<
        T: PartialEq + std::fmt::Debug,
    >(
        &self,
        before: Self,
        result: &Result<T>,
        staged: impl FnOnce(&mut Self) -> Result<T>,
    ) {
        self.assert_in_place_matches_staged_by(before, result, staged, |live, staged| {
            let parts = |saved: &std::result::Result<Vec<u8>, String>| {
                saved.as_ref().map_err(String::clone).map(|bytes| {
                    let package =
                        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes))
                            .expect("a saved package reopens");
                    let mut parts = package
                        .parts
                        .iter()
                        .map(|(name, bytes)| {
                            (
                                name.clone(),
                                without_namespace_declarations(&without_declaration_whitespace(
                                    bytes,
                                )),
                            )
                        })
                        .collect::<Vec<_>>();
                    parts.sort();
                    parts
                })
            };
            assert!(
                parts(live) == parts(staged),
                "in-place edit saves other parts than the staged edit, namespace declarations aside"
            );
        });
    }

    fn assert_in_place_matches_staged_by<T: PartialEq + std::fmt::Debug>(
        &self,
        mut before: Self,
        result: &Result<T>,
        staged: impl FnOnce(&mut Self) -> Result<T>,
        same_saves: impl FnOnce(
            &std::result::Result<Vec<u8>, String>,
            &std::result::Result<Vec<u8>, String>,
        ),
    ) {
        let unchanged = before
            .clone_for_staging()
            .to_bytes()
            .map_err(|e| e.to_string());
        let staged_result = staged(&mut before);
        let live = self
            .clone_for_staging()
            .to_bytes()
            .map_err(|e| e.to_string());
        match (result, &staged_result) {
            (Ok(direct), Ok(staged_value)) => {
                assert_eq!(direct, staged_value, "in-place edit answered otherwise");
                same_saves(&live, &before.to_bytes().map_err(|e| e.to_string()));
                assert_eq!(
                    self.package_signatures_invalidated, before.package_signatures_invalidated,
                    "in-place edit invalidates signatures otherwise than the staged edit"
                );
            }
            (Err(direct), Err(staged_error)) => {
                assert_eq!(direct.to_string(), staged_error.to_string());
                assert_eq!(
                    live, unchanged,
                    "a refused in-place edit changed the document"
                );
            }
            // A document that could not be saved before the edit (a typed
            // edit of a body that cannot be saved after one) fails the staged
            // route at its save. An edit that leaves the body alone is made
            // in place and leaves that error to the save, as a section edit
            // does: the save still reports it.
            (Ok(_), Err(staged_error))
                if unchanged.is_err()
                    && live == unchanged
                    && live == Err(staged_error.to_string()) => {}
            (Ok(direct), Err(staged_error)) => {
                panic!("in-place edit {direct:?} hid the staged error {staged_error}")
            }
            (Err(direct), Ok(_)) => panic!("in-place edit refused with {direct}"),
        }
    }
}

/// `xml` without the whitespace between its XML declaration and the
/// element after it: a producer writes a newline there, which an edit in
/// place keeps with the rest of the part's bytes while a part written from
/// a staged copy does not.
#[cfg(debug_assertions)]
fn without_declaration_whitespace(xml: &[u8]) -> Vec<u8> {
    let Some(end) = xml
        .strip_prefix(b"<?xml".as_slice())
        .and_then(|rest| rest.windows(2).position(|pair| pair == b"?>"))
        .map(|at| at + b"<?xml".len() + 2)
    else {
        return xml.to_vec();
    };
    let after = xml[end..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(xml.len() - end);
    let mut out = Vec::with_capacity(xml.len());
    out.extend_from_slice(&xml[..end]);
    out.extend_from_slice(&xml[end + after..]);
    out
}

/// `xml` without the namespace declarations of its tags (` xmlns="..."`,
/// ` xmlns:p="..."` inside `<...>`); text and other bytes as they are. A
/// prefix bound again to another namespace is not told apart: the oracle
/// that uses this compares what both routes wrote from one model.
#[cfg(debug_assertions)]
pub(crate) fn without_namespace_declarations(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut in_tag = false;
    let mut quote = None;
    let mut at = 0;
    while at < xml.len() {
        let byte = xml[at];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if in_tag && (byte == b'"' || byte == b'\'') => quote = Some(byte),
            None if in_tag && byte == b'>' => in_tag = false,
            None if byte == b'<' => in_tag = true,
            None if in_tag
                && (xml[at..].starts_with(b" xmlns=") || xml[at..].starts_with(b" xmlns:")) =>
            {
                // Skip the attribute: its name, `=` and its quoted value.
                let Some(open) = xml[at..].iter().position(|b| *b == b'"' || *b == b'\'') else {
                    break;
                };
                let delimiter = xml[at + open];
                let Some(close) = xml[at + open + 1..].iter().position(|b| *b == delimiter) else {
                    break;
                };
                at += open + 1 + close + 1;
                continue;
            }
            None => {}
        }
        out.push(byte);
        at += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edit that writes a part, reserves a part name and changes the
    /// comments model, then fails as `fail` says.
    fn edit_then(document: &mut Document, fail: impl FnOnce() -> Result<()>) -> Result<()> {
        document.edit_in_place(&[EditUnit::Comments], |document| {
            document
                .package
                .set_part("/word/extra.xml", b"<extra/>".to_vec());
            document
                .identifiers
                .reserve_preferred_part_name("/word/reserved.xml")?;
            document.comments = Some(oxoffice_docx_oxml::comments::CT_Comments::new());
            document.comments_dirty = true;
            fail()
        })
    }

    fn state(document: &Document) -> (Vec<u8>, bool, bool) {
        (
            document.clone_for_staging().to_bytes().unwrap(),
            document.comments.is_some(),
            document.package.get_part("/word/extra.xml").is_some(),
        )
    }

    #[test]
    fn a_refusal_after_a_custom_xml_store_and_a_removal_leaves_the_document_as_it_was() {
        let mut document = Document::new();
        document
            .set_document_variable("kept", "value")
            .expect("a variable is set");
        let before = document.clone_for_staging().to_bytes().unwrap();
        let identifiers = document.identifiers.clone();
        let result: Result<()> =
            document.edit_in_place(&[EditUnit::Settings, EditUnit::Parts], |document| {
                crate::bibliography::add_custom_xml_store_in_place(document, b"<data/>", &[])?;
                document.remove_document_variable("kept")?;
                let settings = document
                    .settings_part_name
                    .clone()
                    .expect("a settings part");
                document.package.remove_part(&settings);
                document.package.remove_part_rels(&settings);
                document.package.remove_content_type_override(&settings);
                document.identifiers.retire_authored_part(&settings);
                Err(Error::Other("refused".to_owned()))
            });
        assert!(result.is_err());
        assert_eq!(document.identifiers, identifiers);
        assert_eq!(document.clone_for_staging().to_bytes().unwrap(), before);
    }

    /// The relationships of the main part, as their ids, types and targets.
    fn main_relationships(document: &Document) -> Option<Vec<oxoffice_opc::Relationship>> {
        document
            .package
            .get_part_rels(&document.doc_part_name)
            .map(|relationships| relationships.items.clone())
    }

    #[test]
    fn a_refusal_puts_back_relationships_removed_whole_then_created_again() {
        let mut document = Document::new();
        let owner = document.doc_part_name.clone();
        let before = main_relationships(&document);
        assert!(before.as_ref().is_some_and(|items| !items.is_empty()));
        let result: Result<()> = document.edit_in_place(&[EditUnit::Parts], |document| {
            document.package.remove_part_rels(&owner);
            assert!(document.package.part_journal_records_rels(&owner));
            document.add_relationship_journaled(Some(&owner), "rId90", "urn:test", "a.xml");
            document.add_external_relationship_journaled(
                &owner,
                "rId91",
                "urn:test",
                "https://example.com",
            );
            Err(Error::Other("refused".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(main_relationships(&document), before);
    }

    #[test]
    fn a_refusal_puts_back_a_relationship_removed_then_added_again_with_its_id() {
        let mut document = Document::new();
        let owner = document.doc_part_name.clone();
        let before = main_relationships(&document);
        let first = before.as_ref().unwrap()[0].clone();
        let result: Result<()> = document.edit_in_place(&[EditUnit::Parts], |document| {
            let removed = document
                .package
                .remove_relationships_where(&owner, |item| item.id == first.id);
            assert_eq!(removed, std::slice::from_ref(&first));
            document.add_relationship_journaled(Some(&owner), &first.id, "urn:test", "b.xml");
            Err(Error::Other("refused".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(main_relationships(&document), before);
    }

    #[test]
    fn a_refused_in_place_edit_leaves_the_document_as_it_was() {
        let mut document = Document::new();
        let before = state(&document);
        let identifiers = document.identifiers.clone();
        let error = edit_then(&mut document, || Err(Error::Other("refused".into()))).unwrap_err();
        assert_eq!(error.to_string(), "refused");
        assert_eq!(state(&document), before);
        assert_eq!(document.identifiers, identifiers);
        assert!(!document.in_place.open);
    }

    #[test]
    fn a_panic_inside_an_in_place_edit_leaves_the_document_as_it_was() {
        let mut document = Document::new();
        let before = state(&document);
        let identifiers = document.identifiers.clone();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            edit_then(&mut document, || panic!("edit panicked"))
        }));
        assert!(unwound.is_err());
        assert_eq!(state(&document), before);
        assert_eq!(document.identifiers, identifiers);
        assert!(!document.in_place.open);
    }

    /// The run "!" appended to the first body paragraph: typed on `live`,
    /// by the staged byte splice (which repeats the root's namespace
    /// declarations on the paragraph) with `staged_text` on the oracle's copy.
    #[cfg(debug_assertions)]
    fn check_appended_run(staged_text: &'static str) {
        let mut document = Document::new();
        document.add_paragraph("Alpha");
        let before = document.clone_for_staging();
        let location = before
            .direct_body_item_location(0)
            .unwrap()
            .expect("the paragraph has a location");
        let BodyContent::Paragraph(paragraph) = &mut document.document.body.content[0] else {
            panic!("the first block is the paragraph");
        };
        paragraph
            .insert_accepted_run(1, CT_R::new("!"))
            .expect("the paragraph takes a run at its end");
        document.assert_in_place_matches_staged_declarations_aside(before, &Ok(()), |staged| {
            let mut candidate = staged.clone_for_staging();
            candidate.insert_story_run_staged(
                &crate::StoryRunPosition {
                    location,
                    run_index: 1,
                },
                CT_R::new(staged_text),
            )?;
            let reopened = candidate.prepare_and_reopen_staged()?;
            staged.commit_staged_mutation(reopened);
            Ok(())
        });
    }

    #[test]
    #[cfg(debug_assertions)]
    fn the_oracle_takes_a_respliced_paragraph_for_the_same_paragraph() {
        check_appended_run("!");
    }

    #[test]
    #[should_panic(expected = "in-place edit saves other parts than the staged edit")]
    #[cfg(debug_assertions)]
    fn the_oracle_sees_a_difference_inside_a_respliced_paragraph() {
        check_appended_run("?");
    }

    /// Five paragraphs, "p0" to "p4".
    fn five_paragraphs() -> Document {
        let mut document = Document::new();
        for index in 0..5 {
            document.add_paragraph(&format!("p{index}"));
        }
        document
    }

    fn body_texts(document: &Document) -> Vec<String> {
        document.paragraphs().iter().map(|p| p.text()).collect()
    }

    /// Two disjoint units: the first keeps its length, the last loses its
    /// block. The report tells both, and the undo puts both back.
    #[test]
    fn several_body_block_units_report_and_undo_each() {
        let mut document = five_paragraphs();
        let texts = body_texts(&document);
        let units = [
            EditUnit::BodyBlocks { slot: 1, count: 1 },
            EditUnit::BodyBlocks { slot: 3, count: 1 },
        ];
        document.hold_edit_undo();
        document
            .edit_in_place(&units, |document| {
                let content = &mut document.document.body.content;
                content[1] = content[0].clone();
                content.remove(3);
                document.invalidate_layout();
                Ok(())
            })
            .unwrap();
        let undo = document
            .take_edit_undo()
            .expect("the edit changed the body");
        assert_eq!(body_texts(&document), ["p0", "p0", "p2", "p4"]);
        let report = undo.report();
        assert_eq!(
            report.body_blocks,
            [
                BodyBlocksChange {
                    slot: 1,
                    removed: 1,
                    inserted: 1
                },
                BodyBlocksChange {
                    slot: 3,
                    removed: 1,
                    inserted: 0
                },
            ]
        );
        assert_eq!(report.body, None);
        assert!(report.units.iter().all(|(_, changed)| *changed));
        document.undo_edit(undo);
        assert_eq!(body_texts(&document), texts);
    }

    /// A unit the edit left alone is reported unchanged, the other changed.
    #[test]
    fn an_untouched_body_block_unit_is_reported_unchanged() {
        let mut document = five_paragraphs();
        let units = [
            EditUnit::BodyBlocks { slot: 0, count: 1 },
            EditUnit::BodyBlocks { slot: 2, count: 2 },
        ];
        document.hold_edit_undo();
        document
            .edit_in_place(&units, |document| {
                let content = &mut document.document.body.content;
                content.insert(4, content[0].clone());
                document.invalidate_layout();
                Ok(())
            })
            .unwrap();
        let undo = document
            .take_edit_undo()
            .expect("the edit changed the body");
        let report = undo.report();
        assert_eq!(
            report.body_blocks,
            [BodyBlocksChange {
                slot: 2,
                removed: 2,
                inserted: 3
            }]
        );
        assert_eq!(
            report.units,
            [(units[0].clone(), false), (units[1].clone(), true)]
        );
        document.undo_edit(undo);
        assert_eq!(body_texts(&document), ["p0", "p1", "p2", "p3", "p4"]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "body block units are disjoint and in slot order")]
    fn overlapping_body_block_units_are_refused() {
        let mut document = five_paragraphs();
        let units = [
            EditUnit::BodyBlocks { slot: 1, count: 2 },
            EditUnit::BodyBlocks { slot: 2, count: 1 },
        ];
        let _ = document.edit_in_place(&units, |_| Ok(()));
    }

    /// An insertion unit before a replaced block at the same slot: only the
    /// unit that changed is reported changed.
    #[test]
    fn two_units_at_one_slot_report_their_own_change() {
        let mut document = five_paragraphs();
        let units = [
            EditUnit::BodyBlocks { slot: 3, count: 0 },
            EditUnit::BodyBlocks { slot: 3, count: 1 },
        ];
        document.hold_edit_undo();
        document
            .edit_in_place(&units, |document| {
                let content = &mut document.document.body.content;
                content[3] = content[0].clone();
                document.invalidate_layout();
                Ok(())
            })
            .unwrap();
        let undo = document
            .take_edit_undo()
            .expect("the edit changed the body");
        assert_eq!(
            undo.report().units,
            [(units[0].clone(), false), (units[1].clone(), true)]
        );
        document.undo_edit(undo);
        assert_eq!(body_texts(&document), ["p0", "p1", "p2", "p3", "p4"]);
    }

    /// A unit before the last that changes its number of blocks breaks the
    /// contract: the debug undo oracle refuses the edit.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic]
    fn a_unit_before_the_last_that_changes_its_count_is_refused() {
        let mut document = five_paragraphs();
        let units = [
            EditUnit::BodyBlocks { slot: 1, count: 1 },
            EditUnit::BodyBlocks { slot: 3, count: 1 },
        ];
        let _ = document.edit_in_place(&units, |document| {
            let content = &mut document.document.body.content;
            content.insert(1, content[0].clone());
            document.invalidate_layout();
            Ok(())
        });
    }
}
