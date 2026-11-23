//! Citations placed after a piece of main-story text.

use oxoffice_docx_oxml::text::{CT_R, Field, FieldForm, RunContent};

use super::{CitationOptions, bibliography_error, citation_instruction};
use crate::document::visit_body_paragraphs_mut;
use crate::{Document, Result};

impl Document {
    /// Insert a complex citation right after the `occurrence`-th match of
    /// `text` in the main story, counted from zero.
    ///
    /// Matches are found as [`Self::add_comment_on_text`] finds them:
    /// case-sensitive and non-overlapping, in document order through body
    /// paragraphs, tables (nested ones included) and block content controls,
    /// never across two paragraphs, in the literal run text that
    /// [`Self::split_run`] offsets count. The run holding the end of the
    /// match is split there and the citation goes at that boundary. A missing
    /// occurrence, or a boundary where no run can go, is an error and leaves
    /// the document unchanged.
    pub fn insert_citation_after_text(
        &mut self,
        text: &str,
        occurrence: usize,
        options: &CitationOptions,
    ) -> Result<()> {
        if text.is_empty() {
            return Err(bibliography_error("citation anchor text must not be empty"));
        }
        let field = Field::from_instruction(
            citation_instruction(options)?,
            FieldForm::Complex,
            Vec::new(),
        )?;
        field
            .validate_for_attachment()
            .map_err(|error| bibliography_error(error.to_string()))?;
        let mut run = CT_R::new("");
        run.content = vec![RunContent::Field(field)];
        let mut candidate = self.clone_for_staging();
        let mut remaining = occurrence;
        let mut placed = None;
        visit_body_paragraphs_mut(&mut candidate.document.body.content, &mut |paragraph| {
            if placed.is_some() {
                return;
            }
            let literal = paragraph.accepted_literal_text();
            let matches = literal
                .match_indices(text)
                .map(|(byte, _)| byte)
                .collect::<Vec<_>>();
            let Some(byte) = matches.get(remaining) else {
                remaining -= matches.len();
                return;
            };
            let start = literal[..*byte].chars().count();
            let end = start + text.chars().count();
            placed = Some(
                paragraph
                    .split_accepted_literal_span(start, end)
                    .map_err(|error| {
                        bibliography_error(format!("citation anchor text cannot be split: {error}"))
                    })
                    .and_then(|(_, end)| {
                        paragraph
                            .insert_accepted_run(end, run.clone())
                            .map_err(|error| {
                                bibliography_error(format!("citation insertion boundary: {error}"))
                            })
                    }),
            );
        });
        placed.ok_or_else(|| {
            // Every paragraph was searched, so `remaining` counts past them all.
            let found = occurrence - remaining;
            let times = if found == 1 { "time" } else { "times" };
            bibliography_error(format!(
                "citation anchor text {text:?} has no occurrence {occurrence}: it occurs {found} {times} in the main story"
            ))
        })??;
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}
