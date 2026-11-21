//! Bibliography source identity, ordered XML and native citation formatting.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

use oxoffice_opc::OpcPackage;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::{Document, Error, Result};

mod apa_language;
mod citation;
mod citation_grammar;
mod locale;
mod paragraph_properties;
mod render;
mod render_rtl;
mod source;
mod update;

use apa_language::*;
use citation::*;
use citation_grammar::*;
use locale::*;
pub(crate) use paragraph_properties::*;
use render::*;
use render_rtl::*;
pub(crate) use source::*;
pub(crate) use update::*;

const BIB_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/bibliography";
const CUSTOM_XML_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const CUSTOM_XML_PROPS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CUSTOM_XML_PROPS_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";

/// The seventeen standard bibliography source classifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BibliographySourceKind {
    ArticleInAPeriodical,
    Book,
    BookSection,
    JournalArticle,
    ConferenceProceedings,
    Report,
    SoundRecording,
    Performance,
    Art,
    DocumentFromInternetSite,
    InternetSite,
    Film,
    Interview,
    Patent,
    ElectronicSource,
    Case,
    Misc,
}

impl BibliographySourceKind {
    fn xml_name(self) -> &'static str {
        match self {
            Self::ArticleInAPeriodical => "ArticleInAPeriodical",
            Self::Book => "Book",
            Self::BookSection => "BookSection",
            Self::JournalArticle => "JournalArticle",
            Self::ConferenceProceedings => "ConferenceProceedings",
            Self::Report => "Report",
            Self::SoundRecording => "SoundRecording",
            Self::Performance => "Performance",
            Self::Art => "Art",
            Self::DocumentFromInternetSite => "DocumentFromInternetSite",
            Self::InternetSite => "InternetSite",
            Self::Film => "Film",
            Self::Interview => "Interview",
            Self::Patent => "Patent",
            Self::ElectronicSource => "ElectronicSource",
            Self::Case => "Case",
            Self::Misc => "Misc",
        }
    }

    fn from_xml_name(name: &str) -> Option<Self> {
        match name {
            "ArticleInAPeriodical" => Some(Self::ArticleInAPeriodical),
            "Book" => Some(Self::Book),
            "BookSection" => Some(Self::BookSection),
            "JournalArticle" => Some(Self::JournalArticle),
            "ConferenceProceedings" => Some(Self::ConferenceProceedings),
            "Report" => Some(Self::Report),
            "SoundRecording" => Some(Self::SoundRecording),
            "Performance" => Some(Self::Performance),
            "Art" => Some(Self::Art),
            "DocumentFromInternetSite" => Some(Self::DocumentFromInternetSite),
            "InternetSite" => Some(Self::InternetSite),
            "Film" => Some(Self::Film),
            "Interview" => Some(Self::Interview),
            "Patent" => Some(Self::Patent),
            "ElectronicSource" => Some(Self::ElectronicSource),
            "Case" => Some(Self::Case),
            "Misc" => Some(Self::Misc),
            _ => None,
        }
    }
}

/// A standard contributor role in the ordered author list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BibliographyContributorRole {
    Artist,
    Author,
    BookAuthor,
    Compiler,
    Composer,
    Conductor,
    Counsel,
    Director,
    Editor,
    Interviewee,
    Interviewer,
    Inventor,
    Performer,
    ProducerName,
    Translator,
    Writer,
}

impl BibliographyContributorRole {
    fn xml_name(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Author => "Author",
            Self::BookAuthor => "BookAuthor",
            Self::Compiler => "Compiler",
            Self::Composer => "Composer",
            Self::Conductor => "Conductor",
            Self::Counsel => "Counsel",
            Self::Director => "Director",
            Self::Editor => "Editor",
            Self::Interviewee => "Interviewee",
            Self::Interviewer => "Interviewer",
            Self::Inventor => "Inventor",
            Self::Performer => "Performer",
            Self::ProducerName => "ProducerName",
            Self::Translator => "Translator",
            Self::Writer => "Writer",
        }
    }

    fn from_xml_name(name: &str) -> Option<Self> {
        match name {
            "Artist" => Some(Self::Artist),
            "Author" => Some(Self::Author),
            "BookAuthor" => Some(Self::BookAuthor),
            "Compiler" => Some(Self::Compiler),
            "Composer" => Some(Self::Composer),
            "Conductor" => Some(Self::Conductor),
            "Counsel" => Some(Self::Counsel),
            "Director" => Some(Self::Director),
            "Editor" => Some(Self::Editor),
            "Interviewee" => Some(Self::Interviewee),
            "Interviewer" => Some(Self::Interviewer),
            "Inventor" => Some(Self::Inventor),
            "Performer" => Some(Self::Performer),
            "ProducerName" => Some(Self::ProducerName),
            "Translator" => Some(Self::Translator),
            "Writer" => Some(Self::Writer),
            _ => None,
        }
    }
}

