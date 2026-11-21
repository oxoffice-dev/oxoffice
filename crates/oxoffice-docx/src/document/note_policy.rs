//! Footnote and endnote options: families, special records, numbering, restart and placement.

use super::*;

/// Which of Word's independent note streams a policy governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteFamily {
    Footnote,
    Endnote,
}

/// Special content placed before or after a note that continues across pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSpecialRecord {
    Separator,
    ContinuationSeparator,
    ContinuationNotice,
}

impl NoteSpecialRecord {
    pub(super) fn note_type(self) -> oxoffice_docx_oxml::footnotes::NoteType {
        use oxoffice_docx_oxml::footnotes::NoteType;
        match self {
            Self::Separator => NoteType::Separator,
            Self::ContinuationSeparator => NoteType::ContinuationSeparator,
            Self::ContinuationNotice => NoteType::ContinuationNotice,
        }
    }
}

/// Marker formats the native renderer can reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteNumberFormat {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetter,
    LowerLetter,
}

impl NoteNumberFormat {
    fn xml_value(self) -> &'static str {
        match self {
            Self::Decimal => "decimal",
            Self::UpperRoman => "upperRoman",
            Self::LowerRoman => "lowerRoman",
            Self::UpperLetter => "upperLetter",
            Self::LowerLetter => "lowerLetter",
        }
    }
}

/// Boundary at which numeric note markers start over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteRestart {
    Continuous,
    EachSection,
    EachPage,
}

impl NoteRestart {
    fn xml_value(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::EachSection => "eachSect",
            Self::EachPage => "eachPage",
        }
    }
}

/// Boundary at which note content appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePlacement {
    PageBottom,
    BeneathText,
    SectionEnd,
    DocumentEnd,
}

impl NotePlacement {
    fn xml_value(self) -> &'static str {
        match self {
            Self::PageBottom => "pageBottom",
            Self::BeneathText => "beneathText",
            Self::SectionEnd => "sectEnd",
            Self::DocumentEnd => "docEnd",
        }
    }
}

/// A complete visible note policy. `start` is one-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotePolicy {
    pub format: NoteNumberFormat,
    pub start: u32,
    pub restart: NoteRestart,
    pub placement: NotePlacement,
}

impl NotePolicy {
    pub(super) fn properties(self, family: NoteFamily) -> Result<CT_NoteProperties> {
        if self.start == 0 {
            return Err(Error::Other(
                "note numbering must start above zero".to_owned(),
            ));
        }
        let valid = matches!(
            (family, self.placement),
            (
                NoteFamily::Footnote,
                NotePlacement::PageBottom | NotePlacement::BeneathText
            ) | (
                NoteFamily::Endnote,
                NotePlacement::SectionEnd | NotePlacement::DocumentEnd
            )
        );
        if !valid {
            return Err(Error::Other(
                "placement is invalid for this note family".to_owned(),
            ));
        }
        Ok(CT_NoteProperties {
            pos: Some(self.placement.xml_value().to_owned()),
            num_fmt: Some(self.format.xml_value().to_owned()),
            num_start: Some(self.start),
            num_restart: Some(self.restart.xml_value().to_owned()),
            ..CT_NoteProperties::default()
        })
    }
}
