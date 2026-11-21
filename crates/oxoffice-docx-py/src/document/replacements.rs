//! Replacing text through the owner inventory: dormant, expected and scoped replacements.

use super::*;

impl PyDocument {
    /// `check` refuses a pair whose text is left out of its replacement's
    /// reach, as `Document::try_replace_all_checked` does.
    pub(super) fn dormant_replacements(
        &mut self,
        py: Python<'_>,
        pairs: &[(&str, &str, Option<usize>)],
        batch: bool,
        check: Option<oxoffice_docx::ReplaceOptions>,
    ) -> PyResult<Option<(oxoffice_docx::Document, Vec<usize>)>> {
        let candidate = self.inner.clone_for_staging();
        let changed = match check {
            None => py
                .detach(|| candidate.try_replace_all_expected_owned(pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, batch))?,
            Some(options) => py
                .detach(|| candidate.try_replace_all_checked_owned(pairs, options))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|refusal| crate::replacement_refusal_to_pyerr(py, refusal, batch))?,
        };
        if let Some((candidate, _)) = &changed {
            drop(validate_word_facts(candidate, py)?);
        }
        Ok(changed)
    }

    pub(super) fn commit_dormant_replacement(
        &mut self,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        assert!(self.snapshot_scope.is_none());
        let prepared = self
            .revisions
            .prepare_scopes(operation, &[], &[])
            .map_err(revision_to_pyerr)?;
        self.revisions
            .commit_scopes(prepared)
            .map_err(revision_to_pyerr)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Run an ordered replacement batch that publishes only when every
    /// expected count holds, and stale live handles only when it replaced
    /// something.
    pub(super) fn expected_replacements(
        &mut self,
        py: Python<'_>,
        pairs: &[(&str, &str, Option<usize>)],
        batch: bool,
        check: Option<oxoffice_docx::ReplaceOptions>,
    ) -> PyResult<(oxoffice_docx::Document, Vec<usize>)> {
        let mut candidate = self.inner.clone_for_staging();
        let counts = match check {
            None => py
                .detach(|| candidate.try_replace_all_expected(pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, batch))?,
            Some(options) => py
                .detach(|| candidate.try_replace_all_checked(pairs, options))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|refusal| crate::replacement_refusal_to_pyerr(py, refusal, batch))?,
        };
        Ok((candidate, counts))
    }

    // Both checked item and physical cell routes share singular count/error and
    // publication policy. The concrete closures are the two actual consumers.
    pub(crate) fn scoped_replacement<F>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        mutation: F,
    ) -> PyResult<usize>
    where
        F: FnOnce(
                oxoffice_docx::Document,
            ) -> oxoffice_docx::Result<
                Result<
                    Option<(oxoffice_docx::Document, usize)>,
                    oxoffice_docx::ReplacementCountMismatch,
                >,
            > + Send,
    {
        let candidate = self.inner.clone_for_staging();
        let changed = py
            .detach(|| mutation(candidate))
            .map_err(|error| docx_to_pyerr(py, error))?
            .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, false))?;
        let Some((candidate, count)) = changed else {
            return Ok(0);
        };
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            operation,
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(count)
    }
}