/// A standard textual source property, including repeated occurrences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BibliographySourceField {
    AbbreviatedCaseNumber,
    AlbumTitle,
    BookTitle,
    Broadcaster,
    BroadcastTitle,
    CaseNumber,
    ChapterNumber,
    City,
    Comments,
    ConferenceName,
    CountryRegion,
    Court,
    Day,
    DayAccessed,
    Department,
    Distributor,
    Edition,
    Institution,
    InternetSiteTitle,
    Issue,
    JournalName,
    Medium,
    Month,
    MonthAccessed,
    NumberVolumes,
    Pages,
    PatentNumber,
    PeriodicalTitle,
    ProductionCompany,
    PublicationTitle,
    Publisher,
    RecordingNumber,
    ReferenceOrder,
    Reporter,
    ShortTitle,
    StandardNumber,
    StateProvince,
    Station,
    Theater,
    ThesisType,
    Title,
    PatentType,
    Url,
    Version,
    Volume,
    Year,
    YearAccessed,
}

impl BibliographySourceField {
    fn xml_name(self) -> &'static str {
        match self {
            Self::AbbreviatedCaseNumber => "AbbreviatedCaseNumber",
            Self::AlbumTitle => "AlbumTitle",
            Self::BookTitle => "BookTitle",
            Self::Broadcaster => "Broadcaster",
            Self::BroadcastTitle => "BroadcastTitle",
            Self::CaseNumber => "CaseNumber",
            Self::ChapterNumber => "ChapterNumber",
            Self::City => "City",
            Self::Comments => "Comments",
            Self::ConferenceName => "ConferenceName",
            Self::CountryRegion => "CountryRegion",
            Self::Court => "Court",
            Self::Day => "Day",
            Self::DayAccessed => "DayAccessed",
            Self::Department => "Department",
            Self::Distributor => "Distributor",
            Self::Edition => "Edition",
            Self::Institution => "Institution",
            Self::InternetSiteTitle => "InternetSiteTitle",
            Self::Issue => "Issue",
            Self::JournalName => "JournalName",
            Self::Medium => "Medium",
            Self::Month => "Month",
            Self::MonthAccessed => "MonthAccessed",
            Self::NumberVolumes => "NumberVolumes",
            Self::Pages => "Pages",
            Self::PatentNumber => "PatentNumber",
            Self::PeriodicalTitle => "PeriodicalTitle",
            Self::ProductionCompany => "ProductionCompany",
            Self::PublicationTitle => "PublicationTitle",
            Self::Publisher => "Publisher",
            Self::RecordingNumber => "RecordingNumber",
            Self::ReferenceOrder => "RefOrder",
            Self::Reporter => "Reporter",
            Self::ShortTitle => "ShortTitle",
            Self::StandardNumber => "StandardNumber",
            Self::StateProvince => "StateProvince",
            Self::Station => "Station",
            Self::Theater => "Theater",
            Self::ThesisType => "ThesisType",
            Self::Title => "Title",
            Self::PatentType => "Type",
            Self::Url => "URL",
            Self::Version => "Version",
            Self::Volume => "Volume",
            Self::Year => "Year",
            Self::YearAccessed => "YearAccessed",
        }
    }

    fn from_xml_name(name: &str) -> Option<Self> {
        match name {
            "AbbreviatedCaseNumber" => Some(Self::AbbreviatedCaseNumber),
            "AlbumTitle" => Some(Self::AlbumTitle),
            "BookTitle" => Some(Self::BookTitle),
            "Broadcaster" => Some(Self::Broadcaster),
            "BroadcastTitle" => Some(Self::BroadcastTitle),
            "CaseNumber" => Some(Self::CaseNumber),
            "ChapterNumber" => Some(Self::ChapterNumber),
            "City" => Some(Self::City),
            "Comments" => Some(Self::Comments),
            "ConferenceName" => Some(Self::ConferenceName),
            "CountryRegion" => Some(Self::CountryRegion),
            "Court" => Some(Self::Court),
            "Day" => Some(Self::Day),
            "DayAccessed" => Some(Self::DayAccessed),
            "Department" => Some(Self::Department),
            "Distributor" => Some(Self::Distributor),
            "Edition" => Some(Self::Edition),
            "Institution" => Some(Self::Institution),
            "InternetSiteTitle" => Some(Self::InternetSiteTitle),
            "Issue" => Some(Self::Issue),
            "JournalName" => Some(Self::JournalName),
            "Medium" => Some(Self::Medium),
            "Month" => Some(Self::Month),
            "MonthAccessed" => Some(Self::MonthAccessed),
            "NumberVolumes" => Some(Self::NumberVolumes),
            "Pages" => Some(Self::Pages),
            "PatentNumber" => Some(Self::PatentNumber),
            "PeriodicalTitle" => Some(Self::PeriodicalTitle),
            "ProductionCompany" => Some(Self::ProductionCompany),
            "PublicationTitle" => Some(Self::PublicationTitle),
            "Publisher" => Some(Self::Publisher),
            "RecordingNumber" => Some(Self::RecordingNumber),
            "RefOrder" => Some(Self::ReferenceOrder),
            "Reporter" => Some(Self::Reporter),
            "ShortTitle" => Some(Self::ShortTitle),
            "StandardNumber" => Some(Self::StandardNumber),
            "StateProvince" => Some(Self::StateProvince),
            "Station" => Some(Self::Station),
            "Theater" => Some(Self::Theater),
            "ThesisType" => Some(Self::ThesisType),
            "Title" => Some(Self::Title),
            "Type" => Some(Self::PatentType),
            "URL" => Some(Self::Url),
            "Version" => Some(Self::Version),
            "Volume" => Some(Self::Volume),
            "Year" => Some(Self::Year),
            "YearAccessed" => Some(Self::YearAccessed),
            _ => None,
        }
    }
}

