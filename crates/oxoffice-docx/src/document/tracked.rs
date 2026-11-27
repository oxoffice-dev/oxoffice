//! Tracked changes written by edits: a session, its author and date, and the
//! edits it records as revisions (spec of 2026-11-27, writing revisions and
//! comments).

use super::*;
use crate::comments::RevisionDate;

/// Who and when the edits of a tracked session are recorded as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedChanges {
    author: String,
    timestamp: Option<String>,
}

impl TrackedChanges {
    /// The changes of `author`, dated as `date` says. [`RevisionDate::Now`]
    /// is the time of this call, which every revision of the session takes.
    /// An empty author, a character XML 1.0 cannot carry or a date that is
    /// not RFC 3339 is refused.
    pub fn new(author: &str, date: RevisionDate<'_>) -> Result<Self> {
        if author.is_empty() {
            return Err(Error::Other("a tracked change needs an author".to_owned()));
        }
        oxoffice_core::xml::reject_non_xml_characters("revision author", author)?;
        let timestamp = match date {
            RevisionDate::Now => Some(crate::revision::rfc3339_now()?),
            RevisionDate::At(value) => {
                crate::revision::validate_revision_timestamp(value)?;
                Some(value.to_owned())
            }
            RevisionDate::Undated => None,
        };
        Ok(Self {
            author: author.to_owned(),
            timestamp,
        })
    }

    pub fn author(&self) -> &str {
        &self.author
    }

    /// The `w:date` of each revision, `None` when they are undated.
    pub fn timestamp(&self) -> Option<&str> {
        self.timestamp.as_deref()
    }
}

/// The session a tracked edit runs in: its changes and the ids of every
/// revision it wrote, shared by the staged copies of the document an edit
/// makes.
#[derive(Debug, Clone)]
pub(crate) struct Tracking {
    pub(crate) changes: TrackedChanges,
    ids: Arc<Mutex<Vec<i32>>>,
    /// What the replacement under way searches and writes: Word deletes each
    /// whole match and inserts the whole replacement (Word check W12).
    pub(crate) pattern: Option<Arc<ReplacePattern>>,
}

/// The pair of a replacement made under tracked changes.
#[derive(Debug)]
pub(crate) enum ReplacePattern {
    /// A literal text and the text written for it, line endings read as
    /// replacement writes them.
    Literal {
        placeholder: String,
        written: String,
    },
    /// A regular expression and its replacement, capture groups included.
    Regex {
        regex: regex::Regex,
        replacement: String,
    },
}

impl ReplacePattern {
    /// A literal pair, its value's line endings read as replacement writes
    /// them: each one line break.
    pub(crate) fn literal(placeholder: &str, replacement: &str) -> Self {
        ReplacePattern::Literal {
            placeholder: placeholder.to_owned(),
            written: replacement.replace("\r\n", "\n").replace('\r', "\n"),
        }
    }
}

impl Document {
    /// Run `replace` with the session under way, if any, told the pattern it
    /// replaces.
    pub(crate) fn with_replace_pattern<T>(
        &mut self,
        pattern: impl FnOnce() -> ReplacePattern,
        replace: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let Some(tracking) = &mut self.tracking else {
            return replace(self);
        };
        tracking.pattern = Some(Arc::new(pattern()));
        let result = replace(self);
        if let Some(tracking) = &mut self.tracking {
            tracking.pattern = None;
        }
        result
    }
}

impl Tracking {
    /// The ids the session wrote so far.
    pub(crate) fn ids(&self) -> Vec<i32> {
        self.ids.lock().expect("tracking ids").clone()
    }

    /// Record ids an edit wrote, once each: a debug cross-check that makes
    /// the same edit again on a copy writes the same ones.
    pub(crate) fn record(&self, written: &[i32]) {
        let mut ids = self.ids.lock().expect("tracking ids");
        for id in written {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
}

/// A document whose edits are written as tracked changes, made by
/// [`Document::track_changes`].
///
/// It holds the document mutably: while it lives, no call it does not offer,
/// and no handle that edits without marks (such as
/// [`Document::paragraph_mut`]), can reach the document. Each call has the
/// name and the arguments of the [`Document`] call it tracks, and makes the
/// same edit, written as insertions and deletions by the session's author at
/// its date. An edit the session cannot write as tracked changes (a change
/// of formatting, the removal of a drawing or a field, text put in a new
/// link) is refused and leaves the document as it was.
pub struct TrackedDocument<'a> {
    document: &'a mut Document,
    tracking: Tracking,
}

impl Document {
    /// Write the edits made through the returned session as tracked changes
    /// by `changes`' author, at its date.
    pub fn track_changes(&mut self, changes: &TrackedChanges) -> TrackedDocument<'_> {
        TrackedDocument {
            document: self,
            tracking: Tracking {
                changes: changes.clone(),
                ids: Arc::default(),
                pattern: None,
            },
        }
    }

