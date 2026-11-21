//! Bibliography entries rendered as paragraphs for right-to-left languages.

use super::*;

pub(super) fn apa_rtl_site_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph =
        apa_semantic_creator_paragraph(source, BibliographyContributorRole::Author, grammar, true)?;
    let title = first_source_property(source, BibliographySourceField::Title);
    let version = first_source_property(source, BibliographySourceField::Version);
    if !title.is_empty() {
        if !version.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{version}", grammar.contributor_separator),
                false,
                grammar.language_bidi != Some("ug-CN"),
            )?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
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
    let mut roles = Vec::new();
    if !editor.is_empty() {
        roles.push(format!("{editor}, {}", grammar.site_editor));
    }
    if !producer.is_empty() {
        roles.push(format!("{producer}, {}", grammar.producer));
    }
    if !company.is_empty() {
        roles.push(company.to_owned());
    }
    if grammar.rtl_labels {
        let mut labeled = Vec::new();
        if !editor.is_empty() {
            labeled.push((editor, grammar.site_editor));
        }
        if !producer.is_empty() {
            labeled.push((producer, grammar.producer));
        }
        if !company.is_empty() {
            labeled.push((company.to_owned(), ""));
        }
        append_apa_semantic_labeled_roles(&mut paragraph, grammar, &labeled)?;
    } else if !roles.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
        if roles.len() > 1 {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &roles[..roles.len() - 1].join(grammar.contributor_separator),
                false,
                false,
            )?;
            append_apa_semantic_run(
                &mut paragraph,
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
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &roles[roles.len() - 1],
            false,
            false,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_sound_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Composer,
        grammar,
        false,
    )?;
    let performer = apa_bibliography_names(
        source,
        BibliographyContributorRole::Performer,
        false,
        grammar,
    );
    let rtl_recorded = grammar.rtl_labels && grammar.language_bidi != Some("ps-AF");
    if !performer.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " [", false, true)?;
        if grammar.recorded_after {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.recorded_prefix,
                false,
                false,
            )?;
            append_apa_semantic_run(&mut paragraph, grammar, &performer, false, false)?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{}", grammar.recorded_separator, grammar.recorded_by),
                false,
                rtl_recorded || grammar.numeric_cs_font.is_some(),
            )?;
        } else {
            let recorded = if rtl_recorded {
                format!("{} ", grammar.recorded_by)
            } else {
                format!("{} {performer}", grammar.recorded_by)
            };
            append_apa_semantic_run(&mut paragraph, grammar, &recorded, false, rtl_recorded)?;
            if rtl_recorded {
                append_apa_semantic_run(&mut paragraph, grammar, &performer, false, false)?;
            }
        }
        append_apa_semantic_run(&mut paragraph, grammar, "].", false, true)?;
    }
    let album = first_source_property(source, BibliographySourceField::AlbumTitle);
    if !album.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        if !grammar.on_after && !grammar.on_label.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.on_label,
                false,
                grammar.rtl_labels,
            )?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.on_before_separator,
                false,
                true,
            )?;
        }
        if grammar.quote_album {
            append_apa_semantic_run(&mut paragraph, grammar, "“", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, album, true, false)?;
        if grammar.quote_album {
            append_apa_semantic_run(&mut paragraph, grammar, "”", false, false)?;
        }
        if grammar.on_after && !grammar.on_label.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{}", grammar.on_separator, grammar.on_label),
                false,
                grammar.rtl_labels || grammar.numeric_cs_font.is_some(),
            )?;
        }
    }
    let medium = first_source_property(source, BibliographySourceField::Medium);
    if !medium.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " [", false, true)?;
        append_apa_semantic_run(&mut paragraph, grammar, medium, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, "].", false, true)?;
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    let producer = apa_bibliography_names(
        source,
        BibliographyContributorRole::ProducerName,
        false,
        grammar,
    );
    if !location.is_empty() || !producer.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        if !location.is_empty() && !producer.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ": ", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, &producer, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_film_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
    let mut properties = bibliography_paragraph_properties(BibliographyStyle::ApaSixthEdition);
    properties.bidi = grammar.semantic_rtl.then_some(true);
    if let Some(mark) = properties.rpr.as_mut() {
        mark.language = grammar.language.map(str::to_owned);
        mark.language_bidi = grammar.language_bidi.map(str::to_owned);
        mark.language_east_asia = grammar.language_east_asia.map(str::to_owned);
    }
    paragraph.properties = Some(properties);
    let roles = [(BibliographyContributorRole::ProducerName, grammar.producer), (BibliographyContributorRole::Writer, grammar.writer), (BibliographyContributorRole::Director, grammar.director)]
        .into_iter().map(|(role,label)| {
            let name = apa_bibliography_names(source, role, true, grammar);
            let generated_initial = grammar.numeric_cs_font.is_none() && !grammar.full_names
        && !grammar.joined_names && source.contributors.iter().any(|contributor| contributor.role == role && matches!(&contributor.value, BibliographyAuthor::People(people) if people.last().is_some_and(|person| !person.last.is_empty() && (!person.first.is_empty() || !person.middle.is_empty()))));
            (name,label,generated_initial)
        }).filter(|(name,_,_)| !name.is_empty()).collect::<Vec<_>>();
    for (index, (name, label, generated_initial)) in roles.iter().enumerate() {
        let name_text = if *generated_initial {
            name.strip_suffix(grammar.initial_mark).unwrap_or(name)
        } else {
            name
        };
        append_apa_semantic_run(&mut paragraph, grammar, name_text, false, false)?;
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!(
                "{} (",
                if *generated_initial {
                    grammar.initial_mark.to_string()
                } else {
                    String::new()
                }
            ),
            false,
            true,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, label, false, grammar.rtl_labels)?;
        if index + 1 < roles.len() {
            let final_join = index + 2 == roles.len();
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!(
                    "){}{}",
                    grammar.contributor_separator,
                    if final_join {
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
                final_join || grammar.rtl_labels || grammar.numeric_cs_font.is_some(),
            )?;
        } else {
            append_apa_semantic_run(&mut paragraph, grammar, ").", false, true)?;
        }
    }
    let date = apa_bibliography_date(source, grammar);
    let title = first_source_property(source, BibliographySourceField::Title);
    append_apa_semantic_run(
        &mut paragraph,
        grammar,
        &format!(
            "{}({date}).{}",
            if roles.is_empty() { "" } else { " " },
            if title.is_empty() { "" } else { " " }
        ),
        false,
        true,
    )?;
    append_apa_semantic_title(
        &mut paragraph,
        grammar,
        title,
        true,
        grammar.curly_titles,
        false,
    )?;
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " [", false, true)?;
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            grammar.motion_picture,
            false,
            grammar.rtl_labels,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, "].", false, true)?;
    }
    let country = first_source_property(source, BibliographySourceField::CountryRegion);
    let distributor = first_source_property(source, BibliographySourceField::Distributor);
    if !country.is_empty() || !distributor.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_run(&mut paragraph, grammar, country, false, false)?;
        if !country.is_empty() && !distributor.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ": ", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, distributor, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_chapter_conference_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Author,
        grammar,
        false,
    )?;
    let title = first_source_property(source, BibliographySourceField::Title);
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    let editor =
        apa_bibliography_names(source, BibliographyContributorRole::Editor, false, grammar);
    let book_author = if source.kind == BibliographySourceKind::BookSection {
        apa_bibliography_names(
            source,
            BibliographyContributorRole::BookAuthor,
            false,
            grammar,
        )
    } else {
        String::new()
    };
    if !book_author.is_empty() || !editor.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        if !grammar.in_after && !grammar.in_label.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{}", grammar.in_label, grammar.in_before_separator),
                false,
                grammar.rtl_labels,
            )?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, &book_author, false, false)?;
        if !book_author.is_empty() && !editor.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
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
        if !editor.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, &editor, false, false)?;
            append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
            let (label, terminal) = grammar
                .editor
                .strip_suffix('.')
                .map_or((grammar.editor, ""), |label| (label, "."));
            append_apa_semantic_run(&mut paragraph, grammar, label, false, grammar.rtl_labels)?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{terminal})"),
                false,
                true,
            )?;
        }
        if grammar.in_after && source.kind == BibliographySourceKind::BookSection {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{}", grammar.in_separator, grammar.in_label),
                false,
                grammar.rtl_labels || grammar.numeric_cs_font.is_some(),
            )?;
        }
        if grammar.numeric_cs_font.is_some() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.contributor_separator.trim_end(),
                false,
                false,
            )?;
            append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        } else {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.contributor_separator,
                false,
                true,
            )?;
        }
    }
    let container = first_source_property(
        source,
        if source.kind == BibliographySourceKind::BookSection {
            BibliographySourceField::BookTitle
        } else {
            BibliographySourceField::ConferenceName
        },
    );
    append_apa_semantic_title(
        &mut paragraph,
        grammar,
        container,
        true,
        grammar.curly_titles && source.kind == BibliographySourceKind::ConferenceProceedings,
        true,
    )?;
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let pages = first_source_property(source, BibliographySourceField::Pages);
    if source.kind == BibliographySourceKind::BookSection {
        let translator = apa_bibliography_names(
            source,
            BibliographyContributorRole::Translator,
            false,
            grammar,
        );
        let edition = first_source_property(source, BibliographySourceField::Edition);
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
        if !details.is_empty() || !pages.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
            if grammar.rtl_labels {
                append_apa_semantic_run(&mut paragraph, grammar, &translator, false, false)?;
                let mut labels = Vec::new();
                if !translator.is_empty() {
                    labels.push(grammar.translator.to_owned());
                }
                if !edition.is_empty() {
                    labels.push(apa_bibliography_edition(edition, grammar));
                }
                if !volume.is_empty() {
                    labels.push(apa_bibliography_volume(volume, grammar));
                }
                let prefix = if translator.is_empty() {
                    ""
                } else {
                    grammar.contributor_separator
                };
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    &format!("{prefix}{}", labels.join(grammar.contributor_separator)),
                    false,
                    true,
                )?;
            } else {
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    &details.join(", "),
                    false,
                    false,
                )?;
            }
            if !pages.is_empty() {
                if !details.is_empty() {
                    append_apa_semantic_run(
                        &mut paragraph,
                        grammar,
                        grammar.contributor_separator,
                        false,
                        grammar.rtl_labels,
                    )?;
                }
                let label = grammar.pages.trim_end();
                let (label, terminal) = label
                    .strip_suffix('.')
                    .map_or((label, ""), |label| (label, "."));
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    label,
                    false,
                    grammar.rtl_labels && grammar.language_bidi != Some("ug-CN"),
                )?;
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    &format!("{terminal} {pages}"),
                    false,
                    true,
                )?;
            }
            append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
        }
        if !container.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
        }
    } else {
        if !container.is_empty() && !grammar.curly_titles {
            append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
        }
        if !volume.is_empty() && grammar.in_after && !grammar.in_label.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}{}", grammar.in_separator, grammar.in_label),
                false,
                grammar.rtl_labels || grammar.numeric_cs_font.is_some(),
            )?;
        }
        if !volume.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
            append_apa_semantic_run(&mut paragraph, grammar, volume, true, true)?;
        }
        if !pages.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                grammar.contributor_separator,
                false,
                true,
            )?;
            let label = grammar.pages.trim_end();
            let (label, terminal) = label
                .strip_suffix('.')
                .map_or((label, ""), |label| (label, "."));
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                label,
                false,
                grammar.rtl_labels && grammar.language_bidi != Some("ug-CN"),
            )?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{terminal} {pages}."),
                false,
                true,
            )?;
        }
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .filter(|field| {
        source.kind == BibliographySourceKind::BookSection
            || *field == BibliographySourceField::City
    })
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if !location.is_empty() || !publisher.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        if !location.is_empty() && !publisher.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ": ", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, publisher, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_general_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Author,
        grammar,
        false,
    )?;
    let title = first_source_property(source, BibliographySourceField::Title);
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    let publication = first_source_property(source, BibliographySourceField::PublicationTitle);
    let volume = first_source_property(source, BibliographySourceField::Volume);
    let issue = first_source_property(
        source,
        if source.kind == BibliographySourceKind::Misc {
            BibliographySourceField::Issue
        } else {
            BibliographySourceField::Edition
        },
    );
    let edition = first_source_property(source, BibliographySourceField::Edition);
    let pages = first_source_property(source, BibliographySourceField::Pages);
    if !publication.is_empty() || !volume.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_title(
            &mut paragraph,
            grammar,
            publication,
            true,
            grammar.curly_titles,
            false,
        )?;
        if !volume.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!(
                    "{}{volume}",
                    if publication.is_empty() {
                        ""
                    } else {
                        grammar.contributor_separator
                    }
                ),
                true,
                grammar.language_bidi != Some("ug-CN"),
            )?;
        }
    }
    if !issue.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!("({issue})"),
            source.kind == BibliographySourceKind::Misc,
            grammar.language_bidi != Some("ug-CN"),
        )?;
    }
    if source.kind == BibliographySourceKind::Misc && !edition.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!("{}{edition}", grammar.contributor_separator),
            true,
            grammar.language_bidi != Some("ug-CN"),
        )?;
    }
    if !pages.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!("{}{pages}", grammar.contributor_separator),
            false,
            grammar.language_bidi != Some("ug-CN"),
        )?;
    }
    if !publication.is_empty() || !volume.is_empty() || !issue.is_empty() || !pages.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    let roles = [
        (BibliographyContributorRole::Editor, grammar.editor),
        (BibliographyContributorRole::Translator, grammar.translator),
        (BibliographyContributorRole::Compiler, grammar.compiler),
    ]
    .into_iter()
    .map(|(role, label)| (apa_bibliography_names(source, role, false, grammar), label))
    .filter(|(name, _)| !name.is_empty())
    .collect::<Vec<_>>();
    if grammar.rtl_labels {
        append_apa_semantic_labeled_roles(&mut paragraph, grammar, &roles)?;
    } else if !roles.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
        if roles.len() > 1 {
            let (preceding, last) = roles.split_at(roles.len() - 1);
            let mut preceding_text = preceding
                .iter()
                .map(|(name, label)| format!("{name}, {label}"))
                .collect::<Vec<_>>()
                .join(grammar.contributor_separator);
            let terminal = if preceding
                .last()
                .is_some_and(|(_, label)| label.ends_with('.'))
            {
                preceding_text.pop();
                "."
            } else {
                ""
            };
            append_apa_semantic_run(&mut paragraph, grammar, &preceding_text, false, false)?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!(
                    "{terminal}{}{}{}",
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
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}, {}", last[0].0, last[0].1),
                false,
                false,
            )?;
        } else {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{}, {}", roles[0].0, roles[0].1),
                false,
                false,
            )?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
    }
    let location = [
        BibliographySourceField::City,
        BibliographySourceField::StateProvince,
        BibliographySourceField::CountryRegion,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if !location.is_empty() || !publisher.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        if !location.is_empty() && !publisher.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ": ", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, publisher, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_performance_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let title = first_source_property(source, BibliographySourceField::Title);
    let mut paragraph =
        apa_semantic_creator_paragraph(source, BibliographyContributorRole::Writer, grammar, true)?;
    if !title.is_empty() && !grammar.curly_titles {
        append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
    }
    let roles = [
        (BibliographyContributorRole::Director, grammar.director),
        (BibliographyContributorRole::Performer, grammar.performer),
    ]
    .into_iter()
    .map(|(role, label)| (apa_bibliography_names(source, role, false, grammar), label))
    .filter(|(name, _)| !name.is_empty())
    .collect::<Vec<_>>();
    if grammar.rtl_labels {
        append_apa_semantic_labeled_roles(&mut paragraph, grammar, &roles)?;
    } else if !roles.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
        for (index, (name, label)) in roles.iter().enumerate() {
            if index != 0 {
                append_apa_semantic_run(
                    &mut paragraph,
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
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{name}, {label}"),
                false,
                false,
            )?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
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
    .collect::<Vec<_>>();
    if !location.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_interview_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let title = first_source_property(source, BibliographySourceField::Title);
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Interviewee,
        grammar,
        false,
    )?;
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    let broadcast = first_source_property(source, BibliographySourceField::BroadcastTitle);
    if !broadcast.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_title(
            &mut paragraph,
            grammar,
            broadcast,
            true,
            grammar.curly_titles,
            true,
        )?;
        if !grammar.curly_titles {
            append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
        }
    }
    let interviewer = apa_bibliography_names(
        source,
        BibliographyContributorRole::Interviewer,
        false,
        grammar,
    );
    if grammar.rtl_labels {
        let roles = if interviewer.is_empty() {
            Vec::new()
        } else {
            vec![(interviewer.clone(), grammar.interviewer)]
        };
        append_apa_semantic_labeled_roles(&mut paragraph, grammar, &roles)?;
    } else if !interviewer.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!("{interviewer}, {}", grammar.interviewer),
            false,
            false,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
    }
    let broadcaster = first_source_property(source, BibliographySourceField::Broadcaster);
    if !broadcaster.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_run(&mut paragraph, grammar, broadcaster, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    let location = [
        BibliographySourceField::Station,
        BibliographySourceField::City,
    ]
    .into_iter()
    .map(|field| first_source_property(source, field))
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    if !location.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_legal_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = if source.kind == BibliographySourceKind::Patent {
        apa_semantic_creator_paragraph(
            source,
            BibliographyContributorRole::Inventor,
            grammar,
            false,
        )?
    } else {
        let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
        let mut properties = bibliography_paragraph_properties(BibliographyStyle::ApaSixthEdition);
        properties.bidi = grammar.semantic_rtl.then_some(true);
        if let Some(mark) = properties.rpr.as_mut() {
            mark.language = grammar.language.map(str::to_owned);
            mark.language_bidi = grammar.language_bidi.map(str::to_owned);
            mark.language_east_asia = grammar.language_east_asia.map(str::to_owned);
        }
        paragraph.properties = Some(properties);
        paragraph
    };
    if source.kind == BibliographySourceKind::Patent {
        let country = first_source_property(source, BibliographySourceField::CountryRegion);
        let number = first_source_property(source, BibliographySourceField::PatentNumber);
        if !country.is_empty() || !number.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, " ", true, true)?;
            append_apa_semantic_run(&mut paragraph, grammar, country, true, false)?;
            if !country.is_empty() && !number.is_empty() {
                append_apa_semantic_run(&mut paragraph, grammar, " ", true, true)?;
            }
            if grammar.rtl_labels {
                append_apa_semantic_run(&mut paragraph, grammar, grammar.patent, true, true)?;
                append_apa_semantic_run(&mut paragraph, grammar, number, true, false)?;
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    grammar.patent_suffix,
                    true,
                    true,
                )?;
            } else {
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    &format!("{}{number}{}", grammar.patent, grammar.patent_suffix),
                    true,
                    false,
                )?;
            }
            append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
        }
    } else {
        let title = first_source_property(source, BibliographySourceField::Title);
        let number = first_source_property(source, BibliographySourceField::CaseNumber);
        append_apa_semantic_run(&mut paragraph, grammar, title, false, false)?;
        if !number.is_empty() {
            if !title.is_empty() {
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    grammar.contributor_separator,
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
            append_apa_semantic_run(&mut paragraph, grammar, number, false, false)?;
        }
        let court = first_source_property(source, BibliographySourceField::Court);
        let month = first_source_property(source, BibliographySourceField::Month);
        let day = first_source_property(source, BibliographySourceField::Day);
        let year = first_source_property(source, BibliographySourceField::Year);
        if !court.is_empty() || !year.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
            append_apa_semantic_run(&mut paragraph, grammar, court, false, false)?;
            if (grammar.day_first || grammar.calendar_year_first)
                && !year.is_empty()
                && !month.is_empty()
            {
                let date = apa_bibliography_date(source, grammar);
                append_apa_semantic_run(
                    &mut paragraph,
                    grammar,
                    &format!("{}{date}", if court.is_empty() { "" } else { " " }),
                    false,
                    true,
                )?;
            } else {
                if !month.is_empty() {
                    append_apa_semantic_run(
                        &mut paragraph,
                        grammar,
                        &format!("{}{month}", if court.is_empty() { "" } else { " " }),
                        false,
                        true,
                    )?;
                    if !day.is_empty() {
                        append_apa_semantic_run(
                            &mut paragraph,
                            grammar,
                            &format!(" {day}"),
                            false,
                            true,
                        )?;
                    }
                }
                if !year.is_empty() {
                    append_apa_semantic_run(
                        &mut paragraph,
                        grammar,
                        &format!(
                            "{}{year}",
                            if month.is_empty() {
                                " "
                            } else {
                                grammar.calendar_day_year
                            }
                        ),
                        false,
                        true,
                    )?;
                }
            }
            append_apa_semantic_run(&mut paragraph, grammar, ")", false, true)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_art_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Artist,
        grammar,
        first_source_property(source, BibliographySourceField::PublicationTitle).is_empty(),
    )?;
    let title = first_source_property(source, BibliographySourceField::Title);
    let publication = first_source_property(source, BibliographySourceField::PublicationTitle);
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", publication.is_empty(), true)?;
    }
    if !publication.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_title(
            &mut paragraph,
            grammar,
            publication,
            true,
            grammar.curly_titles,
            true,
        )?;
        if !grammar.curly_titles {
            append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
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
    .collect::<Vec<_>>();
    if !location.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_periodical_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph = apa_semantic_creator_paragraph(
        source,
        BibliographyContributorRole::Author,
        grammar,
        false,
    )?;
    let title = first_source_property(source, BibliographySourceField::Title);
    if !title.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_secondary(&mut paragraph, source, grammar)?;
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
    if !periodical.is_empty() || !volume.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_title(
            &mut paragraph,
            grammar,
            periodical,
            true,
            grammar.curly_titles,
            false,
        )?;
        if !volume.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!(
                    "{}{volume}",
                    if periodical.is_empty() {
                        ""
                    } else {
                        grammar.contributor_separator
                    }
                ),
                true,
                grammar.language_bidi != Some("ug-CN"),
            )?;
        }
    }
    if !issue.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!("({issue})"),
            false,
            grammar.language_bidi != Some("ug-CN"),
        )?;
    }
    if !pages.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            grammar.contributor_separator,
            false,
            grammar.language_bidi != Some("ug-CN"),
        )?;
        if source.kind == BibliographySourceKind::ArticleInAPeriodical {
            let label = grammar.pages.trim_end();
            let (label, terminal) = label
                .strip_suffix('.')
                .map_or((label, ""), |text| (text, "."));
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                label,
                false,
                grammar.rtl_labels && grammar.language_bidi != Some("ug-CN"),
            )?;
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!("{terminal} "),
                false,
                true,
            )?;
        }
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            pages,
            false,
            grammar.language_bidi != Some("ug-CN")
                || source.kind == BibliographySourceKind::ArticleInAPeriodical,
        )?;
    }
    if !periodical.is_empty() || !volume.is_empty() || !issue.is_empty() || !pages.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}