/// A pinned installed Word bibliography style edition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BibliographyStyle {
    ApaSixthEdition,
    Chicago,
    Gb7714,
    GostName,
    GostTitle,
    HarvardAnglia,
    Ieee,
    Iso690AuthorDate,
    Iso690Numeric,
    MlaSeventhEdition,
    Sist02,
    Turabian,
}

/// Ordered components of a person's bibliography name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyPerson {
    pub first: Vec<String>,
    pub middle: Vec<String>,
    pub last: Vec<String>,
}

/// Personal or corporate authorship. Corporate is legal only for Author and Performer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibliographyAuthor {
    People(Vec<BibliographyPerson>),
    Corporate(String),
}

/// One contributor occurrence in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyContributor {
    pub role: BibliographyContributorRole,
    pub value: BibliographyAuthor,
}

/// One textual property occurrence in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyProperty {
    pub field: BibliographySourceField,
    pub value: String,
}

/// Editable standard source data. Unmodelled imported XML remains package-owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographySource {
    pub tag: String,
    pub guid: String,
    pub kind: BibliographySourceKind,
    /// Numeric source locale. Zero contributes no formatting override.
    /// Imported lexical spellings remain unchanged by inspection or a no-op replacement.
    pub locale: Option<u32>,
    pub contributors: Vec<BibliographyContributor>,
    pub properties: Vec<BibliographyProperty>,
}

/// Per-source citation switches, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationSourceOptions {
    pub tag: String,
    pub pages: Option<String>,
    pub volume: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub suppress_author: bool,
    pub suppress_year: bool,
    pub suppress_title: bool,
}

/// Citation members and the field-wide formatting locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationOptions {
    pub sources: Vec<CitationSourceOptions>,
    /// Field-wide formatting locale. Sources with an explicit nonzero locale override it.
    /// An omitted or zero locale needs caller-provided application context when no source overrides it.
    pub locale: Option<u32>,
}

/// Formatting style, formatting locale and independent source selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyOptions {
    pub style: BibliographyStyle,
    pub locale: Option<u32>,
    pub locale_filter: Option<u32>,
    pub tags: Vec<String>,
}

/// Preserved collection metadata and its recognized style, when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyStyleInfo {
    pub style_key: Option<String>,
    pub style_path: Option<String>,
    /// Standard Sources style metadata has no formatting locale, so this is absent.
    /// Formatting locale selectors belong to individual field instructions.
    pub locale: Option<u32>,
    pub supported_style: Option<BibliographyStyle>,
}

