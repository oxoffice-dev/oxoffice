//! Bibliography entries rendered as paragraphs, left to right.

use super::*;

pub(super) fn source_bibliography_paragraph(
    source: &BibliographySource,
    style: BibliographyStyle,
    reference: Option<u32>,
    locale: BibliographyFormattingLocale,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    if style == BibliographyStyle::ApaSixthEdition {
        let grammar = apa_bibliography_language(locale)?;
        let measured_sparse_dates = source.kind == BibliographySourceKind::DocumentFromInternetSite
            && grammar.publication_month_first;
        if grammar.language != Some("en-US")
            && ((!measured_sparse_dates
                && first_source_property(source, BibliographySourceField::Year).is_empty()
                && !(source.kind == BibliographySourceKind::Book
                    && grammar.missing_year.is_some()
                    && !grammar.publication_month_first))
                || (!measured_sparse_dates
                    && (grammar.day_first
                        || grammar.calendar_year_first
                        || grammar.publication_month_first)
                    && [BibliographySourceField::Month, BibliographySourceField::Day]
                        .iter()
                        .any(|field| first_source_property(source, *field).is_empty()))
                || source
                    .contributors
                    .iter()
                    .enumerate()
                    .any(|(index, contributor)| {
                        source.contributors[..index]
                            .iter()
                            .any(|earlier| earlier.role == contributor.role)
                    })
                || source
                    .contributors
                    .iter()
                    .any(|contributor| match &contributor.value {
                        BibliographyAuthor::People(people) => {
                            people.len() != 1
                                && !(source.kind == BibliographySourceKind::Book
                                    && people.len() == 2
                                    && match contributor.role {
                                        BibliographyContributorRole::Author => grammar.author_pair,
                                        BibliographyContributorRole::Editor => {
                                            !grammar.editors.is_empty()
                                        }
                                        BibliographyContributorRole::Translator => {
                                            !grammar.translators.is_empty()
                                        }
                                        _ => false,
                                    }
                                    && !apa_bibliography_names(
                                        source,
                                        BibliographyContributorRole::Author,
                                        true,
                                        &grammar,
                                    )
                                    .is_empty())
                        }
                        BibliographyAuthor::Corporate(_) => true,
                    })
                || (!measured_sparse_dates
                    && (!first_source_property(source, BibliographySourceField::Url).is_empty()
                        || !first_source_property(
                            source,
                            BibliographySourceField::InternetSiteTitle,
                        )
                        .is_empty())
                    && [
                        BibliographySourceField::MonthAccessed,
                        BibliographySourceField::DayAccessed,
                        BibliographySourceField::YearAccessed,
                    ]
                    .iter()
                    .any(|field| first_source_property(source, *field).is_empty())))
        {
            return Err(not_covered("this localized APA bibliography entry"));
        }
        if grammar.semantic_rtl
            || grammar.numeric_cs_font.is_some()
            || grammar.literal_font.is_some()
        {
            return match source.kind {
                BibliographySourceKind::Book
                | BibliographySourceKind::Report
                | BibliographySourceKind::DocumentFromInternetSite => {
                    apa_rtl_authored_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::Art => apa_rtl_art_bibliography_paragraph(source, &grammar),
                BibliographySourceKind::ArticleInAPeriodical
                | BibliographySourceKind::JournalArticle => {
                    apa_rtl_periodical_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::Patent | BibliographySourceKind::Case => {
                    apa_rtl_legal_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::Performance => {
                    apa_rtl_performance_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::Interview => {
                    apa_rtl_interview_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::ElectronicSource | BibliographySourceKind::Misc => {
                    apa_rtl_general_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::BookSection
                | BibliographySourceKind::ConferenceProceedings => {
                    apa_rtl_chapter_conference_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::InternetSite => {
                    apa_rtl_site_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::SoundRecording => {
                    apa_rtl_sound_bibliography_paragraph(source, &grammar)
                }
                BibliographySourceKind::Film => {
                    apa_rtl_film_bibliography_paragraph(source, &grammar)
                }
            };
        }
        let mut paragraph = match source.kind {
            BibliographySourceKind::Book
            | BibliographySourceKind::Report
            | BibliographySourceKind::DocumentFromInternetSite => {
                apa_authored_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::Patent | BibliographySourceKind::Case => {
                apa_legal_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::ArticleInAPeriodical
            | BibliographySourceKind::JournalArticle => {
                apa_periodical_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::ElectronicSource | BibliographySourceKind::Misc => {
                apa_general_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::BookSection => {
                apa_chapter_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::ConferenceProceedings => {
                apa_conference_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::Film => apa_film_bibliography_paragraph(source, &grammar),
            BibliographySourceKind::InternetSite => {
                apa_site_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::SoundRecording => {
                apa_sound_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::Art => apa_art_bibliography_paragraph(source, &grammar),
            BibliographySourceKind::Performance => {
                apa_performance_bibliography_paragraph(source, &grammar)
            }
            BibliographySourceKind::Interview => {
                apa_interview_bibliography_paragraph(source, &grammar)
            }
        };
        if let Some(properties) = paragraph.properties.as_mut()
            && let Some(properties) = properties.rpr.as_mut()
        {
            properties.language = grammar.language.map(str::to_owned);
        }
        for run in &mut paragraph.runs {
            if let Some(properties) = run.properties.as_mut()
                && properties.language.is_some()
            {
                properties.language = grammar.language.map(str::to_owned);
            }
        }
        return Ok(paragraph);
    }
    let Some(BibliographyAuthor::People(people)) =
        source.contributors.first().map(|value| &value.value)
    else {
        return Err(not_covered(
            "a bibliography entry whose first contributor is not a list of people",
        ));
    };
    let [person] = people.as_slice() else {
        return Err(not_covered(
            "a bibliography entry with more than one author in this style",
        ));
    };
    let last = person.last.join(" ");
    let first = person.first.join(" ");
    let middle = person.middle.join(" ");
    let title = first_source_property(source, BibliographySourceField::Title);
    let year = first_source_property(source, BibliographySourceField::Year);
    let city = first_source_property(source, BibliographySourceField::City);
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if [
        last.as_str(),
        first.as_str(),
        middle.as_str(),
        title,
        year,
        city,
        publisher,
    ]
    .iter()
    .any(|value| !value.is_ascii())
        || [last.as_str(), first.as_str(), title, year, city, publisher]
            .iter()
            .any(|value| value.is_empty())
    {
        return Err(not_covered(
            "a bibliography entry with an empty or non-ASCII author, title, year, city or publisher in this style",
        ));
    }
    let given = if middle.is_empty() {
        first
    } else {
        format!("{first} {middle}")
    };
    let inverted = format!("{last}, {given}.");
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(style));
    match style {
        BibliographyStyle::Chicago => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{inverted} {year}. "),
                false,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(&format!("{title}."), false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!(" {city}: {publisher}."),
                false,
                false,
            ));
        }
        BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Gb7714 => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{inverted} {year}."),
                true,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(" ", false, false));
            paragraph
                .runs
                .push(bibliography_display_run(&format!("{title}. "), false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!("{city}\u{00a0}: {publisher}, {year}."),
                false,
                false,
            ));
        }
        BibliographyStyle::GostName => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{last} {given}"),
                true,
                false,
            ));
            paragraph.runs.push(bibliography_display_run(
                &format!(" {title} [Book].\u{00a0}- {city}\u{00a0}: {publisher}, {year}."),
                false,
                false,
            ));
        }
        BibliographyStyle::GostTitle => {
            paragraph
                .runs
                .push(bibliography_display_run(title, true, false));
            paragraph.runs.push(bibliography_display_run(&format!(" [Book]\u{00a0}/ auth. {last} {given}.\u{00a0}- {city}\u{00a0}: {publisher}, {year}."), false, false));
        }

        BibliographyStyle::HarvardAnglia => {
            let initials = person
                .first
                .iter()
                .chain(&person.middle)
                .flat_map(|part| part.split_whitespace())
                .filter_map(|part| part.chars().next())
                .map(|initial| format!("{initial}."))
                .collect::<Vec<_>>()
                .join(" ");
            paragraph.runs.push(bibliography_display_run(
                &format!("{last}, {initials}, {year}. "),
                false,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(&format!("{title}. "), false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!("{city}: {publisher}."),
                false,
                false,
            ));
        }
        BibliographyStyle::MlaSeventhEdition => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{inverted} "),
                false,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(title, false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!(". {city}: {publisher}, {year}."),
                false,
                false,
            ));
        }
        BibliographyStyle::Sist02 => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{last}{}{}", person.first.join(""), person.middle.join("")),
                true,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(" ", false, false));
            paragraph
                .runs
                .push(bibliography_display_run(title, false, true));
            paragraph.runs.push(east_asian_bibliography_run("．", true));
            paragraph
                .runs
                .push(bibliography_display_run(city, false, false));
            paragraph
                .runs
                .push(east_asian_bibliography_run("，", false));
            paragraph
                .runs
                .push(bibliography_display_run(publisher, false, false));
            paragraph
                .runs
                .push(east_asian_bibliography_run("，", false));
            paragraph
                .runs
                .push(bibliography_display_run(year, false, false));
            paragraph
                .runs
                .push(east_asian_bibliography_run("．", false));
        }

        BibliographyStyle::Iso690Numeric => {
            let number = reference.ok_or_else(|| {
                bibliography_error("missing effective bibliography reference number")
            })?;
            paragraph.runs.push(bibliography_display_run(
                &format!("{number}. "),
                false,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(&inverted, true, false));
            paragraph
                .runs
                .push(bibliography_display_run(" ", false, false));
            paragraph
                .runs
                .push(bibliography_display_run(&format!("{title}. "), false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!("{city}\u{00a0}: {publisher}, {year}."),
                false,
                false,
            ));
        }
        BibliographyStyle::Turabian => {
            paragraph.runs.push(bibliography_display_run(
                &format!("{inverted} "),
                false,
                false,
            ));
            paragraph
                .runs
                .push(bibliography_display_run(&format!("{title}."), false, true));
            paragraph.runs.push(bibliography_display_run(
                &format!(" {city}: {publisher}, {year}."),
                false,
                false,
            ));
        }
        BibliographyStyle::Ieee => {
            let initials = person
                .first
                .iter()
                .chain(&person.middle)
                .flat_map(|part| part.split_whitespace())
                .filter_map(|part| part.chars().next())
                .map(|initial| format!("{initial}."))
                .collect::<Vec<_>>()
                .join(" ");
            paragraph.runs.push(bibliography_display_run(
                &format!("{initials} {last}, {title}, {city}: {publisher}, {year}. "),
                false,
                false,
            ));
        }
        _ => {
            return Err(source_not_covered(
                style,
                "bibliography entry",
                source,
                &locale,
            ));
        }
    }
    Ok(paragraph)
}

pub(super) fn bibliography_display_run(
    text: &str,
    bold: bool,
    italic: bool,
) -> oxoffice_docx_oxml::text::CT_R {
    let mut run = oxoffice_docx_oxml::text::CT_R::new(text);
    run.properties = Some(oxoffice_docx_oxml::CT_RPr {
        no_proof: Some(true),
        language: Some("en-US".into()),
        bold: bold.then_some(true),
        bold_cs: bold.then_some(true),
        italic: italic.then_some(true),
        italic_cs: italic.then_some(true),
        ..Default::default()
    });
    run
}

fn east_asian_bibliography_run(text: &str, italic: bool) -> oxoffice_docx_oxml::text::CT_R {
    let mut run = bibliography_display_run(text, false, italic);
    if let Some(properties) = run.properties.as_mut() {
        properties.font_ascii = Some("MS Gothic".into());
        properties.font_hansi = Some("MS Gothic".into());
        properties.font_east_asia = Some("MS Gothic".into());
        properties.font_cs = Some("MS Gothic".into());
        properties.font_hint = Some("eastAsia".into());
    }
    run
}

pub(super) fn bibliography_paragraph_properties(
    style: BibliographyStyle,
) -> oxoffice_docx_oxml::CT_PPr {
    use oxoffice_docx_oxml::units::{HalfPoint, Twips};
    let hanging = matches!(
        style,
        BibliographyStyle::ApaSixthEdition
            | BibliographyStyle::Chicago
            | BibliographyStyle::MlaSeventhEdition
            | BibliographyStyle::Turabian
    );
    oxoffice_docx_oxml::CT_PPr {
        style_id: Some("Bibliography".into()),
        ind_left: hanging.then_some(Twips(720)),
        ind_hanging: hanging.then_some(Twips(720)),
        rpr: Some(oxoffice_docx_oxml::CT_RPr {
            no_proof: Some(true),
            sz: Some(HalfPoint(24)),
            sz_cs: Some(HalfPoint(24)),
            language: Some("en-US".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(super) fn apa_bibliography_names(
    source: &BibliographySource,
    role: BibliographyContributorRole,
    inverted: bool,
    grammar: &ApaBibliographyLanguage,
) -> String {
    source
        .contributors
        .iter()
        .filter(|contributor| contributor.role == role)
        .map(|contributor| {
            let names = match &contributor.value {
                BibliographyAuthor::Corporate(value) => vec![if value.ends_with('.') {
                    value.clone()
                } else {
                    format!("{value}.")
                }],
                BibliographyAuthor::People(people) => people
                    .iter()
                    .filter_map(|person| {
                        let first = person.first.first().map_or("", String::as_str);
                        let middle = person.middle.first().map_or("", String::as_str);
                        let last = person.last.first().map_or("", String::as_str);
                        if grammar.joined_names {
                            let name = [last, middle, first].join("");
                            return (!name.is_empty()).then_some(name);
                        }
                        if grammar.full_names {
                            let name = [first, middle, last]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" ");
                            return (!name.is_empty()).then_some(name);
                        }
                        let initials = [first, middle]
                            .into_iter()
                            .flat_map(str::split_whitespace)
                            .map(|part| {
                                part.split('-')
                                    .filter_map(|part| part.chars().next())
                                    .map(|initial| format!("{initial}{}", grammar.initial_mark))
                                    .collect::<Vec<_>>()
                                    .join("-")
                            })
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        if last.is_empty() {
                            let given = [first, middle]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" ");
                            return (!given.is_empty()).then(|| {
                                if given.ends_with('.') {
                                    given
                                } else {
                                    format!("{given}.")
                                }
                            });
                        }
                        Some(if initials.is_empty() {
                            if last.ends_with('.') {
                                last.to_owned()
                            } else {
                                format!("{last}.")
                            }
                        } else if inverted {
                            format!("{last}, {initials}")
                        } else {
                            format!("{initials} {last}")
                        })
                    })
                    .collect::<Vec<_>>(),
            };
            match names.as_slice() {
                [] => String::new(),
                [name] => name.clone(),
                names
                    if names.len() <= 7
                        && role == BibliographyContributorRole::Editor
                        && !inverted =>
                {
                    names.join(grammar.contributor_separator)
                }
                names if names.len() <= 7 => format!(
                    "{}{}{} {}",
                    names[..names.len() - 1].join(grammar.contributor_separator),
                    grammar.contributor_separator,
                    grammar.conjunction,
                    names.last().expect("nonempty names")
                ),
                names => format!(
                    "{}, . . . {}",
                    names[..6].join(", "),
                    names.last().expect("nonempty names")
                ),
            }
        })
        .collect::<String>()
}

pub(super) fn bibliography_display_runs(
    text: &str,
    italic: bool,
) -> Vec<oxoffice_docx_oxml::text::CT_R> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut east_asian = None;
    for (offset, character) in text.char_indices() {
        let current = matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}');
        if east_asian.is_some_and(|previous| previous != current) {
            let fragment = &text[start..offset];
            runs.push(if east_asian == Some(true) {
                east_asian_bibliography_run(fragment, italic)
            } else {
                bibliography_display_run(fragment, false, italic)
            });
            start = offset;
        }
        east_asian = Some(current);
    }
    if start < text.len() {
        runs.push(if east_asian == Some(true) {
            east_asian_bibliography_run(&text[start..], italic)
        } else {
            bibliography_display_run(&text[start..], false, italic)
        });
    }
    runs
}

pub(super) fn apa_bibliography_edition(edition: &str, grammar: &ApaBibliographyLanguage) -> String {
    if grammar.edition_before {
        format!(
            "{}{}{edition}{}",
            grammar.edition, grammar.edition_separator, grammar.edition_suffix
        )
    } else {
        format!(
            "{edition}{}{}{}",
            grammar.edition_separator, grammar.edition, grammar.edition_suffix
        )
    }
}

pub(super) fn apa_bibliography_volume(volume: &str, grammar: &ApaBibliographyLanguage) -> String {
    if grammar.volume_before {
        format!(
            "{}{}{volume}{}",
            grammar.volume, grammar.volume_separator, grammar.volume_suffix
        )
    } else {
        format!(
            "{volume}{}{}{}",
            grammar.volume_separator, grammar.volume, grammar.volume_suffix
        )
    }
}

pub(super) fn apa_bibliography_date(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> String {
    let year = first_source_property(source, BibliographySourceField::Year);
    let date = if year.is_empty() {
        grammar.missing_year.unwrap_or_default().to_owned()
    } else {
        year.to_owned()
    };
    let month = first_source_property(source, BibliographySourceField::Month);
    let day = first_source_property(source, BibliographySourceField::Day);
    if grammar.publication_month_first && !year.is_empty() && !month.is_empty() {
        return if day.is_empty() {
            format!("{month}{}{year}", grammar.month_year)
        } else {
            format!("{month} {day}{}{year}", grammar.calendar_day_year)
        };
    }
    if grammar.day_first && !year.is_empty() && !month.is_empty() {
        return format!(
            "{}{day}{}{month}{}{year}{}",
            grammar.date_prefix, grammar.day_month, grammar.month_year, grammar.date_suffix
        );
    }
    if !year.is_empty() && !month.is_empty() {
        return apa_bibliography_year_date(year, month, day, grammar);
    }
    date
}

fn apa_bibliography_year_date(
    year: &str,
    month: &str,
    day: &str,
    grammar: &ApaBibliographyLanguage,
) -> String {
    if grammar.year_day_first {
        return format!(
            "{year}{}{day}{}{month}{}",
            grammar.year_month, grammar.day_month, grammar.date_suffix
        );
    }
    let mut date = format!("{year}{}{month}", grammar.year_month);
    if !day.is_empty() {
        date.push_str(&format!("{}{day}", grammar.month_day));
    }
    date.push_str(grammar.date_suffix);
    date
}

fn apa_bibliography_retrieval(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> String {
    let mut publication = String::new();
    let url = first_source_property(source, BibliographySourceField::Url);
    let site = first_source_property(source, BibliographySourceField::InternetSiteTitle);
    if !url.is_empty() || !site.is_empty() {
        let month = first_source_property(source, BibliographySourceField::MonthAccessed);
        let day = first_source_property(source, BibliographySourceField::DayAccessed);
        let year = first_source_property(source, BibliographySourceField::YearAccessed);
        publication.push_str(if month.is_empty() && year.is_empty() {
            grammar.retrieved_without_date.unwrap_or(grammar.retrieved)
        } else {
            grammar.retrieved
        });
        if grammar.calendar_year_first && !grammar.retrieval_date_after {
            publication.push_str(grammar.retrieval_date_separator);
            publication.push_str(&apa_bibliography_year_date(year, month, day, grammar));
        } else if grammar.day_first && !grammar.retrieval_date_after {
            let date = format!(
                "{day}{}{month}{}{year}",
                grammar.day_month, grammar.month_year
            );
            if !date.is_empty() {
                publication.push_str(grammar.retrieval_date_separator);
                publication.push_str(&date);
            }
        } else if !grammar.retrieval_date_after {
            if !month.is_empty() {
                publication.push_str(&format!("{}{month}", grammar.retrieval_date_separator));
                if !day.is_empty() {
                    publication.push_str(&format!(" {day}"));
                }
            }
            if !year.is_empty() {
                publication.push_str(if !month.is_empty() && !day.is_empty() {
                    grammar.calendar_day_year
                } else if !month.is_empty() && grammar.publication_month_first {
                    grammar.month_year
                } else {
                    " "
                });
                publication.push_str(year);
            }
        }
        publication.push_str(if month.is_empty() && year.is_empty() {
            grammar.from_without_date
        } else {
            grammar.from_with_date
        });
        if !site.is_empty() {
            publication.push_str(site);
            publication.push_str(if url.is_empty() { "." } else { ": " });
        }
        publication.push_str(url);
        if grammar.retrieval_date_after {
            publication.push_str(grammar.retrieval_after_prefix);
            publication.push_str(grammar.date_prefix);
            if grammar.calendar_year_first {
                publication.push_str(&apa_bibliography_year_date(year, month, day, grammar));
            } else if grammar.day_first {
                publication.push_str(&format!(
                    "{day}{}{month}{}{year}",
                    grammar.day_month, grammar.month_year
                ));
            } else {
                publication.push_str(&format!(
                    "{month}{}{day}{}{year}",
                    grammar.month_day, grammar.calendar_day_year
                ));
            }
        }
        publication.push_str(grammar.retrieval_suffix);
    }
    publication
}

fn apa_legal_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    let retrieval = apa_bibliography_retrieval(source, grammar);
    if source.kind == BibliographySourceKind::Patent {
        let inventor =
            apa_bibliography_names(source, BibliographyContributorRole::Inventor, true, grammar);
        let date = apa_bibliography_date(source, grammar);
        let mut prefix = if inventor.is_empty() {
            format!("({date}).")
        } else {
            format!("{inventor} ({date}).")
        };
        let country = first_source_property(source, BibliographySourceField::CountryRegion);
        let number = first_source_property(source, BibliographySourceField::PatentNumber);
        if country.is_empty() && number.is_empty() {
            prefix.push_str(&retrieval);
            paragraph
                .runs
                .extend(bibliography_display_runs(&prefix, false));
        } else {
            paragraph
                .runs
                .extend(bibliography_display_runs(&prefix, false));
            paragraph.runs.extend(bibliography_display_runs(
                &format!(
                    " {}{}{number}{}.",
                    if country.is_empty() {
                        String::new()
                    } else {
                        format!("{country} ")
                    },
                    grammar.patent,
                    grammar.patent_suffix
                ),
                true,
            ));
            paragraph
                .runs
                .extend(bibliography_display_runs(&retrieval, false));
        }
    } else {
        let title = first_source_property(source, BibliographySourceField::Title);
        let number = first_source_property(source, BibliographySourceField::CaseNumber);
        let court = first_source_property(source, BibliographySourceField::Court);
        let year = first_source_property(source, BibliographySourceField::Year);
        let month = first_source_property(source, BibliographySourceField::Month);
        let day = first_source_property(source, BibliographySourceField::Day);
        let mut text = title.to_owned();
        if !number.is_empty() {
            if !text.is_empty() {
                text.push_str(", ");
            }
            text.push_str(number);
        }
        let mut date = court.to_owned();
        if grammar.day_first || grammar.calendar_year_first {
            if !date.is_empty() {
                date.push(' ');
            }
            date.push_str(&apa_bibliography_date(source, grammar));
        } else {
            if !month.is_empty() {
                if !date.is_empty() {
                    date.push(' ');
                }
                date.push_str(month);
                if !day.is_empty() {
                    date.push_str(&format!(" {day}"));
                }
            }
            if !year.is_empty() {
                if !date.is_empty() {
                    date.push_str(if month.is_empty() {
                        " "
                    } else {
                        grammar.calendar_day_year
                    });
                }
                date.push_str(year);
            }
        }
        if !date.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("({date})"));
        }
        let invalid = text.is_empty();
        if invalid {
            text.push_str("Invalid source specified");
        }
        text.push('.');
        text.push_str(&retrieval);
        if invalid {
            let mut run = bibliography_display_run(&text, true, false);
            if let Some(properties) = run.properties.as_mut() {
                properties.language = None;
            }
            paragraph.runs.push(run);
        } else {
            paragraph
                .runs
                .extend(bibliography_display_runs(&text, false));
        }
    }
    paragraph
}

fn apa_general_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let mut prefix = apa_author_title_prefix(source, grammar);
    let publication = first_source_property(source, BibliographySourceField::PublicationTitle);
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let issue = first_source_property(source, BibliographySourceField::Issue);
    let edition = first_source_property(source, BibliographySourceField::Edition);
    let pages = first_source_property(source, BibliographySourceField::Pages);
    let miscellaneous = source.kind == BibliographySourceKind::Misc;
    let mut heading = publication.to_owned();
    if !volume.is_empty() {
        if !heading.is_empty() {
            heading.push_str(", ");
        }
        heading.push_str(volume);
    }
    if miscellaneous {
        if !issue.is_empty() {
            heading.push_str(&format!("({issue})"));
        }
        if !edition.is_empty() {
            if !heading.is_empty() {
                heading.push_str(", ");
            }
            heading.push_str(edition);
        }
    }
    let mut suffix = String::new();
    if !miscellaneous && !edition.is_empty() {
        suffix.push_str(&format!("({edition})"));
    }
    if !pages.is_empty() {
        if !heading.is_empty() || !suffix.is_empty() {
            suffix.push_str(", ");
        }
        suffix.push_str(pages);
    }
    if !heading.is_empty() || !suffix.is_empty() {
        suffix.push('.');
    }
    let mut secondary = Vec::new();
    for (role, label) in [
        (BibliographyContributorRole::Editor, grammar.editor),
        (BibliographyContributorRole::Translator, grammar.translator),
        (BibliographyContributorRole::Compiler, grammar.compiler),
    ] {
        let name = apa_bibliography_names(source, role, false, grammar);
        if !name.is_empty() {
            secondary.push(format!("{name}, {label}"));
        }
    }
    if !secondary.is_empty() {
        let names = if secondary.len() == 1 {
            secondary[0].clone()
        } else {
            format!(
                "{}{}{} {}",
                secondary[..secondary.len() - 1].join(grammar.contributor_separator),
                grammar.contributor_separator,
                grammar.conjunction,
                secondary.last().unwrap()
            )
        };
        suffix.push_str(&format!(" ({names})"));
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if !location.is_empty() || !publisher.is_empty() {
        suffix.push(' ');
        suffix.push_str(&location);
        if !location.is_empty() && !publisher.is_empty() {
            suffix.push_str(": ");
        }
        suffix.push_str(publisher);
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if heading.is_empty() {
        if !suffix.is_empty() && !suffix.starts_with(' ') {
            prefix.push(' ');
        }
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    } else {
        prefix.push(' ');
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph
            .runs
            .extend(bibliography_display_runs(&heading, true));
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    }
    paragraph
}

fn apa_interview_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let creator = apa_bibliography_names(
        source,
        BibliographyContributorRole::Interviewee,
        true,
        grammar,
    );
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let broadcast = first_source_property(source, BibliographySourceField::BroadcastTitle);
    let mut prefix = if creator.is_empty() {
        format!("({date}).")
    } else {
        format!("{creator} ({date}).")
    };
    if !title.is_empty() {
        prefix.push_str(&format!(" {title}."));
    }
    let interviewer = apa_bibliography_names(
        source,
        BibliographyContributorRole::Interviewer,
        false,
        grammar,
    );
    let mut suffix = String::new();
    if broadcast.is_empty() {
        let pages = first_source_property(source, BibliographySourceField::Pages);
        if !pages.is_empty() {
            prefix.push_str(&format!(" {pages}."));
        }
    }
    let mut secondary = Vec::new();
    if !interviewer.is_empty() {
        secondary.push(format!("{interviewer}, {}", grammar.interviewer));
    }
    if broadcast.is_empty() {
        for (role, label) in [
            (BibliographyContributorRole::Editor, grammar.site_editor),
            (BibliographyContributorRole::Translator, "Translator"),
        ] {
            let name = apa_bibliography_names(source, role, false, grammar);
            if !name.is_empty() {
                secondary.push(format!("{name}, {label}"));
            }
        }
    }
    if !secondary.is_empty() {
        let names = if secondary.len() == 1 {
            secondary[0].clone()
        } else {
            format!(
                "{}{}{} {}",
                secondary[..secondary.len() - 1].join(grammar.contributor_separator),
                grammar.contributor_separator,
                grammar.conjunction,
                secondary.last().unwrap()
            )
        };
        suffix.push_str(&format!(" ({names})"));
    }
    if broadcast.is_empty() {
        let location = [
            BibliographySourceField::City,
            BibliographySourceField::StateProvince,
            BibliographySourceField::CountryRegion,
        ]
        .into_iter()
        .map(|field| first_source_property(source, field))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        let publisher = first_source_property(source, BibliographySourceField::Publisher);
        if !location.is_empty() || !publisher.is_empty() {
            suffix.push(' ');
            suffix.push_str(&location);
            if !location.is_empty() && !publisher.is_empty() {
                suffix.push_str(": ");
            }
            suffix.push_str(publisher);
            suffix.push('.');
        }
    } else {
        let broadcaster = first_source_property(source, BibliographySourceField::Broadcaster);
        if !broadcaster.is_empty() {
            suffix.push_str(&format!(" {broadcaster}."));
        }
        let location = [
            BibliographySourceField::Station,
            BibliographySourceField::City,
        ]
        .into_iter()
        .map(|field| first_source_property(source, field))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        if !location.is_empty() {
            suffix.push_str(&format!(" {location}."));
        }
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if broadcast.is_empty() {
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    } else {
        prefix.push(' ');
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph
            .runs
            .extend(bibliography_display_runs(&format!("{broadcast}."), true));
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    }
    paragraph
}

fn apa_author_title_prefix(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> String {
    let author = apa_bibliography_names(source, BibliographyContributorRole::Author, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    if author.is_empty() {
        if title.is_empty() {
            format!("({date}).")
        } else {
            format!("{title}. ({date}).")
        }
    } else {
        format!(
            "{author} ({date}).{}",
            if title.is_empty() {
                String::new()
            } else {
                format!(" {title}.")
            }
        )
    }
}

fn apa_chapter_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let mut prefix = apa_author_title_prefix(source, grammar);
    let author = apa_bibliography_names(
        source,
        BibliographyContributorRole::BookAuthor,
        false,
        grammar,
    );
    let editor =
        apa_bibliography_names(source, BibliographyContributorRole::Editor, false, grammar);
    let book = first_source_property(source, BibliographySourceField::BookTitle);
    let mut names = author;
    if !editor.is_empty() {
        if !names.is_empty() {
            names.push_str(&format!(
                "{}{} ",
                grammar.contributor_separator, grammar.conjunction
            ));
        }
        names.push_str(&format!("{editor} ({})", grammar.editor));
    }
    if !names.is_empty() || !book.is_empty() {
        prefix.push(' ');
        if !grammar.in_after && !grammar.in_label.is_empty() {
            prefix.push_str(grammar.in_label);
            prefix.push_str(grammar.in_before_separator);
        }
        prefix.push_str(&names);
        if !names.is_empty() {
            if grammar.in_after && !grammar.in_label.is_empty() {
                prefix.push_str(&format!("{}{}", grammar.in_separator, grammar.in_label));
            }
            prefix.push_str(if book.is_empty() { "." } else { ", " });
        }
    }
    let mut suffix = String::new();
    if !book.is_empty() {
        let translator = apa_bibliography_names(
            source,
            BibliographyContributorRole::Translator,
            false,
            grammar,
        );
        let edition = first_source_property(source, BibliographySourceField::Edition);
        let volume = first_source_property(source, BibliographySourceField::Volume);
        let pages = first_source_property(source, BibliographySourceField::Pages);
        let mut details = Vec::new();
        if !translator.is_empty() {
            details.push(format!("{translator}, {}", grammar.translator));
        }
        if !edition.is_empty() {
            details.push(apa_bibliography_edition(edition, grammar));
        }
        if !volume.is_empty() {
            details.push(apa_bibliography_volume(volume, grammar));
        }
        if !pages.is_empty() {
            details.push(format!("{}{pages}", grammar.pages));
        }
        if !details.is_empty() {
            suffix.push_str(&format!(" ({})", details.join(", ")));
        }
        suffix.push('.');
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if !location.is_empty() || !publisher.is_empty() {
        suffix.push(' ');
        suffix.push_str(&location);
        if !location.is_empty() && !publisher.is_empty() {
            suffix.push_str(": ");
        }
        suffix.push_str(publisher);
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if book.is_empty() {
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    } else {
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph.runs.extend(bibliography_display_runs(book, true));
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    }
    paragraph
}

fn apa_conference_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let mut prefix = apa_author_title_prefix(source, grammar);
    let editor =
        apa_bibliography_names(source, BibliographyContributorRole::Editor, false, grammar);
    let conference = first_source_property(source, BibliographySourceField::ConferenceName);
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let pages = first_source_property(source, BibliographySourceField::Pages);
    if !editor.is_empty() {
        prefix.push(' ');
        if !grammar.in_after && !grammar.in_label.is_empty() {
            prefix.push_str(grammar.in_label);
            prefix.push_str(grammar.in_before_separator);
        }
        prefix.push_str(&format!(
            "{editor} ({}){}",
            grammar.editor,
            if conference.is_empty() { "." } else { "," }
        ));
    }
    if !conference.is_empty() || !volume.is_empty() {
        prefix.push(' ');
    }
    let city = first_source_property(source, BibliographySourceField::City);
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    let mut suffix = String::new();
    if !pages.is_empty() {
        suffix.push_str(&format!(", {}{pages}.", grammar.pages));
    }
    if !city.is_empty() || !publisher.is_empty() {
        suffix.push(' ');
        suffix.push_str(city);
        if !city.is_empty() && !publisher.is_empty() {
            suffix.push_str(": ");
        }
        suffix.push_str(publisher);
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if conference.is_empty() && volume.is_empty() {
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    } else {
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        if !conference.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&format!("{conference}."), true));
            if !volume.is_empty() {
                paragraph.runs.extend(bibliography_display_runs(
                    &if grammar.in_after && !grammar.in_label.is_empty() {
                        format!("{}{} ", grammar.in_separator, grammar.in_label)
                    } else {
                        " ".to_owned()
                    },
                    false,
                ));
            }
        }
        if !volume.is_empty() {
            if conference.is_empty() && grammar.in_after && !grammar.in_label.is_empty() {
                paragraph.runs.extend(bibliography_display_runs(
                    &format!("{} ", grammar.in_label),
                    false,
                ));
            }
            paragraph.runs.extend(bibliography_display_runs(
                &format!("{volume}{}", if pages.is_empty() { "." } else { "" }),
                true,
            ));
        }
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    }
    paragraph
}

fn apa_film_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let mut creators = Vec::new();
    for (role, label) in [
        (BibliographyContributorRole::ProducerName, grammar.producer),
        (BibliographyContributorRole::Writer, grammar.writer),
        (BibliographyContributorRole::Director, grammar.director),
    ] {
        let name = apa_bibliography_names(source, role, true, grammar);
        if !name.is_empty() {
            creators.push(format!("{name} ({label})"));
        }
    }
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    let mut suffix = String::new();
    if creators.is_empty() && !title.is_empty() {
        paragraph
            .runs
            .extend(bibliography_display_runs(title, true));
        suffix.push_str(&format!(" ({date})."));
    } else {
        let prefix = if creators.is_empty() {
            format!("({date}).")
        } else {
            let names = if creators.len() == 1 {
                creators[0].clone()
            } else {
                format!(
                    "{}{}{} {}",
                    creators[..creators.len() - 1].join(grammar.contributor_separator),
                    grammar.contributor_separator,
                    grammar.conjunction,
                    creators.last().unwrap()
                )
            };
            format!(
                "{names}. ({date}).{}",
                if title.is_empty() { "" } else { " " }
            )
        };
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(title, true));
        }
    }
    suffix.push_str(&format!(" [{}].", grammar.motion_picture));
    let country = first_source_property(source, BibliographySourceField::CountryRegion);
    let distributor = first_source_property(source, BibliographySourceField::Distributor);
    if !country.is_empty() || !distributor.is_empty() {
        suffix.push(' ');
        suffix.push_str(country);
        if !country.is_empty() && !distributor.is_empty() {
            suffix.push_str(": ");
        }
        suffix.push_str(distributor);
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    if title.is_empty() && paragraph.runs.len() == 1 {
        suffix.insert_str(0, &paragraph.runs.pop().unwrap().text());
    }
    paragraph
        .runs
        .extend(bibliography_display_runs(&suffix, false));
    paragraph
}

fn apa_site_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let creator =
        apa_bibliography_names(source, BibliographyContributorRole::Author, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let version = first_source_property(source, BibliographySourceField::Version);
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    let mut suffix = String::new();
    if creator.is_empty() && !title.is_empty() {
        paragraph
            .runs
            .extend(bibliography_display_runs(title, true));
        suffix.push_str(&format!(". ({date})."));
    } else {
        let prefix = if creator.is_empty() {
            format!("({date}).")
        } else {
            format!(
                "{creator} ({date}).{}",
                if title.is_empty() { "" } else { " " }
            )
        };
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(title, true));
            if !version.is_empty() {
                suffix.push_str(&format!(", {version}"));
            }
            suffix.push('.');
        }
    }
    let editor =
        apa_bibliography_names(source, BibliographyContributorRole::Editor, false, grammar);
    let producer = apa_bibliography_names(
        source,
        BibliographyContributorRole::ProducerName,
        false,
        grammar,
    );
    let company = first_source_property(source, BibliographySourceField::ProductionCompany);
    let mut secondary = Vec::new();
    if !editor.is_empty() {
        secondary.push(format!("{editor}, {}", grammar.site_editor));
    }
    if !producer.is_empty() {
        secondary.push(format!("{producer}, {}", grammar.producer));
    }
    if !company.is_empty() {
        secondary.push(company.to_owned());
    }
    if !secondary.is_empty() {
        let joined = if secondary.len() == 1 {
            secondary[0].clone()
        } else {
            format!(
                "{}{}{} {}",
                secondary[..secondary.len() - 1].join(grammar.contributor_separator),
                grammar.contributor_separator,
                grammar.conjunction,
                secondary.last().unwrap()
            )
        };
        suffix.push_str(&format!(" ({joined})"));
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    if paragraph.runs.len() == 1 && title.is_empty() {
        let prefix = paragraph.runs.pop().unwrap().text();
        suffix.insert_str(0, &prefix);
    }
    paragraph
        .runs
        .extend(bibliography_display_runs(&suffix, false));
    paragraph
}

fn apa_sound_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let creator =
        apa_bibliography_names(source, BibliographyContributorRole::Composer, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let performer = apa_bibliography_names(
        source,
        BibliographyContributorRole::Performer,
        false,
        grammar,
    );
    let mut prefix = if creator.is_empty() {
        if title.is_empty() {
            format!("({date}).")
        } else {
            format!("{title}. ({date}).")
        }
    } else {
        format!(
            "{creator} ({date}).{}",
            if title.is_empty() {
                String::new()
            } else {
                format!(" {title}")
            }
        )
    };
    if !performer.is_empty() {
        prefix.push_str(&if grammar.recorded_after {
            format!(
                " [{}{performer}{}{}].",
                grammar.recorded_prefix, grammar.recorded_separator, grammar.recorded_by
            )
        } else {
            format!(
                " [{}{}{performer}].",
                grammar.recorded_by, grammar.recorded_before_separator
            )
        });
    } else if !creator.is_empty() && !title.is_empty() {
        prefix.push('.');
    }
    let album = first_source_property(source, BibliographySourceField::AlbumTitle);
    let medium = first_source_property(source, BibliographySourceField::Medium);
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let producer = apa_bibliography_names(
        source,
        BibliographyContributorRole::ProducerName,
        false,
        grammar,
    );
    let mut suffix = if medium.is_empty() {
        if album.is_empty() {
            String::new()
        } else {
            ".".to_owned()
        }
    } else {
        format!(" [{medium}].")
    };
    if !location.is_empty() || !producer.is_empty() {
        suffix.push(' ');
        suffix.push_str(&location);
        if !location.is_empty() && !producer.is_empty() {
            suffix.push_str(": ");
        }
        suffix.push_str(&producer);
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if album.is_empty() {
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    } else {
        prefix.push(' ');
        if !grammar.on_after && !grammar.on_label.is_empty() {
            prefix.push_str(grammar.on_label);
            prefix.push_str(grammar.on_before_separator);
        }
        if grammar.on_after && !grammar.on_label.is_empty() {
            suffix.insert_str(0, &format!("{}{}", grammar.on_separator, grammar.on_label));
        }
        if grammar.quote_album {
            prefix.push('"');
            suffix.insert(0, '"');
        }
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph
            .runs
            .extend(bibliography_display_runs(album, true));
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    }
    paragraph
}

fn apa_art_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let creator =
        apa_bibliography_names(source, BibliographyContributorRole::Artist, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let publication = first_source_property(source, BibliographySourceField::PublicationTitle);
    let mut prefix = if creator.is_empty() {
        format!("({date}).")
    } else {
        format!(
            "{creator} ({date}).{}",
            if title.is_empty() { "" } else { " " }
        )
    };
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if !publication.is_empty() {
        if !title.is_empty() {
            prefix.push_str(&format!("{title}. "));
        }
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph
            .runs
            .extend(bibliography_display_runs(&format!("{publication}."), true));
    } else {
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&format!("{title}."), true));
        }
    }
    let location = [
        BibliographySourceField::Institution,
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let mut suffix = if location.is_empty() {
        if title.is_empty() && publication.is_empty() {
            String::new()
        } else {
            " ".to_owned()
        }
    } else {
        format!(" {location}.")
    };
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    paragraph
        .runs
        .extend(bibliography_display_runs(&suffix, false));
    paragraph
}

fn apa_performance_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let creator =
        apa_bibliography_names(source, BibliographyContributorRole::Writer, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    let mut suffix = String::new();
    if creator.is_empty() && !title.is_empty() {
        paragraph
            .runs
            .extend(bibliography_display_runs(&format!("{title}."), true));
        suffix.push_str(&format!(" ({date})."));
    } else {
        let prefix = if creator.is_empty() {
            format!("({date}).")
        } else {
            format!(
                "{creator} ({date}).{}",
                if title.is_empty() { "" } else { " " }
            )
        };
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&format!("{title}."), true));
        }
    }
    let director = apa_bibliography_names(
        source,
        BibliographyContributorRole::Director,
        false,
        grammar,
    );
    let performer = apa_bibliography_names(
        source,
        BibliographyContributorRole::Performer,
        false,
        grammar,
    );
    let mut secondary = Vec::new();
    if !director.is_empty() {
        secondary.push(format!("{director}, {}", grammar.director));
    }
    if !performer.is_empty() {
        secondary.push(format!("{performer}, {}", grammar.performer));
    }
    if !secondary.is_empty() {
        suffix.push_str(&format!(
            " ({})",
            secondary.join(&format!(
                "{}{} ",
                grammar.contributor_separator, grammar.conjunction
            ))
        ));
    }
    let location = [
        BibliographySourceField::Theater,
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    if !location.is_empty() {
        suffix.push_str(&format!(" {location}."));
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    paragraph
        .runs
        .extend(bibliography_display_runs(&suffix, false));
    paragraph
}

fn apa_bibliography_secondary(
    source: &BibliographySource,
    editor_fallback: bool,
    grammar: &ApaBibliographyLanguage,
) -> String {
    let count = |role| {
        source
            .contributors
            .iter()
            .filter(|value| value.role == role)
            .map(|value| match &value.value {
                BibliographyAuthor::People(people) => people.len(),
                BibliographyAuthor::Corporate(_) => 1,
            })
            .sum::<usize>()
    };
    let editor =
        apa_bibliography_names(source, BibliographyContributorRole::Editor, false, grammar);
    let translator = apa_bibliography_names(
        source,
        BibliographyContributorRole::Translator,
        false,
        grammar,
    );
    let mut secondary = Vec::new();
    if !editor.is_empty() && !editor_fallback {
        secondary.push(format!(
            "{editor}, {}",
            if count(BibliographyContributorRole::Editor) > 1 {
                grammar.editors
            } else {
                grammar.editor
            }
        ));
    }
    if !translator.is_empty() {
        secondary.push(format!(
            "{translator}, {}",
            if count(BibliographyContributorRole::Translator) > 1 {
                grammar.translators
            } else {
                grammar.translator
            }
        ));
    }
    if secondary.is_empty() {
        String::new()
    } else {
        format!(
            " ({})",
            secondary.join(&if count(BibliographyContributorRole::Translator) > 1 {
                grammar.contributor_separator.to_owned()
            } else {
                format!(
                    "{}{}{}",
                    grammar.contributor_separator,
                    grammar.conjunction,
                    if grammar.conjunction.is_empty() {
                        ""
                    } else {
                        " "
                    }
                )
            })
        )
    }
}

fn apa_periodical_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    let author = apa_bibliography_names(source, BibliographyContributorRole::Author, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    let mut prefix = if author.is_empty() {
        if title.is_empty() {
            format!("({date}).")
        } else {
            format!("{title}. ({date}).")
        }
    } else {
        format!(
            "{author} ({date}).{}",
            if title.is_empty() {
                String::new()
            } else {
                format!(" {title}.")
            }
        )
    };
    prefix.push_str(&apa_bibliography_secondary(source, false, grammar));
    let periodical = first_source_property(
        source,
        if source.kind == BibliographySourceKind::JournalArticle {
            BibliographySourceField::JournalName
        } else {
            BibliographySourceField::PeriodicalTitle
        },
    );
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let issue = first_source_property(source, BibliographySourceField::Issue);
    let pages = first_source_property(source, BibliographySourceField::Pages);
    let mut heading = periodical.to_owned();
    if !volume.is_empty() {
        if !heading.is_empty() {
            heading.push_str(", ");
        }
        heading.push_str(volume);
    }
    let mut suffix = String::new();
    if !issue.is_empty() {
        suffix.push_str(&format!("({issue})"));
    }
    if !pages.is_empty() {
        if !heading.is_empty() || !suffix.is_empty() {
            suffix.push_str(", ");
        }
        if source.kind == BibliographySourceKind::ArticleInAPeriodical {
            suffix.push_str(grammar.pages);
        }
        suffix.push_str(pages);
    }
    if !heading.is_empty() || !suffix.is_empty() {
        suffix.push('.');
    }
    suffix.push_str(&apa_bibliography_retrieval(source, grammar));
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    if !heading.is_empty() {
        prefix.push(' ');
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
        paragraph
            .runs
            .extend(bibliography_display_runs(&heading, true));
        paragraph
            .runs
            .extend(bibliography_display_runs(&suffix, false));
    } else {
        if !suffix.is_empty() && !suffix.starts_with(' ') {
            prefix.push(' ');
        }
        prefix.push_str(&suffix);
        paragraph
            .runs
            .extend(bibliography_display_runs(&prefix, false));
    }
    paragraph
}

pub(super) fn append_apa_semantic_run(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    grammar: &ApaBibliographyLanguage,
    text: &str,
    italic: bool,
    rtl: bool,
) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let italic = italic && !grammar.no_italic;
    let properties = oxoffice_docx_oxml::CT_RPr {
        no_proof: Some(true),
        font_cs: if rtl {
            grammar.rtl_cs_font.map(str::to_owned)
        } else {
            None
        },
        font_hint: if grammar.language_east_asia.is_some() {
            Some("eastAsia".into())
        } else {
            grammar.literal_font.is_none().then(|| "cs".into())
        },
        language: grammar.language.map(str::to_owned),
        language_bidi: grammar.language_bidi.map(str::to_owned),
        language_east_asia: grammar.language_east_asia.map(str::to_owned),
        italic: italic.then_some(true),
        italic_cs: italic.then_some(true),
        rtl: (rtl && grammar.semantic_rtl).then_some(true),
        ..Default::default()
    };
    if matches!(
        grammar.language_bidi,
        Some(
            "ug-CN" | "tzm-Arab-MA" | "ur-PK" | "ur-IN" | "yi-Hebr" | "he-IL" | "dv-MV" | "syr-SY"
        )
    ) && !rtl
    {
        // Own paired Title/Last source probes distinguish a measured source-script span, its
        // numeric/neutral suffix, and the next Latin span. Generated operands
        // retain their separate caller-provided direction.
        let source_script = match grammar.language_bidi {
            Some("yi-Hebr" | "he-IL") => '\u{590}'..='\u{5ff}',
            Some("syr-SY") => '\u{700}'..='\u{74f}',
            Some("dv-MV") => '\u{780}'..='\u{7bf}',
            _ => '\u{600}'..='\u{6ff}',
        };
        let mut rtl_source = false;
        for (offset, character) in text.char_indices() {
            if source_script.contains(&character) {
                rtl_source = true;
            } else if character.is_ascii_alphabetic() {
                rtl_source = false;
            } else if character == ' '
                && text[offset + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(|next| source_script.contains(&next))
            {
                rtl_source = true;
            }
            let mut actual = properties.clone();
            actual.rtl = rtl_source.then_some(true);
            if matches!(grammar.language_bidi, Some("dv-MV" | "syr-SY")) {
                // Completed own source and generated neutral controls share the own cs font.
                // Keep the measured Latin suffix free of an explicit cs font.
                actual.font_cs = if rtl_source {
                    grammar.rtl_cs_font.map(str::to_owned)
                } else {
                    None
                };
            }
            append_apa_semantic_text(paragraph, &character.to_string(), actual)?;
        }
        return Ok(());
    }
    if rtl && grammar.language_bidi == Some("syr-SY") {
        // Own completed Syriac controls distinguish neutral generated glue from
        // translated Arabic labels. Original source operands enter separately.
        let previous_arabic = paragraph
            .runs
            .iter()
            .rev()
            .find_map(|run| {
                run.properties
                    .as_ref()
                    .filter(|actual| actual.rtl == Some(true))
            })
            .is_some_and(|actual| actual.language_bidi.is_none());
        let first_arabic = text
            .char_indices()
            .find(|(_, character)| ('\u{600}'..='\u{6ff}').contains(character))
            .map(|(offset, _)| offset);
        let label_start = if text.trim_start_matches(' ').starts_with('(') {
            first_arabic.unwrap_or(0)
        } else {
            text.len() - text.trim_start_matches(' ').len()
        };
        let continuation =
            first_arabic.is_none() && previous_arabic && !text.starts_with([' ', '(']);
        let trailing_glue = text.ends_with(&format!("{} ", grammar.conjunction))
            || (text == grammar.contributor_separator
                && paragraph.runs.last().is_some_and(|run| {
                    run.text().ends_with(')')
                        && run.properties.as_ref().is_some_and(|actual| {
                            actual.rtl == Some(true) && actual.language_bidi.is_none()
                        })
                }));
        for (offset, character) in text.char_indices() {
            let mut actual = properties.clone();
            if (first_arabic.is_some() && offset >= label_start || continuation)
                && !(trailing_glue && offset + character.len_utf8() == text.len())
            {
                actual.font_cs = None;
                actual.language_bidi = None;
            }
            append_apa_semantic_text(paragraph, &character.to_string(), actual)?;
        }
        return Ok(());
    }
    if let Some(font) = grammar.literal_font {
        // Keep date-unit literals separate from numeric and source operands.
        for character in text.chars() {
            let mut actual = properties.clone();
            // Own Japanese/Korean operands distinguish CJK and fullwidth comma
            // from Hangul. Keep that measured EastAsia font boundary separate from
            // the locale's generated-label font and from ASCII punctuation.
            let character_font = if matches!(
                grammar.language_east_asia,
                Some("ii-CN" | "zh-CN" | "zh-SG")
            ) && "专员处导录检电编访译辑页汉".contains(character)
            {
                Some("Microsoft JhengHei")
            } else if grammar.language_east_asia == Some("ii-CN")
                && ('\u{a000}'..='\u{a48f}').contains(&character)
            {
                Some("Microsoft Yi Baiti")
            } else if (grammar.language_east_asia.is_some() && character == '\u{ff0c}')
                || (matches!(grammar.language_east_asia, Some("ja-JP" | "ko-KR"))
                    && ('\u{3040}'..='\u{9fff}').contains(&character))
            {
                Some("MS Gothic")
            } else if grammar
                .literal_script
                .is_some_and(|(first, last)| (first..=last).contains(&character))
            {
                Some(font)
            } else {
                None
            };
            if let Some(font) = character_font {
                actual.font_ascii = Some(font.to_owned());
                actual.font_hansi = Some(font.to_owned());
                actual.font_cs = Some(font.to_owned());
                actual.font_east_asia = grammar.language_east_asia.map(|_| font.to_owned());
            }
            append_apa_semantic_text(paragraph, &character.to_string(), actual)?;
        }
        return Ok(());
    }
    if let Some(font) = grammar.numeric_cs_font {
        let (first_script, last_script) = grammar.literal_script.unwrap_or(('\u{900}', '\u{97f}'));
        // These measured ASCII and own-script operations are not a full script algorithm.
        // Source spans and generated glue enter separately before text is joined.
        if !text.chars().all(|character| {
            character.is_ascii()
                || matches!(character, '\u{a0}' | '\u{600}'..='\u{6ff}')
                || (first_script..=last_script).contains(&character)
                || (grammar.language_bidi == Some("ml-IN")
                    && matches!(character, '\u{200c}' | '\u{200d}'))
        }) {
            return Err(not_covered("this complex-script source text"));
        }
        // Preserve source XML whitespace. This operation normalizes only emitted cache text.
        let normalized = text.replace(['\t', '\n', '\r'], " ");
        let text = if rtl {
            if paragraph
                .runs
                .last()
                .is_some_and(|run| run.text().ends_with(' '))
            {
                normalized.strip_prefix(' ').unwrap_or(&normalized)
            } else {
                &normalized
            }
        } else {
            normalized.trim_start_matches(' ')
        };
        let mut numeric = rtl
            && paragraph
                .runs
                .last()
                .and_then(|run| run.properties.as_ref())
                .is_some_and(|properties| properties.complex_script == Some(true));
        let mut arabic = false;
        let mut after_comma = false;
        for (offset, character) in text.char_indices() {
            if matches!(character, '\u{600}'..='\u{6ff}') {
                arabic = true;
                numeric = false;
                after_comma = false;
            } else if (first_script..=last_script).contains(&character) {
                arabic = false;
                numeric = true;
                after_comma = false;
            } else if character.is_ascii_digit() {
                numeric = !arabic;
                after_comma = false;
            } else if matches!(character, ',' | ';' | '{' | '}' | '\u{a0}') {
                arabic = false;
                numeric = false;
                after_comma = true;
            } else if character.is_ascii_alphabetic() || character == '&' {
                arabic = false;
                numeric = false;
                after_comma = false;
            } else if matches!(character, '+' | '-') {
                arabic = false;
                numeric = true;
                after_comma = false;
            } else if offset == 0 && matches!(character, '(' | '[') {
                numeric = text[character.len_utf8()..]
                    .chars()
                    .find(|next| *next != ' ')
                    .is_some_and(|next| next.is_ascii_digit());
            } else if arabic && character == ' ' {
                if text[offset + 1..]
                    .chars()
                    .find(|next| *next != ' ')
                    .is_some_and(|next| next.is_ascii_alphabetic())
                {
                    arabic = false;
                    numeric = false;
                }
            } else if rtl && character == ' ' {
                numeric = !after_comma
                    || text[offset + 1..].chars().find(|next| *next != ' ') == Some('&');
            } else if !matches!(
                character,
                ' ' | '(' | ')' | '[' | ']' | '.' | ':' | '/' | '-'
            ) && !(grammar.language_bidi == Some("ml-IN")
                && matches!(character, '\u{200c}' | '\u{200d}'))
            {
                return Err(not_covered("this complex-script punctuation"));
            }
            let mut actual = properties.clone();
            if arabic {
                actual.language = None;
                actual.language_bidi = None;
                actual.rtl = Some(true);
            } else if numeric {
                actual.font_cs = Some(font.to_owned());
                actual.complex_script = Some(true);
            }
            // Own source and generated Va controls share this exact measured grapheme.
            // This does not classify font coverage for other Odia operands.
            if grammar.language_bidi == Some("or-IN")
                && ((character == '\u{b35}' && text[offset..].starts_with("ଵା"))
                    || (character == '\u{b3e}' && text[..offset].ends_with('\u{b35}')))
            {
                actual.font_ascii = Some("Noto Sans Oriya".to_owned());
                actual.font_hansi = Some("Noto Sans Oriya".to_owned());
                actual.font_cs = Some("Noto Sans Oriya".to_owned());
                actual.complex_script = Some(true);
            }
            append_apa_semantic_text(paragraph, &character.to_string(), actual)?;
        }
        Ok(())
    } else {
        append_apa_semantic_text(paragraph, text, properties)
    }
}

fn append_apa_semantic_text(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    text: &str,
    properties: oxoffice_docx_oxml::CT_RPr,
) -> Result<()> {
    use oxoffice_docx_oxml::text::{CT_R, RunContent};
    if let Some(last) = paragraph.runs.last_mut()
        && last.properties.as_ref() == Some(&properties)
    {
        let [RunContent::Text(content)] = last.content.as_mut_slice() else {
            return Err(bibliography_error("invalid semantic bibliography text run"));
        };
        content.text.push_str(text);
        content.preserve_space = content.text.starts_with(' ') || content.text.ends_with(' ');
    } else {
        let mut run = CT_R::new(text);
        run.properties = Some(properties);
        paragraph.runs.push(run);
    }
    Ok(())
}

pub(super) fn append_apa_semantic_secondary(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<()> {
    let secondary = [
        (BibliographyContributorRole::Editor, grammar.editor),
        (BibliographyContributorRole::Translator, grammar.translator),
    ]
    .into_iter()
    .map(|(role, label)| (apa_bibliography_names(source, role, false, grammar), label))
    .filter(|(name, _)| !name.is_empty())
    .collect::<Vec<_>>();
    if !secondary.is_empty() {
        append_apa_semantic_run(paragraph, grammar, " (", false, true)?;
        for (index, (name, label)) in secondary.iter().enumerate() {
            if grammar.rtl_labels {
                append_apa_semantic_run(paragraph, grammar, name, false, false)?;
                append_apa_semantic_run(
                    paragraph,
                    grammar,
                    &format!("{}{label}", grammar.contributor_separator),
                    false,
                    true,
                )?;
                if index + 1 < secondary.len() {
                    append_apa_semantic_run(
                        paragraph,
                        grammar,
                        &format!(
                            "{}{}{}",
                            grammar.contributor_separator,
                            grammar.conjunction,
                            if grammar.conjunction.is_empty() {
                                ""
                            } else {
                                " "
                            }
                        ),
                        false,
                        true,
                    )?;
                }
                continue;
            }
            let (label_text, terminal) = label
                .strip_suffix('.')
                .map_or((*label, ""), |text| (text, "."));
            append_apa_semantic_run(
                paragraph,
                grammar,
                &format!("{name}, {label_text}"),
                false,
                false,
            )?;
            append_apa_semantic_run(
                paragraph,
                grammar,
                terminal,
                false,
                grammar.numeric_cs_font.is_none()
                    || label_text.chars().any(|character| {
                        let (first, last) =
                            grammar.literal_script.unwrap_or(('\u{900}', '\u{97f}'));
                        (first..=last).contains(&character)
                    }),
            )?;
            if index + 1 < secondary.len() {
                if grammar.numeric_cs_font.is_some() {
                    append_apa_semantic_run(
                        paragraph,
                        grammar,
                        grammar.contributor_separator.trim_end(),
                        false,
                        false,
                    )?;
                    append_apa_semantic_run(paragraph, grammar, " ", false, true)?;
                    append_apa_semantic_run(paragraph, grammar, grammar.conjunction, false, false)?;
                    append_apa_semantic_run(paragraph, grammar, " ", false, true)?;
                } else {
                    append_apa_semantic_run(
                        paragraph,
                        grammar,
                        &format!(
                            "{}{}{}",
                            grammar.contributor_separator,
                            grammar.conjunction,
                            if grammar.conjunction.is_empty() {
                                ""
                            } else {
                                " "
                            }
                        ),
                        false,
                        true,
                    )?;
                }
            }
        }
        append_apa_semantic_run(
            paragraph,
            grammar,
            ")",
            false,
            grammar.numeric_cs_font.is_none()
                || secondary.last().is_some_and(|(_, label)| {
                    label.chars().any(|character| {
                        let (first, last) =
                            grammar.literal_script.unwrap_or(('\u{900}', '\u{97f}'));
                        (first..=last).contains(&character)
                    })
                }),
        )?;
    }
    Ok(())
}

pub(super) fn append_apa_semantic_title(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    grammar: &ApaBibliographyLanguage,
    title: &str,
    italic: bool,
    quoted: bool,
    terminal: bool,
) -> Result<()> {
    if title.is_empty() {
        return Ok(());
    }
    if quoted {
        append_apa_semantic_run(paragraph, grammar, "“", italic, true)?;
    }
    append_apa_semantic_run(paragraph, grammar, title, italic, false)?;
    if quoted {
        append_apa_semantic_run(
            paragraph,
            grammar,
            if terminal { ".”" } else { "”" },
            italic,
            true,
        )?;
    }
    Ok(())
}

pub(super) fn apa_semantic_creator_paragraph(
    source: &BibliographySource,
    role: BibliographyContributorRole,
    grammar: &ApaBibliographyLanguage,
    title_italic: bool,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    use oxoffice_docx_oxml::text::CT_P;
    let mut paragraph = CT_P::new();
    let mut properties = bibliography_paragraph_properties(BibliographyStyle::ApaSixthEdition);
    properties.bidi = grammar.semantic_rtl.then_some(true);
    if let Some(mark) = properties.rpr.as_mut() {
        mark.language = grammar.language.map(str::to_owned);
        mark.language_bidi = grammar.language_bidi.map(str::to_owned);
        mark.language_east_asia = grammar.language_east_asia.map(str::to_owned);
    }
    paragraph.properties = Some(properties);
    let author = apa_bibliography_names(source, role, true, grammar);
    let date = apa_bibliography_date(source, grammar);
    let title = if source.kind == BibliographySourceKind::Patent {
        ""
    } else {
        first_source_property(source, BibliographySourceField::Title)
    };
    // The final initial mark is generated name grammar. Internal source punctuation
    // remains inside the author operand, including the measured BN discriminator.
    let generated_initial = grammar.numeric_cs_font.is_none()
        && !grammar.full_names
        && !grammar.joined_names
        && source.contributors.iter().any(|contributor| {
            contributor.role == role
                && matches!(&contributor.value, BibliographyAuthor::People(people)
                if people.last().is_some_and(|person| !person.last.is_empty()
                    && (!person.first.is_empty() || !person.middle.is_empty())))
        });
    let author_text = if generated_initial {
        author.strip_suffix(grammar.initial_mark).unwrap_or(&author)
    } else {
        &author
    };
    append_apa_semantic_run(&mut paragraph, grammar, author_text, false, false)?;
    append_apa_semantic_run(
        &mut paragraph,
        grammar,
        &format!(
            "{} ({date}).{}",
            if generated_initial {
                if grammar.initial_mark == '\'' && role != BibliographyContributorRole::Composer {
                    "'.".to_owned()
                } else {
                    grammar.initial_mark.to_string()
                }
            } else if (grammar.full_names || grammar.joined_names)
                && role != BibliographyContributorRole::Composer
            {
                ".".to_owned()
            } else {
                String::new()
            },
            if title.is_empty() { "" } else { " " }
        ),
        false,
        true,
    )?;
    append_apa_semantic_title(
        &mut paragraph,
        grammar,
        title,
        title_italic,
        grammar.curly_titles
            && matches!(
                source.kind,
                BibliographySourceKind::Book
                    | BibliographySourceKind::Report
                    | BibliographySourceKind::DocumentFromInternetSite
                    | BibliographySourceKind::Performance
                    | BibliographySourceKind::InternetSite
            ),
        matches!(
            source.kind,
            BibliographySourceKind::Report
                | BibliographySourceKind::DocumentFromInternetSite
                | BibliographySourceKind::Performance
        ),
    )?;
    Ok(paragraph)
}

pub(super) fn append_apa_semantic_retrieval(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<()> {
    let retrieval = apa_bibliography_retrieval(source, grammar);
    if grammar.rtl_labels {
        let url = first_source_property(source, BibliographySourceField::Url);
        let site = first_source_property(source, BibliographySourceField::InternetSiteTitle);
        let operand = if site.is_empty() {
            url.to_owned()
        } else if url.is_empty() {
            format!("{site}.")
        } else {
            format!("{site}: {url}")
        };
        if !operand.is_empty() && grammar.retrieval_date_after {
            // The lexical consumer already assembles the source-valued date.
            // Match its exact generated prefix and operand, never a substring
            // that could also occur inside a source URL or site title.
            let no_date = first_source_property(source, BibliographySourceField::MonthAccessed)
                .is_empty()
                && first_source_property(source, BibliographySourceField::YearAccessed).is_empty();
            let prefix = format!(
                "{}{}",
                if no_date {
                    grammar.retrieved_without_date.unwrap_or(grammar.retrieved)
                } else {
                    grammar.retrieved
                },
                if no_date {
                    grammar.from_without_date
                } else {
                    grammar.from_with_date
                }
            );
            let suffix = retrieval
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_prefix(&operand))
                .ok_or_else(|| bibliography_error("invalid RTL post-URL retrieval operand"))?;
            append_apa_semantic_run(paragraph, grammar, &prefix, false, true)?;
            append_apa_semantic_run(paragraph, grammar, &operand, false, false)?;
            append_apa_semantic_run(paragraph, grammar, suffix, false, true)?;
        } else if !operand.is_empty() {
            let before_suffix = retrieval
                .strip_suffix(grammar.retrieval_suffix)
                .ok_or_else(|| bibliography_error("invalid RTL retrieval suffix"))?;
            let literal = before_suffix
                .strip_suffix(&operand)
                .ok_or_else(|| bibliography_error("invalid RTL retrieval operand"))?;
            append_apa_semantic_run(paragraph, grammar, literal, false, true)?;
            append_apa_semantic_run(paragraph, grammar, &operand, false, false)?;
            append_apa_semantic_run(paragraph, grammar, grammar.retrieval_suffix, false, true)?;
        }
    } else if let Some(retrieval) = retrieval.strip_prefix(' ') {
        append_apa_semantic_run(paragraph, grammar, " ", false, true)?;
        append_apa_semantic_run(paragraph, grammar, retrieval, false, false)?;
    } else {
        append_apa_semantic_run(paragraph, grammar, &retrieval, false, false)?;
    }
    Ok(())
}

pub(super) fn append_apa_semantic_source_list(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    grammar: &ApaBibliographyLanguage,
    operands: &[&str],
) -> Result<()> {
    for (index, operand) in operands.iter().filter(|text| !text.is_empty()).enumerate() {
        if index != 0 {
            append_apa_semantic_run(
                paragraph,
                grammar,
                grammar.location_separator,
                false,
                grammar.rtl_labels
                    && !matches!(
                        grammar.language_bidi,
                        Some(
                            "ug-CN"
                                | "ur-PK"
                                | "ur-IN"
                                | "he-IL"
                                | "fa-IR"
                                | "sd-Arab-PK"
                                | "pa-Arab-PK"
                                | "ps-AF"
                        )
                    ),
            )?;
        }
        append_apa_semantic_run(paragraph, grammar, operand, false, false)?;
    }
    Ok(())
}

pub(super) fn append_apa_semantic_labeled_roles(
    paragraph: &mut oxoffice_docx_oxml::text::CT_P,
    grammar: &ApaBibliographyLanguage,
    roles: &[(String, &str)],
) -> Result<()> {
    if roles.is_empty() {
        return Ok(());
    }
    append_apa_semantic_run(paragraph, grammar, " (", false, true)?;
    for (index, (name, label)) in roles.iter().enumerate() {
        append_apa_semantic_run(paragraph, grammar, name, false, false)?;
        if !label.is_empty() {
            append_apa_semantic_run(
                paragraph,
                grammar,
                &format!("{}{label}", grammar.contributor_separator),
                false,
                true,
            )?;
        }
        if index + 1 != roles.len() {
            append_apa_semantic_run(
                paragraph,
                grammar,
                &format!(
                    "{}{}",
                    grammar.contributor_separator,
                    if index + 2 == roles.len() {
                        format!(
                            "{}{}",
                            grammar.conjunction,
                            if grammar.conjunction.is_empty() {
                                ""
                            } else {
                                " "
                            }
                        )
                    } else {
                        String::new()
                    }
                ),
                false,
                true,
            )?;
        }
    }
    append_apa_semantic_run(paragraph, grammar, ")", false, true)
}

fn apa_authored_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> oxoffice_docx_oxml::text::CT_P {
    use oxoffice_docx_oxml::text::CT_P;
    let mut author_label =
        apa_bibliography_names(source, BibliographyContributorRole::Author, true, grammar);
    let editor_count = source
        .contributors
        .iter()
        .filter(|value| value.role == BibliographyContributorRole::Editor)
        .map(|value| match &value.value {
            BibliographyAuthor::People(people) => people.len(),
            BibliographyAuthor::Corporate(_) => 1,
        })
        .sum::<usize>();
    let editor_label = if editor_count > 1 {
        grammar.editors
    } else {
        grammar.editor
    };
    let editor_fallback = source.kind != BibliographySourceKind::Report
        && author_label.is_empty()
        && !apa_bibliography_names(source, BibliographyContributorRole::Editor, true, grammar)
            .is_empty();
    if editor_fallback {
        author_label = format!(
            "{} ({editor_label}).",
            apa_bibliography_names(source, BibliographyContributorRole::Editor, true, grammar)
        );
    }
    let title = first_source_property(source, BibliographySourceField::Title);
    let date = apa_bibliography_date(source, grammar);
    let year = date.as_str();

    let edition = first_source_property(source, BibliographySourceField::Edition);
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let mut parenthetical = Vec::new();
    if source.kind == BibliographySourceKind::Book && !edition.is_empty() {
        parenthetical.push(apa_bibliography_edition(edition, grammar));
    }
    if source.kind == BibliographySourceKind::Book && !volume.is_empty() {
        parenthetical.push(apa_bibliography_volume(volume, grammar));
    }
    let title_display = if parenthetical.is_empty() {
        format!("{title}.")
    } else {
        title.to_owned()
    };
    let mut paragraph = CT_P::new();
    paragraph.properties = Some(bibliography_paragraph_properties(
        BibliographyStyle::ApaSixthEdition,
    ));
    let mut publication = if parenthetical.is_empty() {
        String::new()
    } else {
        format!(" ({}).", parenthetical.join(", "))
    };
    if author_label.is_empty() && source.kind == BibliographySourceKind::Report {
        paragraph.runs.extend(bibliography_display_runs(
            &format!("({year}).{}", if title.is_empty() { "" } else { " " }),
            false,
        ));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&title_display, true));
        }
    } else if author_label.is_empty() {
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&title_display, true));
            publication.push_str(&format!(" ({year})."));
        } else {
            publication.insert_str(0, &format!("({year})."));
        }
    } else {
        paragraph.runs.extend(bibliography_display_runs(
            &format!(
                "{author_label} ({year}).{}",
                if title.is_empty() { "" } else { " " }
            ),
            false,
        ));
        if !title.is_empty() {
            paragraph
                .runs
                .extend(bibliography_display_runs(&title_display, true));
        }
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .filter(|field| {
        source.kind != BibliographySourceKind::Report || *field == BibliographySourceField::City
    })
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let publisher = first_source_property(source, BibliographySourceField::Publisher);

    if source.kind != BibliographySourceKind::Report {
        publication.push_str(&apa_bibliography_secondary(
            source,
            editor_fallback,
            grammar,
        ));
    }
    if source.kind == BibliographySourceKind::Report {
        let institution = [
            BibliographySourceField::Institution,
            BibliographySourceField::Department,
        ]
        .into_iter()
        .map(|field| first_source_property(source, field))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        if !institution.is_empty() {
            publication.push_str(&format!(" {institution}."));
        } else if publisher.is_empty() {
            let thesis = first_source_property(source, BibliographySourceField::ThesisType);
            if !thesis.is_empty() {
                publication.push_str(&format!(" {thesis}"));
                if !location.is_empty() {
                    publication.push_str(&format!(", {location}"));
                }
                publication.push('.');
            }
        }
    }
    let report_thesis = source.kind == BibliographySourceKind::Report
        && publisher.is_empty()
        && !first_source_property(source, BibliographySourceField::ThesisType).is_empty()
        && first_source_property(source, BibliographySourceField::Institution).is_empty()
        && first_source_property(source, BibliographySourceField::Department).is_empty();
    if !report_thesis
        && matches!(
            source.kind,
            BibliographySourceKind::Book | BibliographySourceKind::Report
        )
        && (!location.is_empty()
            || !publisher.is_empty()
            || (!title.is_empty()
                && (!author_label.is_empty() || source.kind == BibliographySourceKind::Report)))
    {
        publication.push(' ');
        publication.push_str(&location);
        if !location.is_empty() && !publisher.is_empty() {
            publication.push_str(": ");
        }
        publication.push_str(publisher);
        if !location.is_empty() || !publisher.is_empty() {
            publication.push('.');
        }
    }
    let retrieval = apa_bibliography_retrieval(source, grammar);
    if source.kind == BibliographySourceKind::DocumentFromInternetSite
        && !author_label.is_empty()
        && !title.is_empty()
        && publication.is_empty()
        && retrieval.is_empty()
    {
        publication.push(' ');
    }
    publication.push_str(&retrieval);
    if !publication.is_empty() {
        paragraph
            .runs
            .extend(bibliography_display_runs(&publication, false));
    }
    paragraph
}
