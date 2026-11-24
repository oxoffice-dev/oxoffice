//! Edits of one direct body paragraph on a copy of that paragraph alone,
//! swapped in when the edit succeeds.

use super::*;

impl Document {
    /// Apply `edit` to paragraph `index` (counted as [`Self::paragraph_mut`]
    /// counts it) without a copy of the document, when that paragraph is a
    /// direct body child: `edit` runs on a copy of the paragraph, which takes
    /// its place when the edit succeeds. Returns the edit's value and the
    /// paragraph it replaced, so that a caller whose own publication refuses
    /// can put it back with [`Self::restore_body_paragraph`].
    ///
    /// An edit that fails or panics leaves the document untouched. A
    /// paragraph that is not a direct body child (inside a block content
    /// control) returns `Ok(None)` before `edit` runs; an index out of range
    /// is an error. Only an edit that changes this one paragraph belongs
    /// here: one that reserves an identifier or adds a relationship does not.
    #[doc(hidden)]
    pub fn edit_body_paragraph_in_place<T>(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<Option<(T, CT_P)>> {
        if self.paragraph(index).is_none() {
            return Err(Error::Other("paragraph index out of range".to_owned()));
        }
        let Some(slot) = self.content_index_of_paragraph(index) else {
            return Ok(None);
        };
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let mut edited = current.as_ref().clone();
        let value = edit(&mut Paragraph { inner: &mut edited })?;
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get_mut(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let old = std::mem::replace(current.as_mut(), edited);
        self.invalidate_layout();
        Ok(Some((value, old)))
    }

    /// Put back `paragraph`, which [`Self::edit_body_paragraph_in_place`]
    /// returned for paragraph `index`, in that paragraph's place. The edited
    /// paragraph it replaces is returned. An index that no longer names a
    /// direct body paragraph is an error and changes nothing.
    #[doc(hidden)]
    pub fn restore_body_paragraph(&mut self, index: usize, paragraph: CT_P) -> Result<CT_P> {
        let slot = self.content_index_of_paragraph(index).ok_or_else(|| {
            Error::Other("paragraph index does not name a direct body paragraph".to_owned())
        })?;
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get_mut(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let edited = std::mem::replace(current.as_mut(), paragraph);
        self.invalidate_layout();
        Ok(edited)
    }
}