/// Source identity and the editable supported projection, when unambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographySourceInfo {
    pub tag: String,
    pub guid: Option<String>,
    pub source_type: Option<String>,
    /// Editable data when identity and modeled members have an unambiguous projection.
    pub supported: Option<BibliographySource>,
    /// Inspection limitations, including unknown or ambiguous imported locale identities.
    /// Absence of a numeric locale alone does not distinguish omitted data from a projection limit.
    pub diagnostics: Vec<String>,
}

/// Explicit update results. Unsupported producers retain their complete caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyUpdateReport {
    pub updated_citations: usize,
    pub rebuilt_bibliographies: usize,
    pub diagnostics: Vec<String>,
}

// Exact ranges retain repeated members, unknown attributes and producer prefixes.
#[derive(Debug)]
struct SourceXmlNode {
    namespace: String,
    local: String,
    name: String,
    full: Range<usize>,
    content: Range<usize>,
    attributes: Vec<(String, String, String)>,
    children: Vec<usize>,
    text: String,
    opaque_scalar: bool,
    empty: bool,
}

#[derive(Debug)]
struct SourceXml {
    nodes: Vec<SourceXmlNode>,
}

fn bibliography_error(message: impl Into<String>) -> Error {
    Error::Other(format!("bibliography: {}", message.into()))
}

fn namespace_value(value: ResolveResult<'_>) -> Result<String> {
    match value {
        ResolveResult::Bound(value) => String::from_utf8(value.as_ref().to_vec())
            .map_err(|_| bibliography_error("namespace is not UTF-8")),
        ResolveResult::Unbound => Ok(String::new()),
        ResolveResult::Unknown(prefix) => Err(bibliography_error(format!(
            "unbound namespace prefix {}",
            String::from_utf8_lossy(&prefix)
        ))),
    }
}

struct BibliographyCollection {
    part: String,
    xml: Vec<u8>,
    parsed: SourceXml,
    sources: Vec<(usize, BibliographySourceInfo)>,
}

impl Document {
    /// Inspect the qualified source collection without changing producer XML.
    pub fn bibliography_sources(&self) -> Result<Vec<BibliographySourceInfo>> {
        Ok(bibliography_collection(&self.package, &self.doc_part_name)?
            .map(|collection| {
                collection
                    .sources
                    .into_iter()
                    .map(|(_, source)| source)
                    .collect()
            })
            .unwrap_or_default())
    }
}

impl Document {
    /// Append one validated source while retaining all existing producer records.
    pub fn add_bibliography_source(&mut self, source: &BibliographySource) -> Result<()> {
        validate_source(source)?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        if bibliography_collection(&candidate.package, &candidate.doc_part_name)?.is_none() {
            create_collection(&mut candidate)?;
        }
        let collection = bibliography_collection(&candidate.package, &candidate.doc_part_name)?
            .ok_or_else(|| bibliography_error("source collection creation failed"))?;
        unique_collection_identities(&collection)?;
        if collection.sources.iter().any(|(_, existing)| {
            existing.tag == source.tag
                || existing.guid.as_ref().is_some_and(|guid| {
                    crate::content_control::normalize_store_item_id(guid)
                        == crate::content_control::normalize_store_item_id(&source.guid)
                })
        }) {
            return Err(bibliography_error("source identity already exists"));
        }
        let edit = append_to_source_node(
            &collection.xml,
            &collection.parsed.nodes[0],
            authored_source_xml(source).as_bytes(),
        )?;
        candidate.package.set_part(
            &collection.part,
            apply_source_edits(&collection.xml, vec![edit])?,
        );
        publish_source_collection(self, candidate)
    }

    /// Replace modeled members of one source, preserving its GUID and raw metadata.
    pub fn replace_bibliography_source(&mut self, source: &BibliographySource) -> Result<()> {
        validate_source(source)?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let collection = bibliography_collection(&candidate.package, &candidate.doc_part_name)?
            .ok_or_else(|| bibliography_error("source collection does not exist"))?;
        unique_collection_identities(&collection)?;
        let (index, existing) = collection
            .sources
            .iter()
            .find(|(_, existing)| existing.tag == source.tag)
            .ok_or_else(|| bibliography_error("source tag does not exist"))?;
        if existing.guid.as_deref() != Some(source.guid.as_str()) {
            return Err(bibliography_error("source GUID cannot change"));
        }
        let xml = replace_source_xml(&collection, *index, source)?;
        if xml == collection.xml {
            return Ok(());
        }
        candidate.package.set_part(&collection.part, xml);
        publish_source_collection(self, candidate)
    }