    /// `after`, an edit of paragraph `before`, written as the tracked
    /// changes of the session under way, if any.
    pub(crate) fn redline_edited_paragraph(&mut self, before: &CT_P, after: CT_P) -> Result<CT_P> {
        match &self.tracking {
            Some(tracking) => {
                super::redline::redline_paragraph(before, &after, tracking, &mut self.identifiers)
            }
            None => Ok(after),
        }
    }

    /// Write as tracked changes each body paragraph that differs from the
    /// same one of `before`, the body content before the edit under way.
    pub(crate) fn redline_body(&mut self, before: &[BodyContent]) -> Result<()> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(());
        };
        let mut originals = Vec::new();
        visit_body_paragraphs(before, false, &mut |paragraph| originals.push(paragraph));
        let identifiers = &mut self.identifiers;
        let mut index = 0;
        let mut failure = None;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if failure.is_some() {
                return;
            }
            let Some(original) = originals.get(index) else {
                failure = Some(super::redline::refused("an edit that adds a paragraph"));
                return;
            };
            index += 1;
            if *original != paragraph {
                match super::redline::redline_paragraph(original, paragraph, &tracking, identifiers)
                {
                    Ok(redlined) => *paragraph = redlined,
                    Err(error) => failure = Some(error),
                }
            }
        });
        match failure {
            Some(error) => Err(error),
            None if index != originals.len() => {
                Err(super::redline::refused("an edit that removes a paragraph"))
            }
            None => {
                self.invalidate_layout();
                Ok(())
            }
        }
    }
}

impl Document {
    /// Write as tracked changes each paragraph of header or footer `after`
    /// that differs from the same one of `before`, the part before the edit
    /// under way. A table or a control of the part that changed is refused.
    pub(crate) fn redline_header_footer(
        &mut self,
        before: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
        after: &mut oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    ) -> Result<()> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(());
        };
        if before.extra_xml != after.extra_xml || before.paragraphs.len() != after.paragraphs.len()
        {
            return Err(super::redline::refused(
                "a replacement in a header or footer table or control",
            ));
        }
        for (original, paragraph) in before.paragraphs.iter().zip(&mut after.paragraphs) {
            if original != paragraph {
                *paragraph = super::redline::redline_paragraph(
                    original,
                    paragraph,
                    &tracking,
                    &mut self.identifiers,
                )?;
            }
        }
        Ok(())
    }
}

/// Whether the cell story `story` lies in a text box, as a table a text box
/// holds: its owner's ancestors read up to its start.
fn cell_in_text_box(document: &Document, story: &StoryId) -> Result<bool> {
    let (source, owner) = document.story_source_and_owner(story)?;
    let mut reader =
        oxoffice_core::xml_reader::Reader::from_reader(&source.xml[..owner.full.start]);
    let mut open = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => open.push(element.local_name().as_ref() == b"txbxContent"),
            Ok(Event::End(_)) => {
                open.pop();
            }
            Ok(Event::Eof) => return Ok(open.contains(&true)),
            Ok(_) => {}
            Err(error) => return Err(Error::Other(format!("cell story XML: {error}"))),
        }
    }
}

