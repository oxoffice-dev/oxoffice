//! The list numbering of a paragraph as the comparison keys it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use oxoffice_docx_oxml::properties::CT_PPr;
use oxoffice_docx_oxml::styles::CT_Styles;

/// The styles of both inputs, to read the level a paragraph `w:numId`
/// without `w:ilvl` shows, and the levels read so far by paragraph style
/// and `w:numId`: each check and signature of a comparison reads them again.
pub(super) struct NumberingLevels {
    styles: [Arc<CT_Styles>; 2],
    agreed: RefCell<HashMap<StyleNumbering, Option<u32>>>,
}

/// A paragraph's style id and `w:numId`.
type StyleNumbering = (Option<String>, u32);

impl NumberingLevels {
    pub(super) fn new(original: &Arc<CT_Styles>, edited: &Arc<CT_Styles>) -> Self {
        Self {
            styles: [original.clone(), edited.clone()],
            agreed: RefCell::default(),
        }
    }

    /// The level a `w:numId` without `w:ilvl` in `properties` shows, when
    /// both inputs agree on it: its style chain's `w:ilvl`, else 0, as Word
    /// draws it. Whichever input the paragraph comes from, that is then its
    /// level; where the inputs' styles disagree, `None` keeps the raw form,
    /// so that a real change of level is never hidden.
    fn agreed_level(&self, properties: &CT_PPr) -> Option<u32> {
        let num_id = properties.num_id.filter(|id| *id != 0)?;
        if properties.num_ilvl.is_some() {
            return None;
        }
        let key = (properties.style_id.clone(), num_id);
        if let Some(level) = self.agreed.borrow().get(&key) {
            return *level;
        }
        let direct = CT_PPr {
            style_id: properties.style_id.clone(),
            num_id: Some(num_id),
            ..Default::default()
        };
        let [original, edited] = [&self.styles[0], &self.styles[1]].map(|styles| {
            crate::document::effective_numbering_reference(styles, &direct).map(|(_, level)| level)
        });
        let level = original.filter(|_| original == edited);
        self.agreed.borrow_mut().insert(key, level);
        level
    }

    /// A paragraph's numbering as signatures compare it: its `(w:numId,
    /// w:ilvl)`, a missing `w:ilvl` read as [`Self::agreed_level`] gives it,
    /// so that a copy writing the level its paragraphs already show (as
    /// LibreOffice writes `w:ilvl 0`) is no change. `None` when it names
    /// neither.
    pub(super) fn key(&self, properties: Option<&CT_PPr>) -> Option<(Option<u32>, Option<u32>)> {
        let properties = properties?;
        (properties.num_id.is_some() || properties.num_ilvl.is_some()).then(|| {
            (
                properties.num_id,
                properties
                    .num_ilvl
                    .or_else(|| self.agreed_level(properties)),
            )
        })
    }

    /// Write into `properties` the level [`Self::agreed_level`] gives a
    /// missing `w:ilvl`, so that comparing paragraph properties reads it as
    /// [`Self::key`] does. Only for comparing: the result keeps each input's
    /// own XML.
    pub(super) fn normalize(&self, properties: &mut CT_PPr) {
        if let Some(level) = self.agreed_level(properties) {
            properties.num_ilvl = Some(level);
        }
    }
}

thread_local! {
    /// The levels of the comparison running on this thread, read by the
    /// signatures and the property checks wherever they are reached from.
    static NUMBERING_LEVELS: RefCell<Option<NumberingLevels>> = const { RefCell::new(None) };
}

/// Sets the [`NumberingLevels`] of a comparison while it lives, and puts the
/// previous ones back when it drops, as `RelationshipClassScope` does for
/// relationship classes.
pub(super) struct NumberingLevelsScope(Option<NumberingLevels>);

impl NumberingLevelsScope {
    pub(super) fn enter(levels: NumberingLevels) -> Self {
        Self(NUMBERING_LEVELS.replace(Some(levels)))
    }
}