    /// Remove an unreferenced source without deleting its collection or properties.
    pub fn remove_bibliography_source(&mut self, tag: &str) -> Result<()> {
        if tag.trim().is_empty() {
            return Err(bibliography_error("empty source tag"));
        }
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let collection = bibliography_collection(&candidate.package, &candidate.doc_part_name)?
            .ok_or_else(|| bibliography_error("source collection does not exist"))?;
        unique_collection_identities(&collection)?;
        let (index, _) = collection
            .sources
            .iter()
            .find(|(_, source)| source.tag == tag)
            .ok_or_else(|| bibliography_error("source tag does not exist"))?;
        for instruction in crate::field::bibliography_reference_instructions(&candidate)? {
            if !instruction.quotes_are_balanced() {
                return Err(bibliography_error("ambiguous citation reference"));
            }
            let mut tags = Vec::new();
            for argument in &instruction.arguments {
                let oxoffice_docx_oxml::text::FieldArgument::Text(value) = argument else {
                    return Err(bibliography_error("nested citation reference"));
                };
                tags.push(value.as_str());
            }
            for switch in instruction
                .switches
                .iter()
                .filter(|switch| switch.name == "m")
            {
                let Some(oxoffice_docx_oxml::text::FieldArgument::Text(value)) = &switch.argument
                else {
                    return Err(bibliography_error("ambiguous citation member"));
                };
                tags.push(value.as_str());
            }
            if tags.is_empty() {
                return Err(bibliography_error("citation has no source identity"));
            }
            if tags.contains(&tag) {
                return Err(bibliography_error("source is referenced by a citation"));
            }
        }
        candidate.package.set_part(
            &collection.part,
            apply_source_edits(
                &collection.xml,
                vec![(collection.parsed.nodes[*index].full.clone(), Vec::new())],
            )?,
        );
        publish_source_collection(self, candidate)
    }
}

impl BibliographyStyle {
    fn metadata(self) -> (&'static str, &'static str) {
        match self {
            Self::ApaSixthEdition => ("APASixthEditionOfficeOnline.xsl", "APA"),
            Self::Chicago => ("CHICAGO.XSL", "Chicago"),
            Self::Gb7714 => ("GB.XSL", "GB7714"),
            Self::GostName => ("GostName.XSL", "GOST - Name Sort"),
            Self::GostTitle => ("GostTitle.XSL", "GOST - Title Sort"),
            Self::HarvardAnglia => ("HarvardAnglia2008OfficeOnline.xsl", "Harvard - Anglia"),
            Self::Ieee => ("IEEE2006OfficeOnline.xsl", "IEEE"),
            Self::Iso690AuthorDate => ("ISO690.XSL", "ISO 690 - First Element and Date"),
            Self::Iso690Numeric => ("ISO690Nmerical.XSL", "ISO 690 - Numerical Reference"),
            Self::MlaSeventhEdition => ("MLASeventhEditionOfficeOnline.xsl", "MLA"),
            Self::Sist02 => ("SIST02.XSL", "SIST02"),
            Self::Turabian => ("TURABIAN.XSL", "Turabian"),
        }
    }