impl TrackedDocument<'_> {
    /// Who and when the session's revisions are by.
    pub fn changes(&self) -> &TrackedChanges {
        &self.tracking.changes
    }

    /// The ids of the revisions this session wrote, in the order it wrote
    /// them.
    pub fn revision_ids(&self) -> Vec<i32> {
        self.tracking.ids()
    }

    /// The document, to read.
    pub fn document(&self) -> &Document {
        self.document
    }

    fn run<T>(&mut self, call: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
        /// Puts back the document's session when the call returns or
        /// unwinds, so that a panic leaves no session behind.
        struct Held<'a> {
            document: &'a mut Document,
            tracking: Option<Tracking>,
        }
        impl Drop for Held<'_> {
            fn drop(&mut self) {
                self.document.tracking = self.tracking.take();
            }
        }
        let written = self.tracking.ids().len();
        let tracking = self.document.tracking.replace(self.tracking.clone());
        let held = Held {
            document: self.document,
            tracking,
        };
        let result = call(held.document);
        drop(held);
        if result.is_err() {
            self.forget_since(written);
        }
        result
    }

    /// [`Self::run`] for a call that refuses with `Ok(Err(_))`: that refusal
    /// too leaves the document as it was, and so the session's ids.
    fn run_refusable<T, E>(
        &mut self,
        call: impl FnOnce(&mut Document) -> Result<std::result::Result<T, E>>,
    ) -> Result<std::result::Result<T, E>> {
        let written = self.tracking.ids().len();
        let result = self.run(call);
        if matches!(result, Ok(Err(_))) {
            self.forget_since(written);
        }
        result
    }

    /// Forget the ids recorded after the first `written`: the revisions
    /// that took them were never kept.
    fn forget_since(&self, written: usize) {
        self.tracking
            .ids
            .lock()
            .expect("tracking ids")
            .truncate(written);
    }

    /// [`Document::edit_story_paragraph`]: `edit` on the paragraph at
    /// `location`, in any story, written as tracked changes.
    pub fn edit_paragraph<T>(
        &mut self,
        location: &ContentLocation,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        self.run(|document| document.edit_story_paragraph(location, edit))
    }

    /// `edit` on body paragraph `index`, counted as [`Document::paragraph`]
    /// counts it, written as tracked changes.
    pub fn edit_body_paragraph<T>(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        self.run(|document| {
            let before = document
                .paragraph(index)
                .ok_or_else(|| Error::Other("paragraph index out of range".to_owned()))?
                .inner
                .clone();
            let mut after = before.clone();
            let value = edit(&mut Paragraph { inner: &mut after })?;
            document.localize_run_styles(&mut after);
            let redlined = document.redline_edited_paragraph(&before, after)?;
            *document
                .paragraph_mut(index)
                .expect("the paragraph was just read")
                .inner = redlined;
            Ok(value)
        })
    }

    /// [`Document::edit_cell_paragraph`], written as tracked changes.
    pub fn edit_cell_paragraph<T>(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        paragraph: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        self.run(|document| document.edit_cell_paragraph(table, row, cell, paragraph, edit))
    }

    /// [`Document::set_story_text`], written as tracked changes: the same
    /// edit, the first run's formatting kept, then its redline.
    pub fn set_story_text(&mut self, location: &ContentLocation, value: &str) -> Result<()> {
        self.run(|document| {
            let mut untracked = document.clone_for_staging();
            untracked.tracking = None;
            untracked.set_story_text(location, value)?;
            // The edit changed the story, and so its fingerprint.
            let story = untracked
                .stories()?
                .into_iter()
                .find(|story| {
                    story.kind == location.story.kind
                        && story.part_name == location.story.part_name
                        && story.owner_index == location.story.owner_index
                })
                .ok_or_else(|| Error::Other("the edited story is gone".to_owned()))?;
            let edited_location = ContentLocation {
                story,
                ..location.clone()
            };
            let snapshot = untracked.story_range_paragraph_snapshot(&edited_location)?;
            let edited = CT_P::from_xml_fragment(snapshot.xml())?;
            document.edit_story_paragraph(location, |paragraph| {
                *paragraph.inner = edited;
                Ok(())
            })
        })
    }

    /// [`Document::try_replace_text`], written as tracked changes as Word's
    /// tracked Replace All writes them: each whole match deleted and its
    /// whole value inserted. A value equal to its match counts, as in the
    /// untracked call, and writes nothing. A match in a text box, in a
    /// header or footer table or control, or one with a bookmark edge inside
    /// it, refuses the call.
    pub fn try_replace_text(&mut self, placeholder: &str, replacement: &str) -> Result<usize> {
        self.run(|document| document.try_replace_text(placeholder, replacement))
    }

    /// [`Document::try_replace_text_at`] on a paragraph, written as tracked
    /// changes as [`Self::try_replace_text`] writes them. A table or a block
    /// control as the location, or a text box paragraph, is refused.
    pub fn try_replace_text_at(
        &mut self,
        location: &ContentLocation,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<usize, ReplacementCountMismatch>> {
        if location.item_kind() != StoryItemKind::Paragraph {
            return Err(super::redline::refused(
                "a replacement in a whole table or control",
            ));
        }
        if location.story().kind() == StoryKind::TextBox
            || location.story().kind() == StoryKind::TableCell
                && cell_in_text_box(self.document, location.story())?
        {
            return Err(super::redline::refused("a replacement in a text box"));
        }
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let mut mismatch = None;
        let result = self.run(|document| {
            document.with_replace_pattern(
                || ReplacePattern::literal(placeholder, replacement),
                |document| {
                    document.edit_story_paragraph(location, |paragraph| {
                        let found = oxoffice_docx_oxml::placeholder::try_replace_in_paragraph(
                            paragraph.inner,
                            placeholder,
                            replacement,
                        )?;
                        match expected {
                            Some(expected) if expected != found => {
                                mismatch = Some(ReplacementCountMismatch {
                                    index: 0,
                                    placeholder: placeholder.to_owned(),
                                    expected,
                                    found,
                                    unreached: 0,
                                });
                                Err(Error::Other("replacement count mismatch".to_owned()))
                            }
                            _ => Ok(found),
                        }
                    })
                },
            )
        });
        match (result, mismatch) {
            (_, Some(mismatch)) => Ok(Err(mismatch)),
            (result, None) => result.map(Ok),
        }
    }

    /// [`Document::try_replace_all_checked`], written as tracked changes.
    pub fn try_replace_all_checked(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
        options: ReplaceOptions,
    ) -> Result<std::result::Result<Vec<usize>, ReplacementRefusal>> {
        self.run_refusable(|document| document.try_replace_all_checked(pairs, options))
    }

    /// [`Document::try_replace_all_regex_expected`], written as tracked
    /// changes.
    pub fn try_replace_all_regex_expected(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Vec<usize>, ReplacementCountMismatch>> {
        self.run_refusable(|document| document.try_replace_all_regex_expected(pairs))
    }
}