impl Drop for NumberingLevelsScope {
    fn drop(&mut self) {
        NUMBERING_LEVELS.set(self.0.take());
    }
}

/// [`NumberingLevels::key`] of the comparison in scope; the raw
/// `(w:numId, w:ilvl)` outside one.
pub(super) fn numbering_key(properties: Option<&CT_PPr>) -> Option<(Option<u32>, Option<u32>)> {
    NUMBERING_LEVELS.with_borrow(|levels| match levels {
        Some(levels) => levels.key(properties),
        None => properties.and_then(|properties| {
            (properties.num_id.is_some() || properties.num_ilvl.is_some())
                .then_some((properties.num_id, properties.num_ilvl))
        }),
    })
}

/// [`NumberingLevels::normalize`] of the comparison in scope; nothing outside
/// one.
pub(super) fn normalize_numbering(properties: &mut CT_PPr) {
    NUMBERING_LEVELS.with_borrow(|levels| {
        if let Some(levels) = levels {
            levels.normalize(properties);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default styles with a paragraph style `id` holding `w:ilvl`
    /// `level` and no `w:numId`, when `level` is given.
    fn styles(id: &str, level: Option<u32>) -> Arc<CT_Styles> {
        let mut styles = CT_Styles::new_default();
        let (mut style, _, _) = crate::StyleBuilder::paragraph(id, id).build();
        style.ppr = level.map(|level| CT_PPr {
            num_ilvl: Some(level),
            ..Default::default()
        });
        styles.styles.push(style);
        Arc::new(styles)
    }

    fn numbered(style: &str, num_id: Option<u32>, level: Option<u32>) -> CT_PPr {
        CT_PPr {
            style_id: Some(style.to_owned()),
            num_id,
            num_ilvl: level,
            ..Default::default()
        }
    }

    #[test]
    fn a_missing_level_reads_as_the_level_both_inputs_show() {
        let plain = styles("Item", None);
        let deep = styles("Item", Some(2));
        for (styles, shown) in [(&plain, 0), (&deep, 2)] {
            let levels = NumberingLevels::new(styles, styles);
            let missing = numbered("Item", Some(5), None);
            assert_eq!(levels.key(Some(&missing)), Some((Some(5), Some(shown))));
            assert_eq!(
                levels.key(Some(&missing)),
                levels.key(Some(&numbered("Item", Some(5), Some(shown))))
            );
            let mut normalized = missing.clone();
            levels.normalize(&mut normalized);
            assert_eq!(normalized.num_ilvl, Some(shown));
        }
        // Level 2 shown, level 0 written: a real change stays one.
        let levels = NumberingLevels::new(&deep, &deep);
        assert_ne!(
            levels.key(Some(&numbered("Item", Some(5), None))),
            levels.key(Some(&numbered("Item", Some(5), Some(0))))
        );
    }

    #[test]
    fn a_missing_level_the_inputs_read_differently_keeps_its_raw_form() {
        let (plain, deep) = (styles("Item", None), styles("Item", Some(2)));
        let levels = NumberingLevels::new(&plain, &deep);
        let mut missing = numbered("Item", Some(5), None);
        assert_eq!(levels.key(Some(&missing)), Some((Some(5), None)));
        levels.normalize(&mut missing);
        assert_eq!(missing.num_ilvl, None);
    }

    #[test]
    fn a_paragraph_that_names_no_list_keeps_its_key() {
        let plain = styles("Item", None);
        let levels = NumberingLevels::new(&plain, &plain);
        assert_eq!(levels.key(None), None);
        assert_eq!(levels.key(Some(&CT_PPr::default())), None);
        // A `w:ilvl` alone, and `w:numId 0`, which takes a paragraph off its list.
        assert_eq!(
            levels.key(Some(&numbered("Item", None, Some(1)))),
            Some((None, Some(1)))
        );
        assert_eq!(
            levels.key(Some(&numbered("Item", Some(0), None))),
            Some((Some(0), None))
        );
    }
}
