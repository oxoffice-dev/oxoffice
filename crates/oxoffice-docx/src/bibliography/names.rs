//! Stable names of the bibliography enums, for the CLI and Python.
//!
//! A name is the variant in snake case (`journal_article`, `book_author`,
//! `reference_order`), except the styles, which use the short names of the
//! style editions (`apa6`, `mla7`, `iso690_numeric`). Names never change once
//! published: they are written in scripts and JSON files.

use std::fmt;
use std::str::FromStr;

use super::{
    BibliographyContributorRole, BibliographySourceField, BibliographySourceKind,
    BibliographyStyle, bibliography_error,
};
use crate::Error;

impl BibliographyStyle {
    /// Every value, in declaration order.
    pub const ALL: [Self; 12] = [
        Self::ApaSixthEdition,
        Self::Chicago,
        Self::Gb7714,
        Self::GostName,
        Self::GostTitle,
        Self::HarvardAnglia,
        Self::Ieee,
        Self::Iso690AuthorDate,
        Self::Iso690Numeric,
        Self::MlaSeventhEdition,
        Self::Sist02,
        Self::Turabian,
    ];

    /// The stable name of this value.
    pub fn name(self) -> &'static str {
        match self {
            Self::ApaSixthEdition => "apa6",
            Self::Chicago => "chicago",
            Self::Gb7714 => "gb7714",
            Self::GostName => "gost_name",
            Self::GostTitle => "gost_title",
            Self::HarvardAnglia => "harvard_anglia",
            Self::Ieee => "ieee",
            Self::Iso690AuthorDate => "iso690_author_date",
            Self::Iso690Numeric => "iso690_numeric",
            Self::MlaSeventhEdition => "mla7",
            Self::Sist02 => "sist02",
            Self::Turabian => "turabian",
        }
    }

    /// The value with this stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

impl fmt::Display for BibliographyStyle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for BibliographyStyle {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::from_name(name)
            .ok_or_else(|| bibliography_error(format!("unknown bibliography style {name:?}")))
    }
}

impl BibliographySourceKind {
    /// Every value, in declaration order.
    pub const ALL: [Self; 17] = [
        Self::ArticleInAPeriodical,
        Self::Book,
        Self::BookSection,
        Self::JournalArticle,
        Self::ConferenceProceedings,
        Self::Report,
        Self::SoundRecording,
        Self::Performance,
        Self::Art,
        Self::DocumentFromInternetSite,
        Self::InternetSite,
        Self::Film,
        Self::Interview,
        Self::Patent,
        Self::ElectronicSource,
        Self::Case,
        Self::Misc,
    ];

    /// The stable name of this value.
    pub fn name(self) -> &'static str {
        match self {
            Self::ArticleInAPeriodical => "article_in_a_periodical",
            Self::Book => "book",
            Self::BookSection => "book_section",
            Self::JournalArticle => "journal_article",
            Self::ConferenceProceedings => "conference_proceedings",
            Self::Report => "report",
            Self::SoundRecording => "sound_recording",
            Self::Performance => "performance",
            Self::Art => "art",
            Self::DocumentFromInternetSite => "document_from_internet_site",
            Self::InternetSite => "internet_site",
            Self::Film => "film",
            Self::Interview => "interview",
            Self::Patent => "patent",
            Self::ElectronicSource => "electronic_source",
            Self::Case => "case",
            Self::Misc => "misc",
        }
    }

    /// The value with this stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

impl fmt::Display for BibliographySourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for BibliographySourceKind {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::from_name(name)
            .ok_or_else(|| bibliography_error(format!("unknown source kind {name:?}")))
    }
}

impl BibliographyContributorRole {
    /// Every value, in declaration order.
    pub const ALL: [Self; 16] = [
        Self::Artist,
        Self::Author,
        Self::BookAuthor,
        Self::Compiler,
        Self::Composer,
        Self::Conductor,
        Self::Counsel,
        Self::Director,
        Self::Editor,
        Self::Interviewee,
        Self::Interviewer,
        Self::Inventor,
        Self::Performer,
        Self::ProducerName,
        Self::Translator,
        Self::Writer,
    ];

    /// The stable name of this value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Author => "author",
            Self::BookAuthor => "book_author",
            Self::Compiler => "compiler",
            Self::Composer => "composer",
            Self::Conductor => "conductor",
            Self::Counsel => "counsel",
            Self::Director => "director",
            Self::Editor => "editor",
            Self::Interviewee => "interviewee",
            Self::Interviewer => "interviewer",
            Self::Inventor => "inventor",
            Self::Performer => "performer",
            Self::ProducerName => "producer_name",
            Self::Translator => "translator",
            Self::Writer => "writer",
        }
    }

    /// The value with this stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

impl fmt::Display for BibliographyContributorRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for BibliographyContributorRole {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::from_name(name)
            .ok_or_else(|| bibliography_error(format!("unknown contributor role {name:?}")))
    }
}