pub(super) fn apa_rtl_authored_bibliography_paragraph(
    source: &BibliographySource,
    grammar: &ApaBibliographyLanguage,
) -> Result<oxoffice_docx_oxml::text::CT_P> {
    let mut paragraph =
        apa_semantic_creator_paragraph(source, BibliographyContributorRole::Author, grammar, true)?;
    let title = first_source_property(source, BibliographySourceField::Title);
    let edition = first_source_property(source, BibliographySourceField::Edition);
    let volume = first_source_property(source, BibliographySourceField::Volume);
    if source.kind == BibliographySourceKind::Book
        && (grammar.rtl_labels || grammar.literal_font.is_some() || grammar.edition_before)
        && (!edition.is_empty() || !volume.is_empty())
    {
        let mut details = Vec::new();
        if !edition.is_empty() {
            details.push(apa_bibliography_edition(edition, grammar));
        }
        if !volume.is_empty() {
            details.push(apa_bibliography_volume(volume, grammar));
        }
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!(" ({}).", details.join(grammar.contributor_separator)),
            false,
            true,
        )?;
    } else if source.kind == BibliographySourceKind::Book && !edition.is_empty() {
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &format!(" ({edition} "),
            false,
            true,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, grammar.edition, false, false)?;
        if !volume.is_empty() {
            append_apa_semantic_run(
                &mut paragraph,
                grammar,
                &format!(", {}", apa_bibliography_volume(volume, grammar)),
                false,
                false,
            )?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, ").", false, true)?;
    } else if source.kind == BibliographySourceKind::Book && !volume.is_empty() {
        append_apa_semantic_run(&mut paragraph, grammar, " (", false, true)?;
        append_apa_semantic_run(
            &mut paragraph,
            grammar,
            &apa_bibliography_volume(volume, grammar),
            false,
            false,
        )?;
        append_apa_semantic_run(&mut paragraph, grammar, ").", false, true)?;
    } else if !title.is_empty()
        && !(grammar.curly_titles
            && matches!(
                source.kind,
                BibliographySourceKind::Report | BibliographySourceKind::DocumentFromInternetSite
            ))
    {
        append_apa_semantic_run(&mut paragraph, grammar, ".", true, true)?;
    }
    if source.kind != BibliographySourceKind::Report {
        append_apa_semantic_secondary(&mut paragraph, source, grammar)?;
    }
    if source.kind == BibliographySourceKind::Report {
        let institution = [
            BibliographySourceField::Institution,
            BibliographySourceField::Department,
        ]
        .into_iter()
        .map(|field| first_source_property(source, field))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
        if !institution.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
            append_apa_semantic_source_list(&mut paragraph, grammar, &institution)?;
            append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
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
    .collect::<Vec<_>>();
    let publisher = first_source_property(source, BibliographySourceField::Publisher);
    if source.kind != BibliographySourceKind::DocumentFromInternetSite
        && (!location.is_empty() || !publisher.is_empty())
    {
        append_apa_semantic_run(&mut paragraph, grammar, " ", false, true)?;
        append_apa_semantic_source_list(&mut paragraph, grammar, &location)?;
        if !location.is_empty() && !publisher.is_empty() {
            append_apa_semantic_run(&mut paragraph, grammar, ": ", false, false)?;
        }
        append_apa_semantic_run(&mut paragraph, grammar, publisher, false, false)?;
        append_apa_semantic_run(&mut paragraph, grammar, ".", false, true)?;
    }
    append_apa_semantic_retrieval(&mut paragraph, source, grammar)?;
    Ok(paragraph)
}
