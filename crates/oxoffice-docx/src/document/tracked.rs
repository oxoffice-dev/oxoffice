//! Tracked changes written by edits: a session, its author and date, and the
//! edits it records as revisions (spec of 2026-11-27, writing revisions and
//! comments).

use super::*;
use crate::comments::RevisionDate;
use oxoffice_docx_oxml::table::CellContent;

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

impl Document {
    /// [`Self::set_story_text`] under a session: the untracked edit made on a
    /// copy, its paragraph then written as tracked changes, so that it keeps
    /// what the untracked call keeps (the first run's formatting).
    pub(crate) fn set_story_text_tracked(
        &mut self,
        location: &ContentLocation,
        value: &str,
    ) -> Result<()> {
        let mut untracked = self.clone_for_staging();
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
        self.edit_story_paragraph(location, |paragraph| {
            *paragraph.inner = edited;
            Ok(())
        })
    }

    /// [`Self::try_replace_text_at`] under a session, on a paragraph: each
    /// whole match deleted and its value inserted, as the replace calls
    /// write them. A table or a block control as the location, or a text
    /// box paragraph, is refused.
    pub(crate) fn replace_text_at_tracked(
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
                && cell_in_text_box(self, location.story())?
        {
            return Err(super::redline::refused("a replacement in a text box"));
        }
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let mut mismatch = None;
        let result = self.with_replace_pattern(
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
        );
        match (result, mismatch) {
            (_, Some(mismatch)) => Ok(Err(mismatch)),
            (result, None) => result.map(Ok),
        }
    }

    /// [`Self::try_set_cell_text`] under a session: the untracked edit made
    /// on a copy of the cell, its paragraph then written as tracked changes.
    /// A cell of more than one paragraph, whose others the untracked edit
    /// removes, is refused.
    pub(crate) fn set_cell_text_tracked(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        text: &str,
    ) -> Result<()> {
        let mut edited = self.read_cell(table, row, cell, Clone::clone)?;
        if edited.content.len() != 1 {
            return Err(super::redline::refused("a change of a cell's paragraphs"));
        }
        crate::table::Cell { inner: &mut edited }.try_set_text(text)?;
        let [CellContent::Paragraph(edited)] = edited.content.as_slice() else {
            return Err(super::redline::refused("a change of a cell's paragraphs"));
        };
        let edited = edited.clone();
        self.edit_cell_paragraphs_tracked(table, row, cell, 0..1, |paragraph| {
            *paragraph = edited.clone();
            Ok(0)
        })
        .map(|_| ())
    }

