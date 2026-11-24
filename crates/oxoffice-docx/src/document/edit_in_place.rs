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
}

/// What an in-place edit changed, for the caller that publishes it.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditReport {
    /// Each unit the edit named, and whether it changed.
    pub units: Vec<(EditUnit, bool)>,
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
    /// The endnotes part's bytes before the edit, for its no-op answer.
    endnotes: Option<Option<Vec<u8>>>,
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
            endnotes: None,
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
            scalars,
            parts,
            reservations,
            ..
        } = undo;
        self.package.undo_part_journal(parts);
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
        self.embedded_invalidated_signatures = scalars.embedded_invalidated_signatures;
        self.freed_relationships = scalars.freed_relationships;
        self.package_signatures_invalidated = scalars.package_signatures_invalidated;
        self.invalidate_layout();
    }

    /// Whether the units of `undo` differ from what the document holds now.
    fn report_changes(&self, undo: &EditUndo) -> EditReport {
        EditReport {
            units: undo
                .units
                .iter()
                .map(|unit| {
                    let changed = match unit {
                        EditUnit::Footnotes => undo.footnotes.as_ref().is_some_and(|copy| {
                            copy.footnotes != self.footnotes
                                || copy.part_name != self.footnotes_part_name
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
                    };
                    (unit.clone(), changed)
                })
                .collect(),
        }
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
        if undo.report.changed() {
            document.invalidate_layout();
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
            endnotes,
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
                && *restored.styles == *before.styles,
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
        mut before: Self,
        result: &Result<T>,
        staged: impl FnOnce(&mut Self) -> Result<T>,
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
                assert_eq!(
                    live,
                    before.to_bytes().map_err(|e| e.to_string()),
                    "in-place edit saves other bytes than the staged edit"
                );
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
            (Ok(direct), Err(staged_error)) => {
                panic!("in-place edit {direct:?} hid the staged error {staged_error}")
            }
            (Err(direct), Ok(_)) => panic!("in-place edit refused with {direct}"),
        }
    }
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
}