impl BibliographySourceField {
    /// Every value, in declaration order.
    pub const ALL: [Self; 47] = [
        Self::AbbreviatedCaseNumber,
        Self::AlbumTitle,
        Self::BookTitle,
        Self::Broadcaster,
        Self::BroadcastTitle,
        Self::CaseNumber,
        Self::ChapterNumber,
        Self::City,
        Self::Comments,
        Self::ConferenceName,
        Self::CountryRegion,
        Self::Court,
        Self::Day,
        Self::DayAccessed,
        Self::Department,
        Self::Distributor,
        Self::Edition,
        Self::Institution,
        Self::InternetSiteTitle,
        Self::Issue,
        Self::JournalName,
        Self::Medium,
        Self::Month,
        Self::MonthAccessed,
        Self::NumberVolumes,
        Self::Pages,
        Self::PatentNumber,
        Self::PeriodicalTitle,
        Self::ProductionCompany,
        Self::PublicationTitle,
        Self::Publisher,
        Self::RecordingNumber,
        Self::ReferenceOrder,
        Self::Reporter,
        Self::ShortTitle,
        Self::StandardNumber,
        Self::StateProvince,
        Self::Station,
        Self::Theater,
        Self::ThesisType,
        Self::Title,
        Self::PatentType,
        Self::Url,
        Self::Version,
        Self::Volume,
        Self::Year,
        Self::YearAccessed,
    ];

    /// The stable name of this value.
    pub fn name(self) -> &'static str {
        match self {
            Self::AbbreviatedCaseNumber => "abbreviated_case_number",
            Self::AlbumTitle => "album_title",
            Self::BookTitle => "book_title",
            Self::Broadcaster => "broadcaster",
            Self::BroadcastTitle => "broadcast_title",
            Self::CaseNumber => "case_number",
            Self::ChapterNumber => "chapter_number",
            Self::City => "city",
            Self::Comments => "comments",
            Self::ConferenceName => "conference_name",
            Self::CountryRegion => "country_region",
            Self::Court => "court",
            Self::Day => "day",
            Self::DayAccessed => "day_accessed",
            Self::Department => "department",
            Self::Distributor => "distributor",
            Self::Edition => "edition",
            Self::Institution => "institution",
            Self::InternetSiteTitle => "internet_site_title",
            Self::Issue => "issue",
            Self::JournalName => "journal_name",
            Self::Medium => "medium",
            Self::Month => "month",
            Self::MonthAccessed => "month_accessed",
            Self::NumberVolumes => "number_volumes",
            Self::Pages => "pages",
            Self::PatentNumber => "patent_number",
            Self::PeriodicalTitle => "periodical_title",
            Self::ProductionCompany => "production_company",
            Self::PublicationTitle => "publication_title",
            Self::Publisher => "publisher",
            Self::RecordingNumber => "recording_number",
            Self::ReferenceOrder => "reference_order",
            Self::Reporter => "reporter",
            Self::ShortTitle => "short_title",
            Self::StandardNumber => "standard_number",
            Self::StateProvince => "state_province",
            Self::Station => "station",
            Self::Theater => "theater",
            Self::ThesisType => "thesis_type",
            Self::Title => "title",
            Self::PatentType => "patent_type",
            Self::Url => "url",
            Self::Version => "version",
            Self::Volume => "volume",
            Self::Year => "year",
            Self::YearAccessed => "year_accessed",
        }
    }

    /// The value with this stable name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.name() == name)
    }
}

impl fmt::Display for BibliographySourceField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for BibliographySourceField {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::from_name(name)
            .ok_or_else(|| bibliography_error(format!("unknown source field {name:?}")))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn assert_names_round_trip<T: Copy + Eq + std::hash::Hash + fmt::Debug>(
        all: &[T],
        name: fn(T) -> &'static str,
        from_name: fn(&str) -> Option<T>,
    ) {
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len());
        assert_eq!(
            all.iter()
                .map(|value| name(*value))
                .collect::<HashSet<_>>()
                .len(),
            all.len()
        );
        for value in all {
            assert_eq!(from_name(name(*value)), Some(*value));
        }
    }

    #[test]
    fn every_value_has_a_distinct_name_that_reads_back() {
        assert_names_round_trip(
            &BibliographyStyle::ALL,
            BibliographyStyle::name,
            BibliographyStyle::from_name,
        );
        assert_names_round_trip(
            &BibliographySourceKind::ALL,
            BibliographySourceKind::name,
            BibliographySourceKind::from_name,
        );
        assert_names_round_trip(
            &BibliographyContributorRole::ALL,
            BibliographyContributorRole::name,
            BibliographyContributorRole::from_name,
        );
        assert_names_round_trip(
            &BibliographySourceField::ALL,
            BibliographySourceField::name,
            BibliographySourceField::from_name,
        );
    }

    #[test]
    fn names_are_snake_case_and_xml_names_are_not_accepted() {
        assert_eq!(
            BibliographySourceField::ReferenceOrder.name(),
            "reference_order"
        );
        assert_eq!(BibliographySourceField::PatentType.name(), "patent_type");
        assert_eq!(BibliographySourceField::Url.name(), "url");
        assert_eq!(
            BibliographySourceKind::ArticleInAPeriodical.name(),
            "article_in_a_periodical"
        );
        assert_eq!(
            BibliographyStyle::Iso690Numeric.to_string(),
            "iso690_numeric"
        );
        assert_eq!(
            "harvard_anglia".parse::<BibliographyStyle>().unwrap(),
            BibliographyStyle::HarvardAnglia
        );
        assert_eq!(BibliographySourceField::from_name("RefOrder"), None);
        assert_eq!(BibliographySourceKind::from_name("JournalArticle"), None);
        let error = "harvard-anglia".parse::<BibliographyStyle>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "bibliography: unknown bibliography style \"harvard-anglia\""
        );
    }
}
