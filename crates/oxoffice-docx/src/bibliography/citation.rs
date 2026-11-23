//! Citation creators and runs, bibliography sort keys and the IEEE table.

use super::*;

fn citation_creator_role(
    source: &BibliographySource,
    style: BibliographyStyle,
) -> Option<BibliographyContributorRole> {
    use BibliographyContributorRole as Role;
    use BibliographySourceKind as Kind;
    let roles: &[Role] = match (style, source.kind) {
        (BibliographyStyle::Gb7714, Kind::SoundRecording) => &[Role::Artist],
        (BibliographyStyle::Gb7714, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::Gb7714, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::Gb7714, Kind::Film) => &[Role::Director],
        (BibliographyStyle::Gb7714, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::Gb7714, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::GostName, Kind::SoundRecording) => &[Role::Artist],
        (BibliographyStyle::GostName, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::GostName, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::GostName, Kind::Film) => &[Role::Director],
        (BibliographyStyle::GostName, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::GostName, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::GostTitle, Kind::SoundRecording) => &[Role::Artist],
        (BibliographyStyle::GostTitle, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::GostTitle, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::GostTitle, Kind::Film) => &[Role::Director],
        (BibliographyStyle::GostTitle, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::GostTitle, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::HarvardAnglia, Kind::SoundRecording) => &[Role::Composer],
        (BibliographyStyle::HarvardAnglia, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::HarvardAnglia, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::HarvardAnglia, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::HarvardAnglia, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::MlaSeventhEdition, Kind::SoundRecording) => &[Role::Performer],
        (BibliographyStyle::MlaSeventhEdition, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::MlaSeventhEdition, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::MlaSeventhEdition, Kind::Film) => &[Role::Writer],
        (BibliographyStyle::MlaSeventhEdition, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::MlaSeventhEdition, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::Turabian, Kind::SoundRecording) => &[Role::Performer],
        (BibliographyStyle::Turabian, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::Turabian, Kind::Art) => &[Role::Artist],
        (BibliographyStyle::Turabian, Kind::Film) => &[Role::Writer],
        (BibliographyStyle::Turabian, Kind::Interview) => &[Role::Interviewee],
        (BibliographyStyle::Turabian, Kind::Patent) => &[Role::Inventor],
        (BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Sist02, Kind::SoundRecording) => {
            &[Role::Artist]
        }
        (BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Sist02, Kind::Film) => {
            &[Role::Director]
        }
        (BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Sist02, Kind::Performance) => {
            &[Role::Writer]
        }
        (
            BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Sist02,
            Kind::JournalArticle | Kind::ConferenceProceedings,
        ) => &[],
        (BibliographyStyle::Chicago, Kind::SoundRecording) => &[Role::Performer],
        (BibliographyStyle::Chicago, Kind::Film) => &[Role::Writer],
        (BibliographyStyle::Chicago, Kind::Case) => &[Role::Author],
        (BibliographyStyle::Chicago, Kind::Performance) => &[Role::Writer],
        (BibliographyStyle::ApaSixthEdition, Kind::SoundRecording) => {
            &[Role::Composer, Role::Conductor]
        }
        (BibliographyStyle::ApaSixthEdition, Kind::Film) => {
            &[Role::ProducerName, Role::Director, Role::Writer]
        }
        (BibliographyStyle::ApaSixthEdition, Kind::Case) => &[],
        (BibliographyStyle::ApaSixthEdition, Kind::Book) => &[Role::Author, Role::Editor],
        (_, Kind::Performance) => &[Role::Writer, Role::Performer],
        (_, Kind::Art) => &[Role::Artist],
        (_, Kind::Interview) => &[Role::Interviewee],
        (_, Kind::Patent) => &[Role::Inventor],
        _ => &[Role::Author],
    };
    roles.iter().copied().find(|role| {
        source
            .contributors
            .iter()
            .any(|contributor| contributor.role == *role)
    })
}