    fn from_metadata(path: Option<&str>, key: Option<&str>) -> Option<Self> {
        let styles = [
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
        styles.into_iter().find(|style| {
            let (filename, name) = style.metadata();
            path.is_none_or(|path| {
                path.trim_start_matches(['\\', '/'])
                    .eq_ignore_ascii_case(filename)
            }) && key.is_none_or(|key| key == name)
        })
    }
}

fn style_info(collection: &BibliographyCollection) -> Option<BibliographyStyleInfo> {
    let attributes = &collection.parsed.nodes[0].attributes;
    let get = |name| {
        attributes
            .iter()
            .find(|(namespace, local, _)| namespace.is_empty() && local == name)
            .map(|(_, _, value)| value.clone())
    };
    let style_path = get("SelectedStyle");
    let style_key = get("StyleName");
    if style_path.is_none() && style_key.is_none() {
        return None;
    }
    let supported_style =
        BibliographyStyle::from_metadata(style_path.as_deref(), style_key.as_deref());
    Some(BibliographyStyleInfo {
        style_key,
        style_path,
        locale: None,
        supported_style,
    })
}

fn attach_style_metadata(candidate: &mut Document, style: BibliographyStyle) -> Result<()> {
    if bibliography_collection(&candidate.package, &candidate.doc_part_name)?.is_none() {
        create_collection(candidate)?;
    }
    let collection = bibliography_collection(&candidate.package, &candidate.doc_part_name)?
        .ok_or_else(|| bibliography_error("source collection creation failed"))?;
    unique_collection_identities(&collection)?;
    let root = &collection.parsed.nodes[0];
    let start_end = if root.empty {
        root.full.end
    } else {
        root.content.start
    };
    let start = &collection.xml[root.full.start..start_end];
    let (filename, key) = style.metadata();
    let path = format!("\\{filename}");
    let mut edits = Vec::new();
    let mut appended = String::new();
    for (name, value) in [("SelectedStyle", path.as_str()), ("StyleName", key)] {
        if let Some((_, _, previous)) = root
            .attributes
            .iter()
            .find(|(namespace, local, _)| namespace.is_empty() && local == name)
        {
            if previous == value {
                continue;
            }
            let (begin, end) = crate::field::attribute_value_span(start, name.as_bytes())
                .ok_or_else(|| bibliography_error("style attribute source range disappeared"))?;
            edits.push((
                root.full.start + begin..root.full.start + end,
                quick_xml::escape::escape(value).as_bytes().to_vec(),
            ));
        } else {
            appended.push_str(&format!(" {name}=\"{}\"", quick_xml::escape::escape(value)));
        }
    }
    if !appended.is_empty() {
        let insertion = start_end - if root.empty { 2 } else { 1 };
        edits.push((insertion..insertion, appended.into_bytes()));
    }
    if !edits.is_empty() {
        candidate.package.set_part(
            &collection.part,
            apply_source_edits(&collection.xml, edits)?,
        );
    }
    Ok(())
}

fn option_switch(name: &str, value: Option<String>) -> oxoffice_docx_oxml::text::FieldSwitch {
    oxoffice_docx_oxml::text::FieldSwitch {
        name: name.to_owned(),
        argument: value.map(oxoffice_docx_oxml::text::FieldArgument::Text),
    }
}

fn bibliography_option_switches(
    options: &BibliographyOptions,
) -> Result<Vec<oxoffice_docx_oxml::text::FieldSwitch>> {
    for locale in [options.locale, options.locale_filter]
        .into_iter()
        .flatten()
    {
        validate_locale_number(locale)?;
    }
    let mut switches = Vec::new();
    if let Some(locale) = options.locale {
        switches.push(option_switch("l", Some(locale.to_string())));
    }
    if let Some(locale) = options.locale_filter {
        switches.push(option_switch("f", Some(locale.to_string())));
    }
    for tag in &options.tags {
        if tag.trim().is_empty() {
            return Err(bibliography_error("empty selected source tag"));
        }
        switches.push(option_switch("m", Some(tag.clone())));
    }
    oxoffice_docx_oxml::text::FieldInstruction::new("BIBLIOGRAPHY", Vec::new(), switches.clone())?;
    Ok(switches)
}

impl Document {
    /// Inspect saved standard style metadata without inventing a root locale.
    pub fn bibliography_style(&self) -> Result<Option<BibliographyStyleInfo>> {
        Ok(bibliography_collection(&self.package, &self.doc_part_name)?
            .as_ref()
            .and_then(style_info))
    }

    /// Atomically set document style and existing bibliography selectors.
    /// Citation instructions and every stored result cache retain their bytes.
    pub fn set_bibliography_options(&mut self, options: &BibliographyOptions) -> Result<()> {
        let switches = bibliography_option_switches(options)?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let before = candidate.package.clone();
        attach_style_metadata(&mut candidate, options.style)?;
        crate::field::patch_bibliography_instruction_options(&mut candidate, &switches)?;
        if candidate.package.parts == before.parts
            && candidate.package.part_rels.len() == before.part_rels.len()
            && candidate
                .package
                .part_rels
                .iter()
                .all(|(part, relationships)| {
                    before
                        .part_rels
                        .get(part)
                        .is_some_and(|previous| previous.items == relationships.items)
                })
            && candidate.package.content_types == before.content_types
        {
            return Ok(());
        }
        publish_source_collection(self, candidate)
    }