    /// [`Self::try_replace_text_in_cell`] under a session: each paragraph of
    /// the cell, or the one given, replaced as [`Self::replace_text_at_tracked`]
    /// replaces it. A cell that holds a table is refused.
    pub(crate) fn replace_in_cell_tracked(
        &mut self,
        (table, row, cell): (usize, usize, usize),
        paragraph: Option<usize>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<usize, ReplacementCountMismatch>> {
        let count = self.read_cell(table, row, cell, |cell| {
            cell.content
                .iter()
                .map(|content| match content {
                    CellContent::Paragraph(_) => Ok(()),
                    _ => Err(super::redline::refused("a replacement in a nested table")),
                })
                .collect::<Result<Vec<_>>>()
                .map(|paragraphs| paragraphs.len())
        })??;
        let paragraphs = match paragraph {
            Some(paragraph) if paragraph < count => paragraph..paragraph + 1,
            Some(paragraph) => {
                return Err(Error::Other(format!(
                    "cell paragraph {paragraph} is out of range"
                )));
            }
            None => 0..count,
        };
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let found = self.with_replace_pattern(
            || ReplacePattern::literal(placeholder, replacement),
            |document| {
                document.edit_cell_paragraphs_tracked(table, row, cell, paragraphs, |paragraph| {
                    Ok(oxoffice_docx_oxml::placeholder::try_replace_in_paragraph(
                        paragraph,
                        placeholder,
                        replacement,
                    )?)
                })
            },
        )?;
        Ok(match expected {
            Some(expected) if expected != found => Err(ReplacementCountMismatch {
                index: 0,
                placeholder: placeholder.to_owned(),
                expected,
                found,
                unreached: 0,
            }),
            _ => Ok(found),
        })
    }

    /// `edit` on each paragraph `paragraphs` of the cell, its result written
    /// as tracked changes of the paragraph before it, and the cell published
    /// through one staged transaction, as [`Self::try_set_cell_text`]
    /// publishes it. The sum of what `edit` returns.
    fn edit_cell_paragraphs_tracked(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        paragraphs: Range<usize>,
        mut edit: impl FnMut(&mut CT_P) -> Result<usize>,
    ) -> Result<usize> {
        let before = self.read_cell(table, row, cell, Clone::clone)?;
        let mut after = before.clone();
        let mut sum = 0;
        for index in paragraphs {
            let (Some(CellContent::Paragraph(old)), Some(CellContent::Paragraph(new))) =
                (before.content.get(index), after.content.get_mut(index))
            else {
                return Err(Error::Other(format!(
                    "cell paragraph {index} is out of range"
                )));
            };
            sum += edit(new)?;
            self.localize_run_styles(new);
            if new != old {
                *new = self.redline_edited_paragraph(old, new.clone())?;
            }
        }
        if after == before {
            return Ok(sum);
        }
        let mut candidate = self.clone_for_staging();
        *nth_table_in_body(&mut candidate.document.body.content, &mut table.clone())
            .and_then(|table| crate::table::table_cell_mut(table, row, cell))
            .ok_or_else(|| Error::Other(format!("cell {row}:{cell} is out of range")))? = after;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(sum)
    }

    /// `read` on cell `cell` of row `row` of body table `table`.
    fn read_cell<T>(
        &self,
        table: usize,
        row: usize,
        cell: usize,
        read: impl FnOnce(&oxoffice_docx_oxml::table::CT_Tc) -> T,
    ) -> Result<T> {
        // Counted as `table_mut` counts tables, which the edit takes.
        let table = nth_table_in_body_ref(&self.document.body.content, &mut table.clone())
            .ok_or_else(|| Error::Other(format!("table index {table} is out of range")))?;
        let cell = crate::table::table_cell(table, row, cell)
            .ok_or_else(|| Error::Other(format!("cell {row}:{cell} is out of range")))?;
        Ok(read(cell))
    }

    /// [`Self::replace_text_at_tracked`] on a document a caller owns: `None`
    /// when nothing matched.
    pub(crate) fn replace_text_at_tracked_owned(
        mut self,
        location: &ContentLocation,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        Ok(
            match self.replace_text_at_tracked(location, placeholder, replacement, expected)? {
                Err(mismatch) => Err(mismatch),
                Ok(0) => Ok(None),
                Ok(count) => Ok(Some((self, count))),
            },
        )
    }
}

impl Document {
    /// `after`, table `before` with its properties edited, written as tracked
    /// changes when a session runs.
    pub(crate) fn redline_edited_table(
        &mut self,
        before: &CT_Tbl,
        after: CT_Tbl,
    ) -> Result<CT_Tbl> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(after);
        };
        let xml = |table: &CT_Tbl| -> Result<String> {
            String::from_utf8(ContentFragment::table(table.clone())?.xml)
                .map_err(|error| Error::Other(error.to_string()))
        };
        let redlined = super::redline::redline_table_xml(
            &xml(before)?,
            &xml(&after)?,
            &tracking,
            &mut self.identifiers,
        )?;
        Ok(CT_Tbl::from_xml_fragment_checked(redlined.as_bytes())?)
    }

    /// Under a session, `block`, a body block placed (or about to be) at body
    /// index `slot`, written as inserted, as Word writes it: a paragraph's
    /// content and mark, each row of a table. With `last`, the block ends the
    /// body: a paragraph's mark becomes the story's final mark, which is
    /// never marked (Word check W12); the mark of the paragraph before it is
    /// the inserted one ([`Self::marked_paragraph_mark`]), and the
    /// new paragraph's properties, which that mark takes, are a change of
    /// that paragraph's.
    #[doc(hidden)]
    pub fn marked_inserted_block(
        &mut self,
        block: BodyContent,
        slot: usize,
        last: bool,
    ) -> Result<BodyContent> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(block);
        };
        let local = match &block {
            BodyContent::Paragraph(_) => "p",
            BodyContent::Table(_) => "tbl",
            _ => {
                return Err(super::redline::refused(
                    "the insertion of a content control",
                ));
            }
        };
        let xml = block_xml(&block)?;
        let previous = match slot
            .checked_sub(1)
            .map(|slot| &self.document.body.content[slot])
        {
            Some(paragraph @ BodyContent::Paragraph(_)) if last => Some(block_xml(paragraph)?),
            _ => None,
        };
        let marked = super::redline::inserted_block_xml(
            &xml,
            &tracking,
            last && local == "p",
            true,
            previous.as_deref(),
        )?;
        let marked =
            super::redline::give_ids(&marked, &tracking, &mut self.identifiers, |marked| {
                super::redline::check_inserted(marked, &xml)
            })?;
        Ok(match local {
            "p" => BodyContent::Paragraph(Box::new(CT_P::from_xml_fragment(marked.as_bytes())?)),
            _ => BodyContent::Table(Box::new(CT_Tbl::from_xml_fragment_checked(
                marked.as_bytes(),
            )?)),
        })
    }

    /// Under a session, body paragraph `slot` with its mark written as
    /// inserted, as a paragraph appended after it writes it (Word check
    /// W12), for [`Self::place_marked_mark`]; nothing is placed yet. The mark
    /// of a paragraph that ends a section is refused: rejecting it would
    /// drop the section break.
    #[doc(hidden)]
    pub fn marked_paragraph_mark(&mut self, slot: usize) -> Result<CT_P> {
        self.marked_paragraph_mark_as(slot, false)
    }

    /// [`Self::marked_paragraph_mark`], the mark written as inserted, or as
    /// deleted when the blocks after it are removed to the body's end.
    fn marked_paragraph_mark_as(&mut self, slot: usize, deleted: bool) -> Result<CT_P> {
        let (xml, marked) = self.paragraph_mark_xml(slot, deleted)?;
        self.marked_mark_with_ids(&xml, &marked, deleted)
    }

    /// Body paragraph `slot` as written, and with its mark written as
    /// inserted or deleted, its id to be given: every refusal comes here,
    /// before an id is taken.
    fn paragraph_mark_xml(&self, slot: usize, deleted: bool) -> Result<(String, String)> {
        let Some(tracking) = &self.tracking else {
            return Err(Error::Other("no tracked changes session is on".to_owned()));
        };
        let Some(block @ BodyContent::Paragraph(paragraph)) = self.document.body.content.get(slot)
        else {
            return Err(Error::Other("body paragraph index out of range".to_owned()));
        };
        if paragraph
            .properties
            .as_ref()
            .is_some_and(|properties| properties.sect_pr.is_some())
        {
            return Err(super::redline::refused(match deleted {
                true => "the removal of the paragraphs after a section break",
                false => "a paragraph added after a section break",
            }));
        }
        if self.block_holds_nested_namespace_owner(block)? {
            return Err(super::redline::refused(
                "a change of a paragraph that declares namespaces of its own",
            ));
        }
        let xml = block_xml(block)?;
        let marked = match deleted {
            true => super::redline::deleted_block_xml(&xml, tracking, false, false, None)?,
            false => super::redline::inserted_block_xml(&xml, tracking, false, false, None)?,
        };
        Ok((xml, marked))
    }

    /// `marked`, from [`Self::paragraph_mark_xml`], with its id given.
    fn marked_mark_with_ids(&mut self, xml: &str, marked: &str, deleted: bool) -> Result<CT_P> {
        let tracking = self.tracking.clone().expect("a session is on");
        let marked =
            super::redline::give_ids(marked, &tracking, &mut self.identifiers, |marked| {
                match deleted {
                    // The content stays: rejected, the paragraph is as it was.
                    true => super::redline::check_rejected(marked, xml),
                    false => super::redline::check_inserted(marked, xml),
                }
            })?;
        Ok(CT_P::from_xml_fragment(marked.as_bytes())?)
    }

    /// Put `paragraph`, which [`Self::marked_paragraph_mark`] wrote, at body
    /// slot `slot`; the body stays as savable as it was.
    #[doc(hidden)]
    pub fn place_marked_mark(&mut self, slot: usize, paragraph: CT_P) {
        let savable = matches!(self.unsavable_body_edit_cache.get(), Some(None));
        self.document.body.content[slot] = BodyContent::Paragraph(Box::new(paragraph));
        self.invalidate_layout();
        self.keep_body_savable_after_typed_edits(slot..slot + 1, savable);
    }

    /// Under a session, `block` written as inserted and placed at body
    /// `slot`, with the mark before it when it ends the body (Word check
    /// W12); a refusal places nothing.
    #[doc(hidden)]
    pub fn insert_tracked_block(&mut self, slot: usize, block: BodyContent) -> Result<()> {
        let paragraph = matches!(block, BodyContent::Paragraph(_));
        let last = slot == self.document.body.content.len();
        // The mark before first: its refusals come before any id is taken.
        let before = self.mark_before_placing(slot, paragraph);
        let mark = before
            .map(|before| self.marked_paragraph_mark(before))
            .transpose()?;
        let marked = self.marked_inserted_block(block, slot, last)?;
        self.insert_body_block(slot, marked);
        if let (Some(before), Some(mark)) = (before, mark) {
            self.place_marked_mark(before, mark);
        }
        Ok(())
    }

    /// Under a session, body blocks `start..end` written as removed, in
    /// place, as Word writes the deletion of whole blocks: each paragraph's
    /// content and mark, each row of a table. When they end the body with a
    /// paragraph, its mark, the story's final mark, stays and takes the
    /// properties of the paragraph before them, whose mark is the deleted one
    /// (Word check W12). Returns the blocks as they were, and that paragraph
    /// as it was.
    #[doc(hidden)]
    pub fn mark_removed_body_blocks(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<(Vec<BodyContent>, Option<CT_P>)> {
        let Some(tracking) = self.tracking.clone() else {
            return Err(Error::Other("no tracked changes session is on".to_owned()));
        };
        let content = &self.document.body.content;
        if start >= end || end > content.len() {
            return Err(Error::Other(
                "content range is reversed or out of range".to_owned(),
            ));
        }
        // A bookmark or another zero-width marker written between blocks
        // stays where it is, as no text of its own goes.
        let marker = |block: &BodyContent| match block {
            BodyContent::RawXml(raw) => raw_local_name(raw).is_some_and(|local| {
                local.ends_with("RangeStart")
                    || local.ends_with("RangeEnd")
                    || matches!(local, "bookmarkStart" | "bookmarkEnd" | "proofErr")
            }),
            _ => false,
        };
        // The paragraph whose mark is the story's final one, when the blocks
        // take it: its mark stays and the one before them is the deleted
        // one (Word check W12).
        let kept = content
            .iter()
            .rposition(|block| matches!(block, BodyContent::Paragraph(_) | BodyContent::Table(_)))
            .filter(|&slot| {
                (start..end).contains(&slot) && matches!(content[slot], BodyContent::Paragraph(_))
            });
        let before = match kept.map(|_| content[..start].iter().rposition(|block| !marker(block))) {
            None => None,
            Some(Some(slot)) if matches!(content[slot], BodyContent::Paragraph(_)) => Some(slot),
            Some(Some(_)) => {
                return Err(super::redline::refused(
                    "the removal of the body's last paragraphs after a table or a content control",
                ));
            }
            Some(None) => {
                return Err(super::redline::refused(
                    "the removal of every paragraph of the body",
                ));
            }
        };
        let previous = before.map(|slot| block_xml(&content[slot])).transpose()?;
        // Every refusal before any id is taken: each block's kind, then each
        // block written as removed.
        let mut written = Vec::with_capacity(end - start);
        for slot in start..end {
            let block = &self.document.body.content[slot];
            match block {
                _ if marker(block) => continue,
                BodyContent::RawXml(raw) => {
                    return Err(super::redline::refused(&format!(
                        "the removal of a {} between blocks",
                        raw_local_name(raw).unwrap_or("raw element")
                    )));
                }
                BodyContent::ContentControl(_) => {
                    return Err(super::redline::refused("the removal of a content control"));
                }
                BodyContent::Paragraph(paragraph)
                    if paragraph
                        .properties
                        .as_ref()
                        .is_some_and(|properties| properties.sect_pr.is_some()) =>
                {
                    return Err(super::redline::refused("the removal of a section break"));
                }
                BodyContent::Paragraph(_) | BodyContent::Table(_) => {}
            }
            if self.block_holds_nested_namespace_owner(block)? {
                return Err(super::redline::refused(
                    "the removal of a block that declares namespaces of its own",
                ));
            }
            let xml = block_xml(block)?;
            let keep = kept == Some(slot);
            let marked = super::redline::deleted_block_xml(
                &xml,
                &tracking,
                keep,
                true,
                previous.as_deref().filter(|_| keep),
            )?;
            written.push((slot, xml, marked));
        }
        let mark = before
            .map(|slot| self.paragraph_mark_xml(slot, true))
            .transpose()?;
        // The ids, once nothing is refused.
        let mark = mark
            .map(|(xml, marked)| self.marked_mark_with_ids(&xml, &marked, true))
            .transpose()?;
        let mut marked = Vec::with_capacity(written.len());
        for (slot, xml, written) in written {
            let written =
                super::redline::give_ids(&written, &tracking, &mut self.identifiers, |written| {
                    super::redline::check_removed(written, &xml)
                })?;
            marked.push((
                slot,
                match &self.document.body.content[slot] {
                    BodyContent::Paragraph(_) => BodyContent::Paragraph(Box::new(
                        CT_P::from_xml_fragment(written.as_bytes())?,
                    )),
                    _ => BodyContent::Table(Box::new(CT_Tbl::from_xml_fragment_checked(
                        written.as_bytes(),
                    )?)),
                },
            ));
        }
        let savable = matches!(self.unsavable_body_edit_cache.get(), Some(None));
        let mut old = self.document.body.content[start..end].to_vec();
        for (slot, block) in marked {
            old[slot - start] = std::mem::replace(&mut self.document.body.content[slot], block);
        }
        self.invalidate_layout();
        self.keep_body_savable_after_typed_edits(start..end, savable);
        let before = match (before, mark) {
            (Some(slot), Some(mark)) => {
                let Some(BodyContent::Paragraph(old)) = self.document.body.content.get(slot) else {
                    unreachable!("the paragraph before was read above");
                };
                let old = (**old).clone();
                self.place_marked_mark(slot, mark);
                Some(old)
            }
            _ => None,
        };
        Ok((old, before))
    }

    /// Under a session, row `row` of body table `index` written as inserted
    /// (a row added or copied) or as removed (it stays, marked), as Word
    /// writes them: `w:trPr/w:ins` or `w:trPr/w:del`, its cells' paragraphs
    /// marked too. Costs that row alone.
    #[doc(hidden)]
    pub fn mark_body_table_row(&mut self, index: usize, row: usize, deleted: bool) -> Result<()> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(());
        };
        let table = nth_table_in_body_ref(&self.document.body.content, &mut index.clone())
            .ok_or_else(|| Error::Other("table index out of range".to_owned()))?;
        if crate::table::table_holds_controls(table) {
            return Err(super::redline::refused(
                "a row of a table that holds a content control",
            ));
        }
        if self.block_holds_nested_namespace_owner(&BodyContent::Table(Box::new(table.clone())))? {
            return Err(super::redline::refused(
                "a row of a table that declares namespaces of its own",
            ));
        }
        let current = crate::table::table_row(table, row)
            .ok_or_else(|| Error::Other("row index out of range".to_owned()))?
            .clone();
        // The table's properties and grid with the row alone.
        let frame = CT_Tbl {
            properties: table.properties.clone(),
            grid: table.grid.clone(),
            rows: vec![current],
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
        };
        let xml = block_xml(&BodyContent::Table(Box::new(frame)))?;
        let marked = match deleted {
            true => super::redline::deleted_block_xml(&xml, &tracking, false, true, None)?,
            false => super::redline::inserted_block_xml(&xml, &tracking, false, true, None)?,
        };
        let marked =
            super::redline::give_ids(&marked, &tracking, &mut self.identifiers, |marked| {
                match deleted {
                    true => super::redline::check_removed(marked, &xml),
                    false => super::redline::check_inserted(marked, &xml),
                }
            })?;
        let mut parsed = CT_Tbl::from_xml_fragment_checked(marked.as_bytes())?;
        let table = nth_table_in_body(&mut self.document.body.content, &mut index.clone())
            .expect("the table read above");
        *crate::table::table_row_mut(table, row).expect("the row read above") =
            parsed.rows.remove(0);
        self.invalidate_layout();
        Ok(())
    }

    /// Put back the blocks [`Self::mark_removed_body_blocks`] marked from
    /// `start`, and the paragraph before them.
    #[doc(hidden)]
    pub fn restore_removed_body_blocks(
        &mut self,
        start: usize,
        blocks: Vec<BodyContent>,
        before: Option<CT_P>,
    ) {
        for (offset, block) in blocks.into_iter().enumerate() {
            self.document.body.content[start + offset] = block;
        }
        if let Some(paragraph) = before {
            self.document.body.content[start - 1] = BodyContent::Paragraph(Box::new(paragraph));
        }
        self.invalidate_layout();
    }

    /// Under a session, the body blocks `self` holds that `before`, the
    /// document a staged insertion was made from, does not: written as
    /// inserted. An edit that also changes another body block or another
    /// story is refused.
    #[doc(hidden)]
    pub fn mark_body_insertion_since(&mut self, before: &Document) -> Result<()> {
        if self.tracking.is_none() {
            return Ok(());
        }
        // Blocks compared as written, as a staged edit may bind their
        // namespaces anew.
        let written = |document: &Document| -> Result<Vec<String>> {
            document
                .document
                .body
                .content
                .iter()
                .map(|block| {
                    let xml = document.standalone_body_item(block)?.1;
                    Ok(super::redline::without_declarations(
                        &String::from_utf8_lossy(&xml),
                    ))
                })
                .collect()
        };
        let (old, new) = (written(before)?, written(self)?);
        let prefix = old
            .iter()
            .zip(&new)
            .take_while(|(old, new)| old == new)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(old, new)| old == new)
            .count();
        if prefix + suffix != old.len() {
            return Err(super::redline::refused("a change of existing blocks"));
        }
        self.refuse_edits_outside_the_body(before)?;
        let count = new.len() - old.len();
        self.mark_inserted_body_blocks(prefix, count)
    }

    /// Under a session, the body paragraphs `self` changed from `before`,
    /// the document a staged edit was made from, written as tracked changes
    /// (a picture or a field added to a run). An edit that adds or removes
    /// a paragraph, or that changes another story, is refused.
    #[doc(hidden)]
    pub fn redline_body_since(&mut self, before: &Document) -> Result<()> {
        if self.tracking.is_none() {
            return Ok(());
        }
        self.refuse_edits_outside_the_body(before)?;
        self.redline_body(&before.document.body.content)
    }

    /// Refuse an edit that changed a story other than the body from
    /// `before`, as written.
    fn refuse_edits_outside_the_body(&self, before: &Document) -> Result<()> {
        let elsewhere = |document: &Document| -> Result<Vec<(ContentLocation, String)>> {
            Ok(document
                .story_item_snapshots()?
                .into_iter()
                .filter(|item| item.location.story().kind() != StoryKind::Body)
                .map(|item| {
                    let xml = String::from_utf8_lossy(&item.xml).into_owned();
                    (item.location, super::redline::without_declarations(&xml))
                })
                .collect())
        };
        match elsewhere(before)? == elsewhere(self)? {
            true => Ok(()),
            false => Err(super::redline::refused("an insertion outside the body")),
        }
    }

    /// Place `block` at body index `slot`, a block
    /// [`Self::marked_inserted_block`] wrote, where a splice placed its plan.
    #[doc(hidden)]
    pub fn insert_body_block(&mut self, slot: usize, block: BodyContent) {
        self.invalidate_layout();
        self.document.body.content.insert(slot, block);
    }

    /// The body paragraph whose mark the blocks `slot..slot + count`, just
    /// placed, must write as inserted: under a session, the paragraph before
    /// blocks that end the body with a paragraph (Word check W12).
    #[doc(hidden)]
    pub fn mark_before_insertion(&self, slot: usize, count: usize) -> Option<usize> {
        let content = &self.document.body.content;
        (self.tracking.is_some()
            && slot > 0
            && slot + count == content.len()
            && matches!(content.last(), Some(BodyContent::Paragraph(_)))
            && matches!(content.get(slot - 1), Some(BodyContent::Paragraph(_))))
        .then(|| slot - 1)
    }

    /// [`Self::mark_before_insertion`] for a block, a paragraph or not, to
    /// be placed at `slot`.
    #[doc(hidden)]
    pub fn mark_before_placing(&self, slot: usize, paragraph: bool) -> Option<usize> {
        let content = &self.document.body.content;
        (self.tracking.is_some()
            && paragraph
            && slot > 0
            && slot == content.len()
            && matches!(content.get(slot - 1), Some(BodyContent::Paragraph(_))))
        .then(|| slot - 1)
    }

    /// Under a session, body blocks `slot..slot + count`, which a splice
    /// just put there, written as inserted (see
    /// [`Self::marked_inserted_block`]), with the mark before them when
    /// they follow the body's last paragraph.
    #[doc(hidden)]
    pub fn mark_inserted_body_blocks(&mut self, slot: usize, count: usize) -> Result<()> {
        let len = self.document.body.content.len();
        if self.tracking.is_none() || count == 0 || slot + count > len {
            return Ok(());
        }
        let last = slot + count == len;
        for index in slot..slot + count {
            let block = std::mem::replace(
                &mut self.document.body.content[index],
                BodyContent::Paragraph(Box::default()),
            );
            let marked = self.marked_inserted_block(block.clone(), index, last && index == len - 1);
            self.document.body.content[index] = match marked {
                Ok(marked) => marked,
                Err(error) => {
                    self.document.body.content[index] = block;
                    return Err(error);
                }
            };
        }
        if let Some(before) = self.mark_before_insertion(slot, count) {
            let mark = self.marked_paragraph_mark(before)?;
            self.place_marked_mark(before, mark);
        }
        self.invalidate_layout();
        Ok(())
    }

    /// The properties of section `index`, to give
    /// [`Self::redline_section_edit`] once they are edited.
    #[doc(hidden)]
    pub fn section_properties_before_edit(&mut self, index: usize) -> Option<CT_SectPr> {
        self.tracking.as_ref()?;
        self.section_mut(index).map(|section| section.inner.clone())
    }

    /// Under a session, write the edit of section `index` from `before` as a
    /// tracked change, or put `before` back and refuse.
    #[doc(hidden)]
    pub fn redline_section_edit(&mut self, index: usize, before: CT_SectPr) -> Result<()> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(());
        };
        let missing = || Error::Other("section index out of range".to_owned());
        let after = self.section_mut(index).ok_or_else(missing)?.inner.clone();
        if after == before {
            return Ok(());
        }
        let xml = |properties: &CT_SectPr| -> Result<String> {
            let mut written = Vec::new();
            properties.to_xml(&mut quick_xml::Writer::new(&mut written))?;
            let written =
                String::from_utf8(written).map_err(|error| Error::Other(error.to_string()))?;
            // Declared on its root, as a fragment of its own.
            let at = written.find([' ', '>', '/']).unwrap_or(written.len());
            Ok(format!(
                "{} xmlns:w=\"{}\" xmlns:r=\"{}\"{}",
                &written[..at],
                WORD_NAMESPACE,
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                &written[at..]
            ))
        };
        let tracked = xml(&before).and_then(|old| {
            let new = xml(&after)?;
            let redlined =
                super::redline::redline_section_xml(&old, &new, &tracking, &mut self.identifiers)?;
            Ok(CT_SectPr::from_xml_fragment_checked(redlined.as_bytes())?)
        });
        let section = self.section_mut(index).ok_or_else(missing)?;
        match tracked {
            Ok(tracked) => {
                *section.inner = tracked;
                Ok(())
            }
            Err(error) => {
                *section.inner = before;
                Err(error)
            }
        }
    }
}

