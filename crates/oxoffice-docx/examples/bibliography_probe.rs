//! Writes one bibliography probe per Word style: every source kind with the
//! fields Word's source dialog shows, author-count and missing-field
//! variants of a book, the citation switches, and one bibliography. Every
//! field asks for en-US (`\l 1033`). Word updates and saves each probe; the
//! saved files are the oracle of `bibliography::oracle_tests`.
//!
//! cargo run -p oxoffice-docx --example bibliography_probe -- <output dir>

use oxoffice_docx::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceKind, BibliographyStyle, CitationOptions, CitationSourceOptions, Document,
    StoryRunPosition,
};

use BibliographyContributorRole as R;
use BibliographySourceField as F;
use BibliographySourceKind as K;

const EN_US: u32 = 1033;

/// The probe file name of each style.
pub const STYLES: [(BibliographyStyle, &str); 12] = [
    (BibliographyStyle::ApaSixthEdition, "apa6"),
    (BibliographyStyle::Chicago, "chicago"),
    (BibliographyStyle::Gb7714, "gb7714"),
    (BibliographyStyle::GostName, "gost_name"),
    (BibliographyStyle::GostTitle, "gost_title"),
    (BibliographyStyle::HarvardAnglia, "harvard_anglia"),
    (BibliographyStyle::Ieee, "ieee"),
    (BibliographyStyle::Iso690AuthorDate, "iso690_author_date"),
    (BibliographyStyle::Iso690Numeric, "iso690_numeric"),
    (BibliographyStyle::MlaSeventhEdition, "mla7"),
    (BibliographyStyle::Sist02, "sist02"),
    (BibliographyStyle::Turabian, "turabian"),
];

fn person(first: &str, middle: &str, last: &str) -> BibliographyPerson {
    let part = |value: &str| {
        if value.is_empty() {
            Vec::new()
        } else {
            vec![value.to_owned()]
        }
    };
    BibliographyPerson {
        first: part(first),
        middle: part(middle),
        last: part(last),
    }
}

fn people(role: R, names: &[(&str, &str, &str)]) -> BibliographyContributor {
    BibliographyContributor {
        role,
        value: BibliographyAuthor::People(
            names
                .iter()
                .map(|(first, middle, last)| person(first, middle, last))
                .collect(),
        ),
    }
}

fn corporate(role: R, name: &str) -> BibliographyContributor {
    BibliographyContributor {
        role,
        value: BibliographyAuthor::Corporate(name.to_owned()),
    }
}