    /// Insert a complex citation at one checked paragraph run boundary.
    pub fn insert_citation(
        &mut self,
        position: &crate::StoryRunPosition,
        options: &CitationOptions,
    ) -> Result<()> {
        if let Some(locale) = options.locale {
            validate_locale_number(locale)?;
        }
        let Some(first) = options.sources.first() else {
            return Err(bibliography_error("citation requires at least one source"));
        };
        let mut switches = Vec::new();
        if let Some(locale) = options.locale {
            switches.push(option_switch("l", Some(locale.to_string())));
        }
        for (index, source) in options.sources.iter().enumerate() {
            if source.tag.trim().is_empty() {
                return Err(bibliography_error("empty citation source tag"));
            }
            if index != 0 {
                switches.push(option_switch("m", Some(source.tag.clone())));
            }
            for (name, value) in [
                ("p", &source.pages),
                ("v", &source.volume),
                ("f", &source.prefix),
                ("s", &source.suffix),
            ] {
                if let Some(value) = value {
                    switches.push(option_switch(name, Some(value.clone())));
                }
            }
            for (name, enabled) in [
                ("n", source.suppress_author),
                ("y", source.suppress_year),
                ("t", source.suppress_title),
            ] {
                if enabled {
                    switches.push(option_switch(name, None));
                }
            }
        }
        let instruction = oxoffice_docx_oxml::text::FieldInstruction::new(
            "CITATION",
            vec![oxoffice_docx_oxml::text::FieldArgument::Text(
                first.tag.clone(),
            )],
            switches,
        )?;
        self.insert_checked_story_field(position, instruction)
    }

    /// Insert one bibliography owner with its own selectors and document style.
    /// Existing bibliography instructions and caches remain unchanged.
    pub fn insert_bibliography(
        &mut self,
        position: &crate::ContentLocation,
        options: &BibliographyOptions,
    ) -> Result<()> {
        let switches = bibliography_option_switches(options)?;
        let instruction =
            oxoffice_docx_oxml::text::FieldInstruction::new("BIBLIOGRAPHY", Vec::new(), switches)?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        attach_style_metadata(&mut candidate, options.style)?;
        let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
        let mut run = oxoffice_docx_oxml::text::CT_R::new("");
        run.content = vec![oxoffice_docx_oxml::text::RunContent::Field(
            oxoffice_docx_oxml::text::Field::from_instruction(
                instruction,
                oxoffice_docx_oxml::text::FieldForm::Complex,
                Vec::new(),
            )?,
        )];
        paragraph.runs.push(run);
        candidate.insert_content(position, crate::ContentFragment::paragraph(paragraph)?)?;
        candidate.prepare_staged_package()?;
        publish_source_collection(self, candidate)
    }
}

// Shared source-driven APA vocabulary. These fields are consumed by authored
// entries and by the existing secondary-contributor/retrieval callers.
#[derive(Clone, Copy)]
struct ApaBibliographyLanguage {
    initial_mark: char,
    rtl_cs_font: Option<&'static str>,
    day_first: bool,
    publication_month_first: bool,
    calendar_day_year: &'static str,
    author_pair: bool,
    missing_year: Option<&'static str>,
    contributor_separator: &'static str,
    conjunction: &'static str,
    date_prefix: &'static str,
    retrieval_date_after: bool,
    retrieval_after_prefix: &'static str,
    day_month: &'static str,
    retrieval_date_separator: &'static str,
    in_before_separator: &'static str,
    on_before_separator: &'static str,
    recorded_before_separator: &'static str,
    month_day: &'static str,