pub(super) fn single_citation_creator(
    source: &BibliographySource,
    style: BibliographyStyle,
) -> Option<&BibliographyPerson> {
    let role = citation_creator_role(source, style)?;
    let mut people = source
        .contributors
        .iter()
        .filter(|contributor| contributor.role == role)
        .flat_map(|contributor| match &contributor.value {
            BibliographyAuthor::People(people) => people.as_slice(),
            _ => &[],
        });
    let person = people.next()?;
    people.next().is_none().then_some(person)
}

pub(super) fn has_equivalent_creator(
    collection: &BibliographyCollection,
    index: usize,
    creator: &BibliographyPerson,
    style: BibliographyStyle,
) -> Result<bool> {
    for (other_index, other_identity) in &collection.sources {
        if *other_index == index {
            continue;
        }
        if let Some(other) = collection.parsed.supported_source(
            *other_index,
            &other_identity.tag,
            other_identity.guid.as_deref(),
            other_identity.source_type.as_deref(),
            false,
        )? && single_citation_creator(&other, style).is_some_and(|person| {
            person.last == creator.last
                && person.first == creator.first
                && person.middle == creator.middle
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn native_citation_run_properties(profile: usize) -> Result<oxoffice_docx_oxml::CT_RPr> {
    use oxoffice_docx_oxml::CT_RPr;
    Ok(match profile {
        0 => CT_RPr {
            no_proof: Some(true),
            ..Default::default()
        },
        1 => CT_RPr {
            no_proof: Some(true),
            font_hint: Some("cs".into()),
            rtl: Some(true),
            ..Default::default()
        },
        2 => CT_RPr {
            no_proof: Some(true),
            font_hint: Some("cs".into()),
            ..Default::default()
        },
        3 => CT_RPr {
            no_proof: Some(true),
            font_hint: Some("eastAsia".into()),
            ..Default::default()
        },
        4 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("MS Gothic".into()),
            font_east_asia: Some("MS Gothic".into()),
            font_hansi: Some("MS Gothic".into()),
            font_cs: Some("MS Gothic".into()),
            font_hint: Some("eastAsia".into()),
            ..Default::default()
        },
        5 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Malgun Gothic".into()),
            font_east_asia: Some("Malgun Gothic".into()),
            font_hansi: Some("Malgun Gothic".into()),
            font_cs: Some("Malgun Gothic".into()),
            font_hint: Some("eastAsia".into()),
            ..Default::default()
        },
        6 => CT_RPr {
            no_proof: Some(true),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        7 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Angsana New".into()),
            font_hansi: Some("Angsana New".into()),
            font_cs: Some("Angsana New".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        8 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Tahoma".into()),
            font_hansi: Some("Tahoma".into()),
            font_cs: Some("Tahoma".into()),
            ..Default::default()
        },
        9 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Helvetica".into()),
            font_hansi: Some("Helvetica".into()),
            font_cs: Some("Helvetica".into()),
            ..Default::default()
        },
        10 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Kohinoor Devanagari".into()),
            font_hansi: Some("Kohinoor Devanagari".into()),
            font_cs: Some("Kohinoor Devanagari".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        11 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Kohinoor Bangla".into()),
            font_hansi: Some("Kohinoor Bangla".into()),
            font_cs: Some("Kohinoor Bangla".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        12 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Gurmukhi MN".into()),
            font_hansi: Some("Gurmukhi MN".into()),
            font_cs: Some("Gurmukhi MN".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        13 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Gujarati Sangam MN".into()),
            font_hansi: Some("Gujarati Sangam MN".into()),
            font_cs: Some("Gujarati Sangam MN".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        14 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Noto Sans Oriya".into()),
            font_hansi: Some("Noto Sans Oriya".into()),
            font_cs: Some("Noto Sans Oriya".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        15 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Latha".into()),
            font_hansi: Some("Latha".into()),
            font_cs: Some("Latha".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        16 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Gautami".into()),
            font_hansi: Some("Gautami".into()),
            font_cs: Some("Gautami".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        17 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Tunga".into()),
            font_hansi: Some("Tunga".into()),
            font_cs: Some("Tunga".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        18 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Kartika".into()),
            font_hansi: Some("Kartika".into()),
            font_cs: Some("Kartika".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        19 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Microsoft JhengHei".into()),
            font_east_asia: Some("Microsoft JhengHei".into()),
            font_hansi: Some("Microsoft JhengHei".into()),
            font_cs: Some("Microsoft JhengHei".into()),
            font_hint: Some("eastAsia".into()),
            ..Default::default()
        },
        20 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Khmer Sangam MN".into()),
            font_hansi: Some("Khmer Sangam MN".into()),
            font_cs: Some("Khmer Sangam MN".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        21 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Lao Sangam MN".into()),
            font_hansi: Some("Lao Sangam MN".into()),
            font_cs: Some("Lao Sangam MN".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        22 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Sinhala Sangam MN".into()),
            font_hansi: Some("Sinhala Sangam MN".into()),
            font_cs: Some("Sinhala Sangam MN".into()),
            font_hint: Some("cs".into()),
            complex_script: Some(true),
            ..Default::default()
        },
        23 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Plantagenet Cherokee".into()),
            font_hansi: Some("Plantagenet Cherokee".into()),
            font_cs: Some("Plantagenet Cherokee".into()),
            ..Default::default()
        },
        24 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("Nyala".into()),
            font_hansi: Some("Nyala".into()),
            font_cs: Some("Nyala".into()),
            ..Default::default()
        },
        25 => CT_RPr {
            no_proof: Some(true),
            font_ascii: Some("MS Mincho".into()),
            font_hansi: Some("MS Mincho".into()),
            font_east_asia: Some("MS Mincho".into()),
            font_cs: Some("MS Mincho".into()),
            font_hint: Some("eastAsia".into()),
            rtl: Some(true),
            ..Default::default()
        },
        _ => return Err(bibliography_error("invalid native citation run profile")),
    })
}

pub(super) fn source_citation_runs(
    source: &BibliographySource,
    creator: Option<&BibliographyPerson>,
    separate_leading: bool,
    grammar: &[NativeCitationRun],
) -> Result<Vec<oxoffice_docx_oxml::text::CT_R>> {
    use oxoffice_docx_oxml::text::CT_R;
    let country = first_source_property(source, BibliographySourceField::CountryRegion);
    let number = first_source_property(source, BibliographySourceField::PatentNumber);
    let year = first_source_property(source, BibliographySourceField::Year);
    let case_title = first_source_property(source, BibliographySourceField::ShortTitle);
    let source_title = first_source_property(source, BibliographySourceField::Title);
    let creator_last = creator
        .and_then(|person| person.last.first())
        .map_or("", String::as_str);
    let creator_first = creator
        .and_then(|person| person.first.first())
        .map_or("", String::as_str);
    let creator_middle = creator
        .and_then(|person| person.middle.first())
        .map_or("", String::as_str);
    if (grammar
        .iter()
        .any(|token| matches!(token, NativeCitationRun::SourceTitle { .. }))
        && (!source_title
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == ' ')
            || source_title.starts_with(' ')
            || source_title.ends_with(' ')))
        || (grammar
            .iter()
            .any(|token| matches!(token, NativeCitationRun::CaseTitle { .. }))
            && !case_title
                .chars()
                .all(|character| character.is_ascii_alphanumeric()))
        || (grammar.iter().any(|token| {
            matches!(
                token,
                NativeCitationRun::Country(_) | NativeCitationRun::Number { .. }
            )
        }) && (!country
            .chars()
            .all(|character| character.is_ascii_alphabetic() || character == ' ')
            || !number
                .chars()
                .all(|character| character.is_ascii_alphanumeric())))
        || (grammar
            .iter()
            .any(|token| matches!(token, NativeCitationRun::CreatorLast { .. }))
            && !creator_last
                .chars()
                .all(|character| character.is_ascii_alphanumeric()))
        || (grammar
            .iter()
            .any(|token| matches!(token, NativeCitationRun::CreatorFirst { .. }))
            && !creator_first
                .chars()
                .all(|character| character.is_ascii_alphanumeric()))
        || (grammar
            .iter()
            .any(|token| matches!(token, NativeCitationRun::CreatorMiddle { .. }))
            && (creator.is_some_and(|person| person.middle.len() > 1)
                || !creator_middle
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())))
        || (grammar
            .iter()
            .any(|token| matches!(token, NativeCitationRun::Year(_)))
            && !year.chars().all(|character| character.is_ascii_digit()))
    {
        return Err(not_covered(
            "a citation whose names or year are not in Latin letters and digits",
        ));
    }
    let mut leading = CT_R::new(" ");
    leading.properties = Some(native_citation_run_properties(0)?);
    let mut body: Vec<CT_R> = Vec::new();
    let mut append = |text: &str, profile| -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let properties = native_citation_run_properties(profile)?;
        if let Some(last) = body.last_mut()
            && last.properties.as_ref() == Some(&properties)
        {
            let Some(oxoffice_docx_oxml::text::RunContent::Text(content)) =
                last.content.first_mut()
            else {
                return Err(bibliography_error("invalid assembled citation run"));
            };
            content.text.push_str(text);
            content.preserve_space = content.text.starts_with(' ') || content.text.ends_with(' ');
        } else {
            let mut run = CT_R::new(text);
            run.properties = Some(properties);
            body.push(run);
        }
        Ok(())
    };
    for token in grammar {
        match token {
            NativeCitationRun::Literal(text, profile) => append(text, *profile)?,
            NativeCitationRun::Country(profile) => append(country, *profile)?,
            NativeCitationRun::Year(profile) => append(year, *profile)?,
            NativeCitationRun::CaseTitle { letters, digits }
            | NativeCitationRun::CreatorLast { letters, digits } => {
                let value = match token {
                    NativeCitationRun::CreatorLast { .. } => creator_last,
                    _ => case_title,
                };
                for character in value.chars() {
                    append(
                        &character.to_string(),
                        if character.is_ascii_digit() {
                            *digits
                        } else {
                            *letters
                        },
                    )?;
                }
            }
            NativeCitationRun::CreatorFirst { letters, digits }
            | NativeCitationRun::CreatorMiddle { letters, digits } => {
                let value = if matches!(token, NativeCitationRun::CreatorFirst { .. }) {
                    creator_first
                } else {
                    creator_middle
                };
                for character in value.chars() {
                    append(
                        &character.to_string(),
                        if character.is_ascii_digit() {
                            *digits
                        } else {
                            *letters
                        },
                    )?;
                }
            }
            NativeCitationRun::CreatorSpace {
                middle,
                letters,
                digits,
            } => {
                let component = if *middle {
                    creator_middle
                } else {
                    creator_first
                };
                append(
                    " ",
                    if component.ends_with(|character: char| character.is_ascii_digit()) {
                        *digits
                    } else {
                        *letters
                    },
                )?;
            }
            NativeCitationRun::SourceTitle {
                letters,
                digits,
                spaces_after_letters,
                spaces_after_digits,
            } => {
                let mut after_digit = false;
                for character in source_title.chars() {
                    let profile = if character.is_ascii_digit() {
                        *digits
                    } else if character == ' ' {
                        if after_digit {
                            *spaces_after_digits
                        } else {
                            *spaces_after_letters
                        }
                    } else {
                        *letters
                    };
                    append(&character.to_string(), profile)?;
                    if character != ' ' {
                        after_digit = character.is_ascii_digit();
                    }
                }
            }
            NativeCitationRun::Number { letters, digits } => {
                for character in number.chars() {
                    append(
                        &character.to_string(),
                        if character.is_ascii_digit() {
                            *digits
                        } else {
                            *letters
                        },
                    )?;
                }
            }
        }
    }
    if !separate_leading {
        let first = body
            .first_mut()
            .ok_or_else(|| bibliography_error("empty assembled citation"))?;
        if first.properties != leading.properties {
            return Err(bibliography_error(
                "inconsistent citation leading-run properties",
            ));
        }
        let Some(oxoffice_docx_oxml::text::RunContent::Text(content)) = first.content.first_mut()
        else {
            return Err(bibliography_error("invalid citation leading run"));
        };
        content.text.insert(0, ' ');
        content.preserve_space = true;
        return Ok(body);
    }
    let mut runs = vec![leading];
    runs.extend(body);
    Ok(runs)
}

// Literal labels and punctuation measured in the authenticated APA6 locale sweep.
// The caller supplies country, patent number and year from its source record.
pub(super) fn apa_plain_patent_punctuation(
    locale: &BibliographyFormattingLocale,
) -> Option<(&'static str, &'static str)> {
    use NativeCitationRun::{Country, Literal, Number, Year};
    match apa_patent_run_grammar(locale)?.1 {
        [
            Literal("(", 0),
            Country(0),
            Literal(label, 0),
            Number {
                letters: 0,
                digits: 0,
            },
            Literal(separator, 0),
            Year(0),
            Literal(")", 0),
        ] => Some((*label, *separator)),
        _ => None,
    }
}

pub(super) fn apa_citation_member(
    source: &BibliographySource,
    options: &CitationSourceOptions,
    patent_punctuation: Option<(&str, &str)>,
) -> String {
    use BibliographySourceKind as Kind;
    let role = citation_creator_role(source, BibliographyStyle::ApaSixthEdition);
    let mut names = source
        .contributors
        .iter()
        .filter(|contributor| role.is_some_and(|role| contributor.role == role))
        .flat_map(|author| match &author.value {
            BibliographyAuthor::Corporate(value) => vec![value.clone()],
            BibliographyAuthor::People(people) => people
                .iter()
                .filter_map(|person| {
                    person
                        .last
                        .first()
                        .or_else(|| person.first.first())
                        .filter(|name| !name.is_empty())
                        .cloned()
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let country = first_source_property(source, BibliographySourceField::CountryRegion);
    let patent = first_source_property(source, BibliographySourceField::PatentNumber);
    let patent_identity = source.kind == Kind::Patent && !country.is_empty() && !patent.is_empty();
    if patent_identity {
        let label = patent_punctuation.map_or(" Patent No. ", |(label, _)| label);
        names = vec![format!("{country}{label}{patent}")];
    }
    let title = first_source_property(source, BibliographySourceField::Title);
    let short_title = first_source_property(source, BibliographySourceField::ShortTitle);
    let year = first_source_property(source, BibliographySourceField::Year);
    let mut result = options.prefix.clone().unwrap_or_default();
    if options.suppress_author || names.is_empty() {
        let label = if short_title.is_empty() {
            title
        } else {
            short_title
        };
        result.push_str(if label.is_empty() && year.is_empty() {
            &source.tag
        } else {
            label
        });
    } else {
        match names.as_slice() {
            [name] => result.push_str(name),
            [left, right] => result.push_str(&format!("{left} & {right}")),
            names if names.len() <= 5 => {
                result.push_str(&names[..names.len() - 1].join(", "));
                result.push_str(&format!(
                    ", & {}",
                    names.last().expect("nonempty author list")
                ));
            }
            names => result.push_str(&format!("{}, et al.", names[0])),
        }
        if !patent_identity && !options.suppress_title && !short_title.is_empty() {
            result.push_str(&format!(", {short_title}"));
        } else if !options.suppress_year && year.is_empty() && !title.is_empty() {
            result.push_str(&format!(", {title}"));
        }
    }
    let displayed_year =
        if source.kind == Kind::InternetSite && year.is_empty() && !title.is_empty() {
            "n.d."
        } else {
            year
        };
    if !options.suppress_year && !displayed_year.is_empty() {
        if !result.is_empty() {
            result.push_str(if patent_identity {
                patent_punctuation.map_or(", ", |(_, separator)| separator)
            } else {
                ", "
            });
        }
        result.push_str(displayed_year);
    }
    if let Some(pages) = &options.pages
        && !pages.is_empty()
    {
        result.push_str(&format!(
            ", {} {pages}",
            if pages.contains(['-', '–']) {
                "pp."
            } else {
                "p."
            }
        ));
    }
    if let Some(volume) = &options.volume
        && !volume.is_empty()
    {
        result.push_str(&format!(", vol. {volume}"));
    }
    if let Some(suffix) = &options.suffix {
        result.push_str(suffix);
    }
    result
}

pub(super) type BibliographySortKey = (Vec<(String, String, String)>, String, String);

pub(super) fn bibliography_sort_key(
    source: &BibliographySource,
    style: BibliographyStyle,
    locale: &BibliographyFormattingLocale,
) -> Result<BibliographySortKey> {
    let full_names = style == BibliographyStyle::ApaSixthEdition
        && apa_bibliography_language(locale.clone())?.full_names;
    let mut people = Vec::new();
    let role = if style == BibliographyStyle::ApaSixthEdition {
        match source.kind {
            BibliographySourceKind::Patent => BibliographyContributorRole::Inventor,
            BibliographySourceKind::Art => BibliographyContributorRole::Artist,
            BibliographySourceKind::Interview => BibliographyContributorRole::Interviewee,
            BibliographySourceKind::Film => BibliographyContributorRole::ProducerName,
            BibliographySourceKind::SoundRecording => BibliographyContributorRole::Composer,
            BibliographySourceKind::Performance => BibliographyContributorRole::Writer,
            _ => BibliographyContributorRole::Author,
        }
    } else {
        BibliographyContributorRole::Author
    };
    for contributor in &source.contributors {
        if contributor.role != role {
            continue;
        }
        match &contributor.value {
            BibliographyAuthor::Corporate(value) => {
                people.push((value.to_lowercase(), String::new(), String::new()))
            }
            BibliographyAuthor::People(names) => {
                for person in names {
                    if full_names {
                        let name = [&person.first, &person.middle, &person.last]
                            .into_iter()
                            .flat_map(|parts| parts.iter())
                            .filter(|part| !part.is_empty())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" ");
                        people.push((name.to_lowercase(), String::new(), String::new()));
                    } else {
                        people.push((
                            person.last.join(" ").to_lowercase(),
                            person.first.join(" ").to_lowercase(),
                            person.middle.join(" ").to_lowercase(),
                        ));
                    }
                }
            }
        }
    }
    let title = first_source_property(source, BibliographySourceField::Title).to_lowercase();
    if people.is_empty()
        || style == BibliographyStyle::GostTitle
        || (style == BibliographyStyle::ApaSixthEdition
            && source.kind == BibliographySourceKind::Case)
    {
        people = vec![(title.clone(), String::new(), String::new())];
    }
    let year = first_source_property(source, BibliographySourceField::Year).to_owned();
    if !people
        .iter()
        .all(|(last, first, middle)| last.is_ascii() && first.is_ascii() && middle.is_ascii())
        || !year.is_ascii()
        || !title.is_ascii()
    {
        return Err(not_covered(
            "sorting a bibliography whose names or titles are not ASCII",
        ));
    }
    Ok((people, year, title))
}

pub(super) fn ieee_bibliography_table(
    entries: Vec<(u32, oxoffice_docx_oxml::text::CT_P)>,
    label_width: i32,
    text_width: i32,
) -> Result<Vec<oxoffice_docx_oxml::document::BodyContent>> {
    use oxoffice_docx_oxml::document::BodyContent;
    use oxoffice_docx_oxml::table::{
        CT_Row, CT_Tbl, CT_TblCellMar, CT_TblGrid, CT_TblGridCol, CT_TblLook, CT_TblPr,
        CT_TblWidth, CT_Tc, CT_TcPr, CT_TrPr, CellContent,
    };
    use oxoffice_docx_oxml::text::CT_P;
    use oxoffice_docx_oxml::units::Twips;
    let total = text_width
        .checked_add(90)
        .ok_or_else(|| bibliography_error("bibliography table width overflow"))?;
    let body_width = total
        .checked_sub(label_width)
        .filter(|width| *width > 0)
        .ok_or_else(|| bibliography_error("bibliography label exceeds the available text width"))?;
    let mut table = CT_Tbl::new();
    table.properties = Some(CT_TblPr {
        width: Some(CT_TblWidth::pct(5000)),
        cell_spacing: Some(CT_TblWidth::dxa(15)),
        cell_margin: Some(CT_TblCellMar {
            top: Some(Twips(15)),
            bottom: Some(Twips(15)),
            left: Some(Twips(15)),
            right: Some(Twips(15)),
        }),
        look: Some(CT_TblLook {
            val: Some("04A0".into()),
            first_row: Some(true),
            last_row: Some(false),
            first_column: Some(true),
            last_column: Some(false),
            no_h_band: Some(false),
            no_v_band: Some(true),
        }),
        ..Default::default()
    });
    table.grid = Some(CT_TblGrid {
        columns: vec![
            CT_TblGridCol {
                width: Twips(label_width),
            },
            CT_TblGridCol {
                width: Twips(body_width),
            },
        ],
        grid_change_xml: None,
        extra_xml: Vec::new(),
    });
    for (index, (number, mut reference)) in entries.into_iter().enumerate() {
        let mut label = CT_P::new();
        label.properties = Some(bibliography_paragraph_properties(BibliographyStyle::Ieee));
        label.runs.push(bibliography_display_run(
            &format!("[{number}] "),
            false,
            false,
        ));
        if index != 0
            && let Some(properties) = label.properties.as_mut()
            && let Some(properties) = properties.rpr.as_mut()
        {
            properties.sz = None;
            properties.sz_cs = None;
        }
        if let Some(properties) = reference.properties.as_mut()
            && let Some(properties) = properties.rpr.as_mut()
        {
            properties.sz = None;
            properties.sz_cs = None;
        }
        let mut row = CT_Row::new();
        row.properties = Some(CT_TrPr {
            cell_spacing: Some(CT_TblWidth::dxa(15)),
            ..Default::default()
        });
        for (paragraph, width) in [
            (label, CT_TblWidth::pct(50)),
            (reference, CT_TblWidth::auto()),
        ] {
            row.cells.push(CT_Tc {
                properties: Some(CT_TcPr {
                    width: Some(width),
                    extra_xml: vec![(12, b"<w:hideMark/>".to_vec())],
                    ..Default::default()
                }),
                content: vec![CellContent::Paragraph(paragraph)],
                extra_xml: Vec::new(),
            });
        }
        table.rows.push(row);
    }
    let mut first = CT_P::new();
    first.properties = Some(oxoffice_docx_oxml::CT_PPr {
        rpr: Some(oxoffice_docx_oxml::CT_RPr {
            no_proof: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut trailing = CT_P::new();
    trailing.properties = Some(oxoffice_docx_oxml::CT_PPr {
        rpr: Some(oxoffice_docx_oxml::CT_RPr {
            font_east_asia: Some("Times New Roman".into()),
            no_proof: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    });
    Ok(vec![
        BodyContent::Paragraph(Box::new(first)),
        BodyContent::Table(Box::new(table)),
        BodyContent::Paragraph(Box::new(trailing)),
    ])
}