struct Probe {
    tag: &'static str,
    kind: BibliographySourceKind,
    contributors: Vec<BibliographyContributor>,
    fields: Vec<(BibliographySourceField, &'static str)>,
}

/// Every source kind once, each by its own author so that no two sources
/// need telling apart, with the fields Word's dialog offers for the kind.
fn kind_probes() -> Vec<Probe> {
    vec![
        Probe {
            tag: "Book",
            kind: K::Book,
            contributors: vec![
                people(R::Author, &[("Alice", "B.", "Adams")]),
                people(R::Editor, &[("Edgar", "", "Ellis")]),
                people(R::Translator, &[("Tess", "", "Tran")]),
            ],
            fields: vec![
                (F::Title, "The shape of rivers"),
                (F::Year, "2018"),
                (F::City, "London"),
                (F::StateProvince, "Greater London"),
                (F::CountryRegion, "United Kingdom"),
                (F::Publisher, "Harbour Press"),
                (F::Volume, "2"),
                (F::NumberVolumes, "3"),
                (F::Edition, "2nd"),
                (F::Pages, "320"),
                (F::ShortTitle, "Rivers"),
                (F::StandardNumber, "978-0-00-000000-1"),
            ],
        },
        Probe {
            tag: "BookSection",
            kind: K::BookSection,
            contributors: vec![
                people(R::Author, &[("Bruno", "", "Baker")]),
                people(R::BookAuthor, &[("Clara", "", "Cole")]),
                people(R::Editor, &[("Dmitri", "", "Dunn")]),
            ],
            fields: vec![
                (F::Title, "Tides and harbours"),
                (F::BookTitle, "Coastal engineering"),
                (F::Year, "2017"),
                (F::Pages, "45-67"),
                (F::City, "Boston"),
                (F::Publisher, "North Books"),
                (F::Volume, "1"),
                (F::Edition, "3rd"),
                (F::ShortTitle, "Tides"),
            ],
        },
        Probe {
            tag: "Journal",
            kind: K::JournalArticle,
            contributors: vec![people(R::Author, &[("Carla", "", "Carter")])],
            fields: vec![
                (F::Title, "Liver cells under stress"),
                (F::JournalName, "Journal of Hepatology Studies"),
                (F::Year, "2021"),
                (F::Month, "March"),
                (F::Day, "4"),
                (F::Pages, "101-115"),
                (F::Volume, "12"),
                (F::Issue, "3"),
                (F::ShortTitle, "Liver cells"),
            ],
        },
        Probe {
            tag: "Periodical",
            kind: K::ArticleInAPeriodical,
            contributors: vec![people(R::Author, &[("Daniel", "", "Davis")])],
            fields: vec![
                (F::Title, "The quiet city"),
                (F::PeriodicalTitle, "Weekly Review"),
                (F::Year, "2019"),
                (F::Month, "June"),
                (F::Day, "12"),
                (F::Pages, "22-25"),
                (F::Volume, "8"),
                (F::Issue, "24"),
                (F::ShortTitle, "Quiet city"),
            ],
        },
        Probe {
            tag: "Conference",
            kind: K::ConferenceProceedings,
            contributors: vec![
                people(R::Author, &[("Emma", "", "Evans")]),
                people(R::Editor, &[("Felix", "", "Ford")]),
            ],
            fields: vec![
                (F::Title, "Fast parsers"),
                (F::ConferenceName, "Symposium on Documents"),
                (F::Year, "2020"),
                (F::Pages, "7-14"),
                (F::City, "Lyon"),
                (F::Publisher, "DocSoc"),
                (F::Volume, "4"),
                (F::ShortTitle, "Parsers"),
            ],
        },
        Probe {
            tag: "Report",
            kind: K::Report,
            contributors: vec![people(R::Author, &[("Fiona", "", "Foster")])],
            fields: vec![
                (F::Title, "Water quality in 2015"),
                (F::Year, "2016"),
                (F::Publisher, "Rivers Agency"),
                (F::City, "Cardiff"),
                (F::ThesisType, "Technical report"),
                (F::StandardNumber, "TR-42"),
                (F::Department, "Hydrology"),
                (F::Institution, "Rivers Agency"),
                (F::ShortTitle, "Water quality"),
            ],
        },
        Probe {
            tag: "Sound",
            kind: K::SoundRecording,
            contributors: vec![
                people(R::Composer, &[("Gina", "", "Garcia")]),
                people(R::Performer, &[("Hugo", "", "Hale")]),
                people(R::Conductor, &[("Ivo", "", "Ince")]),
            ],
            fields: vec![
                (F::Title, "Night song"),
                (F::AlbumTitle, "Evenings"),
                (F::Year, "2014"),
                (F::ProductionCompany, "Blue Label"),
                (F::Medium, "CD"),
                (F::City, "Vienna"),
                (F::ShortTitle, "Night song"),
            ],
        },
        Probe {
            tag: "Performance",
            kind: K::Performance,
            contributors: vec![
                people(R::Writer, &[("Henry", "", "Hughes")]),
                people(R::Performer, &[("Jade", "", "Jordan")]),
                people(R::Director, &[("Karl", "", "Kent")]),
            ],
            fields: vec![
                (F::Title, "The long winter"),
                (F::Theater, "Royal Theatre"),
                (F::City, "Bath"),
                (F::Year, "2013"),
                (F::Month, "May"),
                (F::Day, "2"),
                (F::ShortTitle, "Long winter"),
            ],
        },
        Probe {
            tag: "Art",
            kind: K::Art,
            contributors: vec![people(R::Artist, &[("Iris", "", "Irving")])],
            fields: vec![
                (F::Title, "Red field"),
                (F::Year, "1998"),
                (F::Institution, "City Gallery"),
                (F::City, "Leeds"),
                (F::Medium, "Oil on canvas"),
                (F::ShortTitle, "Red field"),
            ],
        },
        Probe {
            tag: "WebDocument",
            kind: K::DocumentFromInternetSite,
            contributors: vec![people(R::Author, &[("James", "", "Jones")])],
            fields: vec![
                (F::Title, "How rivers move"),
                (F::InternetSiteTitle, "Science Notes"),
                (F::ProductionCompany, "Notes Ltd"),
                (F::Year, "2022"),
                (F::Month, "January"),
                (F::Day, "15"),
                (F::YearAccessed, "2023"),
                (F::MonthAccessed, "February"),
                (F::DayAccessed, "3"),
                (F::Url, "https://example.org/rivers"),
                (F::ShortTitle, "Rivers move"),
            ],
        },
        Probe {
            tag: "WebSite",
            kind: K::InternetSite,
            contributors: vec![people(R::Author, &[("Kara", "", "Klein")])],
            fields: vec![
                (F::Title, "Bird counts"),
                (F::InternetSiteTitle, "Field Diary"),
                (F::Year, "2021"),
                (F::Month, "April"),
                (F::Day, "9"),
                (F::YearAccessed, "2023"),
                (F::MonthAccessed, "March"),
                (F::DayAccessed, "1"),
                (F::Url, "https://example.org/birds"),
                (F::ShortTitle, "Bird counts"),
            ],
        },
        Probe {
            tag: "Film",
            kind: K::Film,
            contributors: vec![
                people(R::Director, &[("Leo", "", "Lopez")]),
                people(R::Performer, &[("Mia", "", "Marsh")]),
                people(R::ProducerName, &[("Ned", "", "North")]),
                people(R::Writer, &[("Ola", "", "Olsen")]),
            ],
            fields: vec![
                (F::Title, "Harbour lights"),
                (F::Year, "2010"),
                (F::Distributor, "Wide Films"),
                (F::Medium, "DVD"),
                (F::CountryRegion, "France"),
                (F::ShortTitle, "Harbour lights"),
            ],
        },
        Probe {
            tag: "Interview",
            kind: K::Interview,
            contributors: vec![
                people(R::Interviewee, &[("Maya", "", "Moore")]),
                people(R::Interviewer, &[("Paul", "", "Page")]),
            ],
            fields: vec![
                (F::Title, "On building bridges"),
                (F::Year, "2012"),
                (F::Month, "October"),
                (F::Day, "21"),
                (F::Broadcaster, "Radio Four"),
                (F::BroadcastTitle, "Morning talk"),
                (F::Station, "FM 93"),
                (F::City, "Manchester"),
                (F::Medium, "Radio"),
                (F::ShortTitle, "Bridges"),
            ],
        },
        Probe {
            tag: "Patent",
            kind: K::Patent,
            contributors: vec![people(R::Inventor, &[("Nina", "", "Nolan")])],
            fields: vec![
                (F::Title, "Folding ladder"),
                (F::Year, "2009"),
                (F::Month, "July"),
                (F::Day, "7"),
                (F::CountryRegion, "United States"),
                (F::PatentType, "Utility"),
                (F::PatentNumber, "7654321"),
                (F::ShortTitle, "Ladder"),
            ],
        },
        Probe {
            tag: "Electronic",
            kind: K::ElectronicSource,
            contributors: vec![people(R::Author, &[("Omar", "", "Owens")])],
            fields: vec![
                (F::Title, "Tide tables"),
                (F::Year, "2011"),
                (F::City, "Brest"),
                (F::Publisher, "Sea Data"),
                (F::Medium, "Database"),
                (F::Edition, "5th"),
                (F::ShortTitle, "Tide tables"),
            ],
        },
        Probe {
            tag: "Case",
            kind: K::Case,
            contributors: Vec::new(),
            fields: vec![
                (F::Title, "Price v. Quinn"),
                (F::CaseNumber, "12-345"),
                (F::Court, "Supreme Court"),
                (F::Year, "2008"),
                (F::Month, "March"),
                (F::Day, "3"),
                (F::Reporter, "U.S."),
                (F::Volume, "550"),
                (F::Pages, "100"),
                (F::AbbreviatedCaseNumber, "No. 12-345"),
                (F::ShortTitle, "Price"),
            ],
        },
        Probe {
            tag: "Misc",
            kind: K::Misc,
            contributors: vec![people(R::Author, &[("Petra", "", "Price")])],
            fields: vec![
                (F::Title, "Garden notes"),
                (F::PublicationTitle, "Allotment letters"),
                (F::Year, "2007"),
                (F::Month, "August"),
                (F::Day, "30"),
                (F::City, "York"),
                (F::Publisher, "Allotment Society"),
                (F::Medium, "Leaflet"),
                (F::ShortTitle, "Garden notes"),
            ],
        },
    ]
}

/// A plain book by `contributors`, as the author-count and missing-field variants.
fn book(
    tag: &'static str,
    contributors: Vec<BibliographyContributor>,
    title: &'static str,
    year: Option<&'static str>,
) -> Probe {
    let mut fields = vec![(F::Title, title)];
    if let Some(year) = year {
        fields.push((F::Year, year));
    }
    fields.extend([(F::City, "Paris"), (F::Publisher, "Seine")]);
    Probe {
        tag,
        kind: K::Book,
        contributors,
        fields,
    }
}

fn variant_probes() -> Vec<Probe> {
    vec![
        book(
            "Authors2",
            vec![people(
                R::Author,
                &[("Quentin", "", "Quinn"), ("Rosa", "", "Reed")],
            )],
            "Two hands",
            Some("2015"),
        ),
        book(
            "Authors3",
            vec![people(
                R::Author,
                &[
                    ("Sam", "", "Stone"),
                    ("Tina", "", "Turner"),
                    ("Uma", "", "Upton"),
                ],
            )],
            "Three voices",
            Some("2014"),
        ),
        book(
            "Authors6",
            vec![people(
                R::Author,
                &[
                    ("Vic", "", "Vance"),
                    ("Wes", "", "Wade"),
                    ("Xia", "", "Xu"),
                    ("Yan", "", "Young"),
                    ("Zoe", "", "Zane"),
                    ("Abe", "", "Ames"),
                ],
            )],
            "Six views",
            Some("2013"),
        ),
        book(
            "Authors8",
            vec![people(
                R::Author,
                &[
                    ("Ben", "", "Bell"),
                    ("Cy", "", "Cross"),
                    ("Di", "", "Dale"),
                    ("Ed", "", "East"),
                    ("Flo", "", "Fox"),
                    ("Gus", "", "Gray"),
                    ("Hal", "", "Hunt"),
                    ("Ivy", "", "Ivers"),
                ],
            )],
            "Eight hands",
            Some("2012"),
        ),
        book(
            "Corporate",
            vec![corporate(R::Author, "World Water Council")],
            "Water for all",
            Some("2016"),
        ),
        book("NoAuthor", Vec::new(), "Anonymous tales", Some("2005")),
        book(
            "NoYear",
            vec![people(R::Author, &[("Lena", "", "Lund")])],
            "Undated notes",
            None,
        ),
    ]
}

fn source(probe: &Probe, index: usize) -> BibliographySource {
    BibliographySource {
        tag: probe.tag.to_owned(),
        guid: format!("{{0B1B0000-0000-4000-8000-{index:012X}}}"),
        kind: probe.kind,
        locale: None,
        contributors: probe.contributors.clone(),
        properties: probe
            .fields
            .iter()
            .map(|(field, value)| BibliographyProperty {
                field: *field,
                value: (*value).to_owned(),
            })
            .collect(),
    }
}

fn member(tag: &str) -> CitationSourceOptions {
    CitationSourceOptions {
        tag: tag.to_owned(),
        pages: None,
        volume: None,
        prefix: None,
        suffix: None,
        suppress_author: false,
        suppress_year: false,
        suppress_title: false,
    }
}

/// The citation switch variants, all on the Book source.
fn switch_citations() -> Vec<Vec<CitationSourceOptions>> {
    let with = |change: fn(&mut CitationSourceOptions)| {
        let mut options = member("Book");
        change(&mut options);
        vec![options]
    };
    vec![
        with(|o| o.pages = Some("12".into())),
        with(|o| o.pages = Some("12-14".into())),
        with(|o| o.volume = Some("2".into())),
        with(|o| {
            o.volume = Some("2".into());
            o.pages = Some("12".into());
        }),
        with(|o| o.prefix = Some("see ".into())),
        with(|o| o.suffix = Some(", emphasis added".into())),
        with(|o| o.suppress_author = true),
        with(|o| o.suppress_year = true),
        with(|o| o.suppress_title = true),
        with(|o| {
            o.suppress_author = true;
            o.suppress_year = true;
        }),
        vec![member("Book"), member("Journal")],
        vec![member("Authors2"), member("Authors3"), member("Corporate")],
    ]
}

fn cite(doc: &mut Document, sources: Vec<CitationSourceOptions>) {
    let label = sources
        .iter()
        .map(|source| source.tag.as_str())
        .collect::<Vec<_>>()
        .join("+");
    doc.add_paragraph(&format!("{label}: "));
    let slot = doc.paragraphs().len() - 1;
    let location = doc
        .direct_body_item_location(slot)
        .expect("paragraph is located")
        .expect("paragraph exists");
    doc.insert_citation(
        &StoryRunPosition {
            location,
            run_index: 1,
        },
        &CitationOptions {
            sources,
            locale: Some(EN_US),
        },
    )
    .expect("citation is inserted");
}

fn probe(style: BibliographyStyle) -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("Bibliography probe: every field asks for en-US.");
    let probes = kind_probes()
        .into_iter()
        .chain(variant_probes())
        .collect::<Vec<_>>();
    for (index, probe) in probes.iter().enumerate() {
        doc.add_bibliography_source(&source(probe, index + 1))
            .expect("source is added");
    }
    for probe in &probes {
        cite(&mut doc, vec![member(probe.tag)]);
    }
    for citation in switch_citations() {
        cite(&mut doc, citation);
    }
    doc.add_paragraph("");
    let slot = doc.paragraphs().len() - 1;
    let end = doc
        .direct_body_item_location(slot)
        .expect("last paragraph is located")
        .expect("last paragraph exists");
    doc.insert_bibliography(
        &end,
        &BibliographyOptions {
            style,
            locale: Some(EN_US),
            locale_filter: None,
            tags: Vec::new(),
        },
    )
    .expect("bibliography is inserted");
    doc
}

fn main() {
    let dir = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&dir).expect("output directory is created");
    for (style, name) in STYLES {
        let path = std::path::Path::new(&dir).join(format!("{name}.docx"));
        probe(style).save(&path).expect("probe is saved");
        println!("{}", path.display());
    }
}