    month_year: &'static str,
    year_month: &'static str,
    date_suffix: &'static str,
    calendar_year_first: bool,
    year_day_first: bool,
    volume_before: bool,
    volume_separator: &'static str,
    volume_suffix: &'static str,
    volume: &'static str,
    pages: &'static str,
    language: Option<&'static str>,
    language_east_asia: Option<&'static str>,
    joined_names: bool,
    semantic_rtl: bool,
    numeric_cs_font: Option<&'static str>,
    literal_font: Option<&'static str>,
    literal_script: Option<(char, char)>,
    no_italic: bool,
    full_names: bool,
    rtl_labels: bool,
    location_separator: &'static str,
    language_bidi: Option<&'static str>,
    patent: &'static str,
    patent_suffix: &'static str,
    compiler: &'static str,
    interviewer: &'static str,
    in_label: &'static str,
    in_after: bool,
    in_separator: &'static str,
    producer: &'static str,
    writer: &'static str,
    director: &'static str,
    performer: &'static str,
    site_editor: &'static str,
    motion_picture: &'static str,
    recorded_by: &'static str,
    recorded_after: bool,
    recorded_separator: &'static str,
    recorded_prefix: &'static str,
    on_label: &'static str,
    on_after: bool,
    on_separator: &'static str,
    quote_album: bool,
    curly_titles: bool,
    editor: &'static str,
    editors: &'static str,
    translator: &'static str,
    translators: &'static str,
    edition: &'static str,
    edition_separator: &'static str,
    edition_before: bool,
    edition_suffix: &'static str,
    retrieved: &'static str,
    retrieved_without_date: Option<&'static str>,
    from_without_date: &'static str,
    from_with_date: &'static str,
    retrieval_suffix: &'static str,
}

fn ensure_bibliography_style(document: &mut Document) -> Result<()> {
    let style_relationship =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    let style_content_type =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
    let edges = document
        .package
        .get_part_rels(&document.doc_part_name)
        .into_iter()
        .flat_map(|relationships| &relationships.items)
        .filter(|edge| edge.rel_type == style_relationship)
        .collect::<Vec<_>>();
    if edges.len() > 1 {
        return Err(bibliography_error("ambiguous bibliography style part"));
    }
    let existing = edges
        .first()
        .map(|edge| internal_target(&document.package, &document.doc_part_name, edge))
        .transpose()?;
    let part = document.reserve_document_part_bundle(
        existing.as_deref(),
        "/word/styles.xml",
        style_relationship,
        style_content_type,
    )?;
    let mut xml = document.package.get_part(&part).map(<[u8]>::to_vec)
        .unwrap_or_else(|| b"<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"></w:styles>".to_vec());
    let parsed = SourceXml::parse(&xml)?;
    let root = &parsed.nodes[0];
    if root.namespace != "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        || root.local != "styles"
    {
        return Err(bibliography_error(
            "bibliography styles root has wrong namespace",
        ));
    }
    let existing_definitions = root
        .children
        .iter()
        .filter(|&&index| {
            let node = &parsed.nodes[index];
            node.namespace == root.namespace
                && node.local == "style"
                && node.attributes.iter().any(|(namespace, local, value)| {
                    namespace == &root.namespace && local == "styleId" && value == "Bibliography"
                })
        })
        .count();
    if existing_definitions > 1 {
        return Err(bibliography_error("ambiguous Bibliography style identity"));
    }
    if existing_definitions == 1 {
        return Ok(());
    }
    if document.styles.get_by_id("Bibliography").is_some() {
        return Err(bibliography_error(
            "unsupported namespace binding in projected Bibliography style",
        ));
    }
    let style = b"<w:style xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" w:type=\"paragraph\" w:styleId=\"Bibliography\"><w:name w:val=\"Bibliography\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"37\"/><w:unhideWhenUsed/></w:style>";
    if root.empty {
        let start = &xml[root.full.start..root.full.end];
        let close = start
            .windows(2)
            .rposition(|bytes| bytes == b"/>")
            .ok_or_else(|| bibliography_error("empty bibliography styles root changed"))?;
        let mut replacement = start[..close].to_vec();
        replacement.push(b'>');
        replacement.extend_from_slice(style);
        replacement.extend_from_slice(format!("</{}>", root.name).as_bytes());
        xml.splice(root.full.clone(), replacement);
    } else {
        xml.splice(root.content.end..root.content.end, style.iter().copied());
    }
    SourceXml::parse(&xml)?;
    document.package.set_part(&part, xml);
    Ok(())
}

impl Document {
    /// Atomically update citation and bibliography caches from saved source and field context.
    /// Imported source XML, including LCID spelling and RefOrder occurrences, retains its bytes.
    /// A field with no effective locale requires [`Self::update_bibliography_with_default_locale`].
    pub fn update_bibliography(&mut self) -> Result<BibliographyUpdateReport> {
        self.update_bibliography_transaction(None)
    }

    /// Atomically update caches with the caller's actual application default locale.
    /// This context applies only when source and field selectors do not specify a nonzero locale.
    /// It does not persist a document default or rewrite source metadata.
    pub fn update_bibliography_with_default_locale(
        &mut self,
        locale: u32,
    ) -> Result<BibliographyUpdateReport> {
        self.update_bibliography_transaction(Some(locale))
    }

    fn update_bibliography_transaction(
        &mut self,
        locale: Option<u32>,
    ) -> Result<BibliographyUpdateReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let state = BibliographyUpdateState::new(&candidate, locale)?;
        let report = crate::field::update_bibliography_caches(&mut candidate, &state)?;
        if report.updated_citations == 0 && report.rebuilt_bibliographies == 0 {
            return Ok(report);
        }
        if report.rebuilt_bibliographies > 0 {
            ensure_bibliography_style(&mut candidate)?;
        }
        publish_source_collection(self, candidate)?;
        Ok(report)
    }
}