/// `block` written alone, with its namespace declarations.
fn block_xml(block: &BodyContent) -> Result<String> {
    let fragment = match block {
        BodyContent::Paragraph(paragraph) => ContentFragment::paragraph((**paragraph).clone())?,
        BodyContent::Table(table) => ContentFragment::table((**table).clone())?,
        _ => {
            return Err(super::redline::refused(
                "the insertion of a content control",
            ));
        }
    };
    String::from_utf8(fragment.xml).map_err(|error| Error::Other(error.to_string()))
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

    /// Forget ids of the session's changes a later change of the same owner
    /// replaced, holding the old value they held.
    pub(crate) fn forget(&self, replaced: &[i32]) {
        self.ids
            .lock()
            .expect("tracking ids")
            .retain(|id| !replaced.contains(id));
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
        let mut content = std::mem::take(&mut self.document.body.content);
        let result = self.redline_blocks(before, &mut content);
        self.document.body.content = content;
        result?;
        self.invalidate_layout();
        Ok(())
    }

    /// Write as tracked changes each paragraph of `after`, body blocks, that
    /// differs from the same one of `before`: the blocks before the edit.
    pub(crate) fn redline_blocks(
        &mut self,
        before: &[BodyContent],
        after: &mut [BodyContent],
    ) -> Result<()> {
        let Some(tracking) = self.tracking.clone() else {
            return Ok(());
        };
        let mut originals = Vec::new();
        visit_body_paragraphs(before, false, &mut |paragraph| originals.push(paragraph));
        let identifiers = &mut self.identifiers;
        let mut index = 0;
        let mut failure = None;
        visit_body_paragraphs_mut(after, &mut |paragraph| {
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
            None => Ok(()),
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
        self.run(|document| document.set_story_text(location, value))
    }

    /// [`Document::insert_paragraph`] written as an insertion, as Word writes
    /// it: the body index of the new paragraph.
    pub fn insert_paragraph(&mut self, index: usize, text: &str) -> Result<usize> {
        self.run(|document| {
            if index > document.content_count() {
                return Err(Error::Other("body index out of range".to_owned()));
            }
            let slot = document.block_insertion_index(index);
            let paragraph = Document::detached_text_paragraph(text);
            document.insert_tracked_block(slot, BodyContent::Paragraph(Box::new(paragraph)))?;
            Ok(slot)
        })
    }

    /// [`Document::add_paragraph`] written as an insertion: the body index
    /// of the new paragraph.
    pub fn add_paragraph(&mut self, text: &str) -> Result<usize> {
        self.run(|document| {
            let slot = document.content_count();
            let paragraph = Document::detached_text_paragraph(text);
            document.insert_tracked_block(slot, BodyContent::Paragraph(Box::new(paragraph)))?;
            Ok(slot)
        })
    }

    /// [`Document::remove_content_range`] written as a deletion, as Word
    /// writes it: the blocks stay, marked removed.
    pub fn remove_content_range(&mut self, start: usize, end: usize) -> Result<()> {
        self.run(|document| document.mark_removed_body_blocks(start, end).map(|_| ()))
    }

    /// [`Document::insert_table`] written as an insertion, each row marked:
    /// the body index of the new table.
    pub fn insert_table(&mut self, index: usize, rows: usize, cols: usize) -> Result<usize> {
        self.run(|document| {
            if index > document.content_count() {
                return Err(Error::Other("body index out of range".to_owned()));
            }
            let slot = document.block_insertion_index(index);
            let table = Document::detached_table(rows, cols);
            document.insert_tracked_block(slot, BodyContent::Table(Box::new(table)))?;
            Ok(slot)
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
        self.run_refusable(|document| {
            document.try_replace_text_at(location, placeholder, replacement, expected)
        })
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

/// A tracked changes session held across calls, for the Python binding: a
/// handle on the session a document runs its edits in.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct TrackedSession(Tracking);

impl TrackedSession {
    pub fn changes(&self) -> &TrackedChanges {
        &self.0.changes
    }

    /// The ids of the revisions the session wrote that `document` holds, in
    /// the order the session wrote them, read from the document's parts: an
    /// edit refused after its revisions took ids, on a copy it dropped or in
    /// place before it put the paragraph back, leaves none.
    pub fn revision_ids(&self, document: &Document) -> Result<Vec<i32>> {
        let ids = self.0.ids();
        if ids.is_empty() {
            return Ok(ids);
        }
        let mut staged = document.clone_for_staging();
        staged.prepare_staged_package()?;
        let held = DocumentIdentifiers::scan(&staged.package)?.revision_ids;
        Ok(ids.into_iter().filter(|id| held.contains(id)).collect())
    }

    /// Whether `other` is a handle on the same session.
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.ids, &other.0.ids)
    }
}

impl Document {
    /// Run every later edit of the paragraph routes and of replacement in a
    /// tracked changes session by `changes`, until
    /// [`Self::stop_tracked_changes`]. Refused while one runs.
    #[doc(hidden)]
    pub fn start_tracked_changes(&mut self, changes: &TrackedChanges) -> Result<TrackedSession> {
        if self.tracking.is_some() {
            return Err(Error::Other(
                "tracked changes are already on: stop them before starting others".to_owned(),
            ));
        }
        let tracking = Tracking {
            changes: changes.clone(),
            ids: Arc::default(),
            pattern: None,
        };
        self.tracking = Some(tracking.clone());
        Ok(TrackedSession(tracking))
    }

    /// End the session [`Self::start_tracked_changes`] started, if one runs.
    /// The session keeps the ids of the revisions the document holds: those
    /// a refused call took on a copy it dropped go, so that a later session
    /// that takes them again does not lend them to this one.
    #[doc(hidden)]
    pub fn stop_tracked_changes(&mut self) -> Option<TrackedSession> {
        let session = TrackedSession(self.tracking.clone()?);
        if let Ok(held) = session.revision_ids(self) {
            *session.0.ids.lock().expect("tracking ids") = held;
        }
        self.tracking = None;
        Some(session)
    }

    /// The session the document's edits run in, if one runs.
    #[doc(hidden)]
    pub fn tracked_session(&self) -> Option<TrackedSession> {
        self.tracking.clone().map(TrackedSession)
    }

    /// Whether a tracked changes session runs.
    #[doc(hidden)]
    pub fn tracks_changes(&self) -> bool {
        self.tracking.is_some()
    }

    /// Refuse `call`, an edit no tracked changes session records yet, while
    /// one runs: the error a session's refused call raises.
    #[doc(hidden)]
    pub fn refuse_under_tracked_changes(&self, call: &str) -> Result<()> {
        match self.tracking {
            Some(_) => Err(Error::Other(format!(
                "{call} is not recorded as a tracked change yet: end the tracked changes to make it"
            ))),
            None => Ok(()),
        }
    }

    /// `edit` on body paragraph `index`, as [`Self::paragraph_mut`] reaches
    /// it, written as tracked changes when a session runs.
    #[doc(hidden)]
    pub fn edit_paragraph_mut<T>(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        let missing = || Error::Other("paragraph index out of range".to_owned());
        if self.tracking.is_none() {
            return edit(&mut self.paragraph_mut(index).ok_or_else(missing)?);
        }
        let before = self.paragraph(index).ok_or_else(missing)?.inner.clone();
        let mut after = before.clone();
        let value = edit(&mut Paragraph { inner: &mut after })?;
        self.localize_run_styles(&mut after);
        let redlined = self.redline_edited_paragraph(&before, after)?;
        *self.paragraph_mut(index).ok_or_else(missing)?.inner = redlined;
        Ok(value)
    }
}

/// The local name of the element `raw` is written as.
fn raw_local_name(raw: &[u8]) -> Option<&str> {
    let tag = oxoffice_core::xml_element::start_tag(raw).ok()?;
    let name = std::str::from_utf8(raw.get(1..1 + tag.name.len())?).ok()?;
    name.rsplit(':').next()
}
