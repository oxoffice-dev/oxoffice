//! Updating citation and bibliography fields: the update state and what it reads from sources.

use super::*;

// Measured numeric CITATION operand/member boundary. Generic formatting has its own phase.
pub(crate) const NUMERIC_CITATION_SWITCH_LIMIT: usize = 10;

pub(crate) struct BibliographyUpdateState {
    collection: Option<BibliographyCollection>,
    style: Option<BibliographyStyle>,
    default_locale: Option<u32>,
    reference_numbers: BTreeMap<String, u32>,
    label_metrics: Option<(oxoffice_layout::FontManager, oxoffice_layout::FontId, f64)>,
}

impl BibliographyUpdateState {
    pub(super) fn new(document: &Document, default_locale: Option<u32>) -> Result<Self> {
        if let Some(locale) = default_locale {
            if locale == 0 {
                return Err(bibliography_error(
                    "application default locale must be nonzero",
                ));
            }
            validate_locale_number(locale)?;
        }
        let collection = bibliography_collection(&document.package, &document.doc_part_name)?;
        if let Some(collection) = &collection {
            unique_collection_identities(collection)?;
        }
        let style = collection
            .as_ref()
            .and_then(style_info)
            .and_then(|info| info.supported_style);
        let mut reference_numbers = BTreeMap::new();
        if matches!(
            style,
            Some(BibliographyStyle::Ieee | BibliographyStyle::Iso690Numeric)
        ) {
            let mut tags = crate::field::bibliography_citation_encounter_tags(document)?;
            if let Some(collection) = &collection {
                tags.extend(
                    collection
                        .sources
                        .iter()
                        .map(|(_, identity)| identity.tag.clone()),
                );
            }
            for tag in tags {
                if reference_numbers.contains_key(&tag)
                    || !collection.as_ref().is_some_and(|collection| {
                        collection
                            .sources
                            .iter()
                            .any(|(_, source)| source.tag == tag)
                    })
                {
                    continue;
                }
                let number = u32::try_from(reference_numbers.len())
                    .ok()
                    .and_then(|number| number.checked_add(1))
                    .ok_or_else(|| {
                        bibliography_error("citation reference count exceeds the numeric domain")
                    })?;
                reference_numbers.insert(tag, number);
            }
        }
        let label_metrics = if style == Some(BibliographyStyle::Ieee) {
            let style_id = if document.styles.get_by_id("Bibliography").is_some() {
                "Bibliography"
            } else {
                "Normal"
            };
            let effective =
                crate::style::resolve_run_properties(Some(style_id), None, &document.styles);
            let input = document.build_layout_input()?;
            let family = oxoffice_docx_layout::engine::resolve_font_family(
                &effective,
                input.theme.as_ref(),
                oxoffice_docx_layout::engine::WordFontSlot::Ascii,
            );
            let mut fonts = oxoffice_layout::FontManager::new_deterministic()?;
            fonts.load_embedded_fonts(&input.embedded_fonts)?;
            fonts.load_additional_fonts(&input.fonts);
            let font = fonts.resolve_font_for_metrics(
                family.as_deref(),
                effective.bold.unwrap_or(false),
                effective.italic.unwrap_or(false),
            )?;
            Some((fonts, font, effective.sz.map_or(11.0, |size| size.to_pt())))
        } else {
            None
        };
        Ok(Self {
            collection,
            style,
            default_locale,
            reference_numbers,
            label_metrics,
        })
    }

    pub(crate) fn bibliography_blocks(
        &self,
        instruction: &oxoffice_docx_oxml::text::FieldInstruction,
        text_width: Option<i32>,
    ) -> Result<Option<Vec<oxoffice_docx_oxml::document::BodyContent>>> {
        use oxoffice_docx_oxml::text::FieldArgument;
        let Some(collection) = &self.collection else {
            return Ok(None);
        };
        let Some(style) = self.style else {
            return Ok(None);
        };
        if !instruction.quotes_are_balanced() || !instruction.arguments.is_empty() {
            return Err(bibliography_error("ambiguous bibliography instruction"));
        }
        let mut tags = Vec::new();
        let mut field_locale = None;
        let mut locale_filter = None;
        for switch in &instruction.switches {
            match switch.name.as_str() {
                "m" | "l" => {
                    let Some(FieldArgument::Text(value)) = &switch.argument else {
                        return Err(bibliography_error(
                            "bibliography operand is missing or nested",
                        ));
                    };
                    if switch.name == "m" {
                        tags.push(value.as_str());
                    } else {
                        field_locale = Some(value.as_str());
                    }
                }
                "f" => {
                    locale_filter = Some(match &switch.argument {
                        None => 0,
                        Some(FieldArgument::Text(value)) => source_locale_number(value)?,
                        Some(_) => {
                            return Err(bibliography_error("nested bibliography locale filter"));
                        }
                    });
                }
                _ => return Ok(None),
            }
        }
        if tags.len() > 1 {
            return Err(not_covered(
                "a bibliography that selects more than one source",
            ));
        }
        let mut entries = Vec::new();
        for (index, identity) in &collection.sources {
            if !tags.is_empty() && !tags.contains(&identity.tag.as_str()) {
                continue;
            }
            let raw_locale = collection.parsed.optional_scalar(*index, "LCID")?;
            if let Some(filter) = locale_filter {
                let source_locale = raw_locale
                    .as_deref()
                    .map(source_locale_number)
                    .transpose()?
                    .unwrap_or(0);
                if source_locale != filter {
                    continue;
                }
            }
            let locale =
                formatting_locale(raw_locale.as_deref(), field_locale, self.default_locale)?;
            let Some(source) = collection.parsed.supported_source(
                *index,
                &identity.tag,
                identity.guid.as_deref(),
                identity.source_type.as_deref(),
                false,
            )?
            else {
                return Ok(None);
            };
            let reference = self.reference_numbers.get(&source.tag).copied();
            let numeric = matches!(
                style,
                BibliographyStyle::Iso690Numeric | BibliographyStyle::Ieee
            );
            let order = if numeric {
                reference.ok_or_else(|| {
                    bibliography_error("missing effective bibliography reference number")
                })?
            } else {
                0
            };
            // The entry first: an uncovered source is refused before its sort key.
            let paragraph = self.entry_paragraph(style, &source, reference, locale.clone())?;
            let key = if !numeric && tags.is_empty() && collection.sources.len() > 1 {
                bibliography_sort_key(&source, style, &locale)?
            } else {
                BibliographySortKey::default()
            };
            let rtl_paragraph_mark = style == BibliographyStyle::ApaSixthEdition
                && matches!(
                    locale,
                    BibliographyFormattingLocale::Numeric(
                        1025 | 1037
                            | 1056
                            | 1065
                            | 1123
                            | 2118
                            | 2137
                            | 1085
                            | 1114
                            | 1119
                            | 1120
                            | 1125
                            | 2080
                            | 1152
                            | 2049
                            | 3073
                            | 4097
                            | 5121
                            | 6145
                            | 7169
                            | 8193
                            | 9217
                            | 10241
                            | 11265
                            | 12289
                            | 13313
                            | 14337
                            | 15361
                            | 16385
                    )
                );
            let complex_paragraph_mark = style == BibliographyStyle::ApaSixthEdition
                && matches!(
                    locale,
                    BibliographyFormattingLocale::Numeric(
                        1081 | 1093
                            | 1094
                            | 1095
                            | 1096
                            | 1097
                            | 1098
                            | 1099
                            | 1100
                            | 1101
                            | 1102
                            | 1103
                            | 2144
                    )
                );
            let east_asia_paragraph_mark = style == BibliographyStyle::ApaSixthEdition
                && matches!(
                    locale,
                    BibliographyFormattingLocale::Numeric(
                        1028 | 1041 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124
                    )
                );
            entries.push((
                order,
                key,
                paragraph,
                (
                    rtl_paragraph_mark,
                    complex_paragraph_mark,
                    east_asia_paragraph_mark,
                ),
            ));
        }
        if entries.is_empty() {
            return Err(not_covered("a bibliography with no source to show"));
        }
        entries.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
        if style == BibliographyStyle::Ieee {
            let text_width = text_width
                .ok_or_else(|| not_covered("an IEEE bibliography whose text width is unknown"))?;
            let (fonts, font, size) = self
                .label_metrics
                .as_ref()
                .ok_or_else(|| bibliography_error("bibliography label font metrics unavailable"))?;
            let mut label_width = 0;
            let mut rows = Vec::new();
            for (number, _, paragraph, _) in entries {
                let advance = fonts
                    .shape_text(*font, &format!("[{number}]"), *size)?
                    .width
                    * 20.0;
                // Native bibliography-specific reserve, measured independently of generic table layout.
                // Four font/digit axes and two body-width axes bind this sans-trailing-space rule.
                let width = (advance + 75.0).ceil();
                if !width.is_finite() || width < 0.0 || width > f64::from(i32::MAX) {
                    return Err(bibliography_error("bibliography label width overflow"));
                }
                label_width = label_width.max(width as i32);
                rows.push((number, paragraph));
            }
            return Ok(Some(ieee_bibliography_table(
                rows,
                label_width,
                text_width,
            )?));
        }
        Ok(Some(
            entries
                .into_iter()
                .enumerate()
                .map(
                    |(
                        index,
                        (
                            _,
                            _,
                            mut paragraph,
                            (rtl_paragraph_mark, complex_paragraph_mark, east_asia_paragraph_mark),
                        ),
                    )| {
                        if index != 0
                            && let Some(properties) = paragraph.properties.as_mut()
                            && let Some(run_properties) = properties.rpr.as_mut()
                        {
                            run_properties.sz = None;
                            run_properties.sz_cs = None;
                            if rtl_paragraph_mark
                                || complex_paragraph_mark
                                || east_asia_paragraph_mark
                            {
                                run_properties.font_hint = Some(
                                    if east_asia_paragraph_mark {
                                        "eastAsia"
                                    } else {
                                        "cs"
                                    }
                                    .into(),
                                );
                                run_properties.rtl = rtl_paragraph_mark.then_some(true);
                                run_properties.complex_script =
                                    complex_paragraph_mark.then_some(true);
                            }
                        }
                        oxoffice_docx_oxml::document::BodyContent::Paragraph(Box::new(paragraph))
                    },
                )
                .collect(),
        ))
    }

    // One source's entry, refused when the style does not render it yet.
    fn entry_paragraph(
        &self,
        style: BibliographyStyle,
        source: &BibliographySource,
        reference: Option<u32>,
        locale: BibliographyFormattingLocale,
    ) -> Result<oxoffice_docx_oxml::text::CT_P> {
        if style != BibliographyStyle::ApaSixthEdition {
            let role = source
                .contributors
                .iter()
                .find(|contributor| contributor.role != BibliographyContributorRole::Author)
                .map(|contributor| contributor.role.xml_name());
            let field = source
                .properties
                .iter()
                .find(|property| {
                    !matches!(
                        property.field,
                        BibliographySourceField::Title
                            | BibliographySourceField::ShortTitle
                            | BibliographySourceField::Year
                            | BibliographySourceField::City
                            | BibliographySourceField::Publisher
                            | BibliographySourceField::ReferenceOrder
                    )
                })
                .map(|property| property.field.xml_name());
            let part = if locale != BibliographyFormattingLocale::Numeric(1033)
                || source.kind != BibliographySourceKind::Book
            {
                Some("bibliography entry".to_owned())
            } else if let Some(role) = role {
                Some(format!("bibliography entry with the {role} role"))
            } else {
                field.map(|field| format!("bibliography entry with the {field} field"))
            };
            if let Some(part) = part {
                return Err(source_not_covered(style, &part, source, &locale));
            }
        }
        source_bibliography_paragraph(source, style, reference, locale)
    }

    /// The entry of the source `tag`, as a bibliography with `field_locale` writes it.
    #[cfg(test)]
    pub(super) fn source_entry_for_test(
        &self,
        tag: &str,
        field_locale: Option<&str>,
    ) -> Result<oxoffice_docx_oxml::text::CT_P> {
        let collection = self.collection.as_ref().expect("source collection");
        let style = self.style.expect("bibliography style");
        let (index, identity) = collection
            .sources
            .iter()
            .find(|(_, identity)| identity.tag == tag)
            .expect("source tag");
        let raw_locale = collection.parsed.optional_scalar(*index, "LCID")?;
        let locale = formatting_locale(raw_locale.as_deref(), field_locale, self.default_locale)?;
        let source = collection
            .parsed
            .supported_source(
                *index,
                &identity.tag,
                identity.guid.as_deref(),
                identity.source_type.as_deref(),
                false,
            )?
            .expect("supported source");
        let reference = self.reference_numbers.get(tag).copied();
        self.entry_paragraph(style, &source, reference, locale)
    }

    pub(crate) fn citation_runs(
        &self,
        instruction: &oxoffice_docx_oxml::text::FieldInstruction,
    ) -> Result<Option<Vec<oxoffice_docx_oxml::text::CT_R>>> {
        use oxoffice_docx_oxml::text::FieldArgument;
        let Some(collection) = &self.collection else {
            return Ok(None);
        };
        let Some(style) = self.style else {
            return Ok(None);
        };
        if !instruction.quotes_are_balanced() {
            return Err(bibliography_error("ambiguous citation instruction"));
        }
        let full_instruction = instruction;
        let numeric_instruction = if matches!(
            style,
            BibliographyStyle::Ieee | BibliographyStyle::Iso690Numeric
        ) {
            if instruction
                .switches
                .iter()
                .skip(NUMERIC_CITATION_SWITCH_LIMIT)
                .any(|switch| {
                    switch.name == "*"
                        && !matches!(&switch.argument,
                    Some(FieldArgument::Text(value)) if value.eq_ignore_ascii_case("Upper"))
                })
            {
                return Err(not_covered(
                    r"a numeric citation with a late \* format switch",
                ));
            }
            let mut effective = instruction.clone();
            effective.switches.truncate(NUMERIC_CITATION_SWITCH_LIMIT);
            Some(effective)
        } else {
            None
        };
        let instruction = numeric_instruction.as_ref().unwrap_or(instruction);
        let [FieldArgument::Text(first)] = instruction.arguments.as_slice() else {
            return Err(bibliography_error(
                "citation requires one initial source tag",
            ));
        };
        let member = |tag: String| CitationSourceOptions {
            tag,
            pages: None,
            volume: None,
            prefix: None,
            suffix: None,
            suppress_author: false,
            suppress_year: false,
            suppress_title: false,
        };
        let mut members = vec![member(first.clone())];
        let mut field_locale = None;
        for switch in &instruction.switches {
            match switch.name.as_str() {
                "n" | "y" | "t" if switch.argument.is_none() => {
                    let current = members.last_mut().expect("initial citation member exists");
                    match switch.name.as_str() {
                        "n" => current.suppress_author = true,
                        "y" => current.suppress_year = true,
                        _ => current.suppress_title = true,
                    }
                }
                "l" | "m" | "p" | "v" | "f" | "s" => {
                    let Some(FieldArgument::Text(value)) = &switch.argument else {
                        return Err(bibliography_error("citation operand is missing or nested"));
                    };
                    if switch.name == "m" {
                        members.push(member(value.clone()));
                    } else if switch.name == "l" {
                        field_locale = Some(value.as_str());
                    } else {
                        let current = members.last_mut().expect("initial citation member exists");
                        let operand = match switch.name.as_str() {
                            "p" => &mut current.pages,
                            "v" => &mut current.volume,
                            "f" => &mut current.prefix,
                            _ => &mut current.suffix,
                        };
                        // Numeric styles use the first scalar operand for each source member.
                        if operand.is_none()
                            || !matches!(
                                style,
                                BibliographyStyle::Ieee | BibliographyStyle::Iso690Numeric
                            )
                        {
                            *operand = Some(value.clone());
                        }
                    }
                }
                "n" | "y" | "t" => return Err(bibliography_error("citation flag has an operand")),
                "*" if matches!(
                    style,
                    BibliographyStyle::Ieee | BibliographyStyle::Iso690Numeric
                ) => {}
                _ => return Ok(None),
            }
        }
        let mut fragments = Vec::new();
        let mut numeric_profile = None;
        for member in &members {
            if member.tag.trim().is_empty() {
                return Err(bibliography_error("empty citation source identity"));
            }
            let Some((index, identity)) = collection
                .sources
                .iter()
                .find(|(_, source)| source.tag == member.tag)
            else {
                return Ok(None);
            };
            let raw_locale = collection.parsed.optional_scalar(*index, "LCID")?;
            let locale =
                formatting_locale(raw_locale.as_deref(), field_locale, self.default_locale)?;
            let Some(source) = collection.parsed.supported_source(
                *index,
                &identity.tag,
                identity.guid.as_deref(),
                identity.source_type.as_deref(),
                false,
            )?
            else {
                return Ok(None);
            };
            if matches!(
                style,
                BibliographyStyle::Ieee | BibliographyStyle::Iso690Numeric
            ) {
                let number = self.reference_numbers.get(&member.tag).ok_or_else(|| {
                    bibliography_error("citation encounter numbering lost its source identity")
                })?;
                let profile = if style == BibliographyStyle::Ieee {
                    Some((true, 0, true))
                } else {
                    iso_numeric_run_profile(&locale)
                }
                .ok_or_else(|| source_not_covered(style, "citation", &source, &locale))?;
                if numeric_profile.is_some_and(|previous| previous != profile) {
                    return Err(not_covered(
                        "a numeric citation whose sources have different locales",
                    ));
                }
                numeric_profile = Some(profile);
                let mut fragment = number.to_string();
                if style == BibliographyStyle::Ieee {
                    if let Some(pages) = &member.pages {
                        if locale != BibliographyFormattingLocale::Numeric(1033)
                            && locale != BibliographyFormattingLocale::Numeric(1036)
                        {
                            return Err(source_not_covered(
                                style,
                                "citation page label",
                                &source,
                                &locale,
                            ));
                        }
                        let label = if pages.contains(['-', '–']) {
                            "pp."
                        } else {
                            "p."
                        };
                        fragment.push_str(&format!(", {label} {pages}"));
                    }
                } else if member.pages.is_some()
                    || member.volume.is_some()
                    || member.prefix.is_some()
                    || member.suffix.is_some()
                {
                    if locale != BibliographyFormattingLocale::Numeric(1033)
                        && locale != BibliographyFormattingLocale::Numeric(1036)
                    {
                        return Err(source_not_covered(
                            style,
                            "citation page or volume",
                            &source,
                            &locale,
                        ));
                    }
                    let pages = member.pages.as_deref().filter(|value| !value.is_empty());
                    let volume = member.volume.as_deref().filter(|value| !value.is_empty());
                    match (volume, pages) {
                        (Some(volume), Some(pages)) => {
                            fragment.push_str(&format!(" {volume}: {pages}"))
                        }
                        (Some(volume), None) => fragment.push_str(&format!(" vol. {volume}")),
                        (None, Some(pages)) => {
                            let label = if pages.contains(['-', '–']) {
                                "pp."
                            } else {
                                "p."
                            };
                            fragment.push_str(&format!(" {label} {pages}"));
                        }
                        (None, None) => {}
                    }
                    fragment = format!(
                        "{}{fragment}{}",
                        member.prefix.as_deref().unwrap_or_default(),
                        member.suffix.as_deref().unwrap_or_default()
                    );
                }
                fragments.push(fragment);
                continue;
            }
            // Additional style, locale and kind branches are implemented from their bound records.
            // An uncovered branch fails the transaction instead of retaining a stale result.
            if style == BibliographyStyle::ApaSixthEdition
                && source.kind == BibliographySourceKind::Patent
                && members.len() == 1
                && !first_source_property(&source, BibliographySourceField::CountryRegion)
                    .is_empty()
                && !first_source_property(&source, BibliographySourceField::PatentNumber).is_empty()
                && !first_source_property(&source, BibliographySourceField::Year).is_empty()
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && let Some(grammar) = apa_patent_run_grammar(&locale)
            {
                return Ok(Some(source_citation_runs(
                    &source, style, &locale, None, grammar.0, grammar.1,
                )?));
            }
            if style == BibliographyStyle::ApaSixthEdition
                && source.kind == BibliographySourceKind::Case
                && members.len() == 1
                && !first_source_property(&source, BibliographySourceField::ShortTitle).is_empty()
                && !first_source_property(&source, BibliographySourceField::Year).is_empty()
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && let Some(grammar) = apa_case_run_grammar(&locale)
            {
                return Ok(Some(source_citation_runs(
                    &source, style, &locale, None, grammar.0, grammar.1,
                )?));
            }
            if style == BibliographyStyle::Chicago
                && members.len() == 1
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && !first_source_property(&source, BibliographySourceField::Year).is_empty()
                && let Some(creator) = single_citation_creator(&source, style)
                && creator.last.len() == 1
                && creator.first.len() <= 1
                && !creator.last[0].is_empty()
                && let Some(grammar) = chicago_run_grammar(&locale)
            {
                if has_equivalent_creator(collection, *index, creator, style)? {
                    return Err(source_not_covered(
                        style,
                        "citation telling it apart from another source by the same author",
                        &source,
                        &locale,
                    ));
                }
                return Ok(Some(source_citation_runs(
                    &source,
                    style,
                    &locale,
                    Some(creator),
                    grammar.0,
                    grammar.1,
                )?));
            }
            if style == BibliographyStyle::ApaSixthEdition
                && !matches!(
                    source.kind,
                    BibliographySourceKind::Patent | BibliographySourceKind::Case
                )
                && members.len() == 1
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && !first_source_property(&source, BibliographySourceField::ShortTitle).is_empty()
                && !first_source_property(&source, BibliographySourceField::Year).is_empty()
                && let Some(creator) = single_citation_creator(&source, style)
                && creator.last.len() == 1
                && creator.first.len() <= 1
                && !creator.last[0].is_empty()
                && !has_equivalent_creator(collection, *index, creator, style)?
                && let Some(grammar) = apa_creator_run_grammar(&locale)
            {
                return Ok(Some(source_citation_runs(
                    &source,
                    style,
                    &locale,
                    Some(creator),
                    grammar.0,
                    grammar.1,
                )?));
            }
            if matches!(
                style,
                BibliographyStyle::Iso690AuthorDate | BibliographyStyle::Sist02
            ) && members.len() == 1
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && !first_source_property(&source, BibliographySourceField::Year).is_empty()
            {
                let title = matches!(
                    source.kind,
                    BibliographySourceKind::JournalArticle
                        | BibliographySourceKind::ConferenceProceedings
                );
                let creator = if title {
                    None
                } else {
                    single_citation_creator(&source, style)
                };
                if title
                    && first_source_property(&source, BibliographySourceField::Title).is_empty()
                {
                    return Err(source_not_covered(
                        style,
                        "citation without a title",
                        &source,
                        &locale,
                    ));
                }
                if !title
                    && !creator.is_some_and(|person| {
                        person.last.len() == 1
                            && !person.last[0].is_empty()
                            && person.first.len() <= 1
                    })
                {
                    return Err(source_not_covered(
                        style,
                        "citation for these contributors",
                        &source,
                        &locale,
                    ));
                }
                if let Some(creator) = creator
                    && has_equivalent_creator(collection, *index, creator, style)?
                {
                    return Err(source_not_covered(
                        style,
                        "citation telling it apart from another source by the same author",
                        &source,
                        &locale,
                    ));
                }
                let grammar = match style {
                    BibliographyStyle::Iso690AuthorDate => {
                        iso_author_date_run_grammar(&locale, title)
                    }
                    BibliographyStyle::Sist02 => sist_run_grammar(&locale, title),
                    _ => unreachable!(),
                };
                if let Some(grammar) = grammar {
                    return Ok(Some(source_citation_runs(
                        &source, style, &locale, creator, grammar.0, grammar.1,
                    )?));
                }
            }
            if matches!(
                style,
                BibliographyStyle::Gb7714
                    | BibliographyStyle::GostName
                    | BibliographyStyle::GostTitle
                    | BibliographyStyle::HarvardAnglia
                    | BibliographyStyle::MlaSeventhEdition
                    | BibliographyStyle::Turabian
            ) && members.len() == 1
                && member.pages.is_none()
                && member.volume.is_none()
                && member.prefix.is_none()
                && member.suffix.is_none()
                && !member.suppress_author
                && !member.suppress_year
                && !member.suppress_title
                && (style == BibliographyStyle::MlaSeventhEdition
                    || !first_source_property(&source, BibliographySourceField::Year).is_empty())
            {
                let title = matches!(
                    (style, source.kind),
                    (
                        BibliographyStyle::Gb7714,
                        BibliographySourceKind::JournalArticle
                            | BibliographySourceKind::ConferenceProceedings
                    ) | (
                        BibliographyStyle::HarvardAnglia,
                        BibliographySourceKind::Film | BibliographySourceKind::Case
                    ) | (
                        BibliographyStyle::MlaSeventhEdition,
                        BibliographySourceKind::Case
                    )
                );
                let creator = if title {
                    None
                } else {
                    single_citation_creator(&source, style)
                };
                if title
                    && first_source_property(
                        &source,
                        if style == BibliographyStyle::MlaSeventhEdition {
                            BibliographySourceField::ShortTitle
                        } else {
                            BibliographySourceField::Title
                        },
                    )
                    .is_empty()
                {
                    return Err(source_not_covered(
                        style,
                        "citation without a title",
                        &source,
                        &locale,
                    ));
                }
                if !title
                    && !creator.is_some_and(|person| {
                        person.last.len() == 1
                            && !person.last[0].is_empty()
                            && person.first.len() <= 1
                    })
                {
                    return Err(source_not_covered(
                        style,
                        "citation for these contributors",
                        &source,
                        &locale,
                    ));
                }
                if let Some(creator) = creator
                    && has_equivalent_creator(collection, *index, creator, style)?
                {
                    return Err(source_not_covered(
                        style,
                        "citation telling it apart from another source by the same author",
                        &source,
                        &locale,
                    ));
                }
                if let Some(grammar) = other_style_citation_run_grammar(style, &locale, title) {
                    return Ok(Some(source_citation_runs(
                        &source, style, &locale, creator, grammar.0, grammar.1,
                    )?));
                }
            }
            let patent_punctuation = apa_plain_patent_punctuation(&locale);
            let measured_plain_patent = source.kind == BibliographySourceKind::Patent
                && !first_source_property(&source, BibliographySourceField::CountryRegion)
                    .is_empty()
                && !first_source_property(&source, BibliographySourceField::PatentNumber)
                    .is_empty()
                && patent_punctuation.is_some()
                && member.pages.is_none()
                && member.volume.is_none();
            let measured_french_plain = locale == BibliographyFormattingLocale::Numeric(1036)
                && source.kind == BibliographySourceKind::Book
                && member.pages.as_deref().is_none_or(str::is_empty)
                && member.volume.as_deref().is_none_or(str::is_empty)
                && source.contributors.iter().filter(|contributor| contributor.role == BibliographyContributorRole::Author)
                    .all(|contributor| matches!(&contributor.value, BibliographyAuthor::People(people) if people.len() == 1));
            if style != BibliographyStyle::ApaSixthEdition
                || !(locale == BibliographyFormattingLocale::Numeric(1033)
                    || measured_french_plain
                    || measured_plain_patent)
            {
                return Err(source_not_covered(style, "citation", &source, &locale));
            }
            let disambiguate = single_citation_creator(&source, style)
                .map(|creator| has_equivalent_creator(collection, *index, creator, style))
                .transpose()?
                .unwrap_or(false);
            fragments.push(apa_citation_member(
                &source,
                member,
                patent_punctuation,
                disambiguate,
            ));
        }
        if let Some((separate, profile, square)) = numeric_profile {
            let separator = if style == BibliographyStyle::Ieee {
                ", "
            } else {
                "; "
            };
            let content = fragments.join(separator);
            let text = if square {
                format!("[{content}]")
            } else {
                format!("({content})")
            };
            let text =
                oxoffice_docx_layout::engine::format_numeric_field_general(full_instruction, &text)
                    .map_err(|diagnostic| bibliography_error(&diagnostic))?;
            if profile == 0 && !text.is_ascii() {
                if text.chars().any(|character| {
                    !matches!(character,
                    '\u{0000}'..='\u{00ff}' | '\u{0370}'..='\u{03ff}' |
                    '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
                }) {
                    return Err(not_covered("a numeric citation in this script"));
                }
                let mut runs = bibliography_display_runs(
                    &if separate { text } else { format!(" {text}") },
                    false,
                );
                for run in &mut runs {
                    if let Some(properties) = &mut run.properties {
                        properties.language = None;
                    }
                }
                if separate {
                    let mut leading = oxoffice_docx_oxml::text::CT_R::new(" ");
                    leading.properties = Some(native_citation_run_properties(0)?);
                    runs.insert(0, leading);
                }
                return Ok(Some(runs));
            }
            let mut run = oxoffice_docx_oxml::text::CT_R::new(if separate { &text } else { "" });
            if !separate {
                run = oxoffice_docx_oxml::text::CT_R::new(&format!(" {text}"));
            }
            run.properties = Some(native_citation_run_properties(profile)?);
            if !separate {
                return Ok(Some(vec![run]));
            }
            let mut leading = oxoffice_docx_oxml::text::CT_R::new(" ");
            leading.properties = Some(native_citation_run_properties(0)?);
            return Ok(Some(vec![leading, run]));
        }
        let mut leading = oxoffice_docx_oxml::text::CT_R::new(" ");
        leading.properties = Some(oxoffice_docx_oxml::CT_RPr {
            no_proof: Some(true),
            ..Default::default()
        });
        let mut citation =
            oxoffice_docx_oxml::text::CT_R::new(&format!("({})", fragments.join("; ")));
        citation.properties = leading.properties.clone();
        Ok(Some(vec![leading, citation]))
    }
}

fn iso_numeric_run_profile(locale: &BibliographyFormattingLocale) -> Option<(bool, usize, bool)> {
    let numeric = |number| match number {
        1025 | 1037 | 1056 | 1065 | 1085 | 1114 | 1119 | 1120 | 1123 | 1125 | 1152 | 2049
        | 2080 | 2118 | 2137 | 3073 | 4097 | 5121 | 6145 | 7169 | 8193 | 9217 | 10241 | 11265
        | 12289 | 13313 | 14337 | 15361 | 16385 => Some((true, 1, false)),
        1026 | 1027 | 1029 | 1030 | 1031 | 1032 | 1033 | 1034 | 1035 | 1036 | 1038 | 1039
        | 1040 | 1043 | 1044 | 1045 | 1046 | 1047 | 1048 | 1049 | 1050 | 1051 | 1052 | 1053
        | 1055 | 1057 | 1058 | 1059 | 1060 | 1061 | 1062 | 1063 | 1064 | 1066 | 1067 | 1068
        | 1069 | 1070 | 1071 | 1072 | 1073 | 1074 | 1075 | 1076 | 1077 | 1078 | 1079 | 1080
        | 1082 | 1083 | 1084 | 1086 | 1087 | 1088 | 1089 | 1090 | 1091 | 1092 | 1104 | 1106
        | 1110 | 1116 | 1117 | 1118 | 1122 | 1124 | 1126 | 1127 | 1128 | 1129 | 1130 | 1131
        | 1132 | 1136 | 1137 | 1138 | 1139 | 1140 | 1141 | 1142 | 1143 | 1145 | 1153 | 2055
        | 2058 | 2060 | 2064 | 2067 | 2068 | 2070 | 2072 | 2073 | 2074 | 2077 | 2092 | 2108
        | 2110 | 2115 | 2143 | 2155 | 2163 | 3079 | 3081 | 3082 | 3084 | 3098 | 3179 | 4103
        | 4105 | 4106 | 4108 | 4122 | 5127 | 5129 | 5130 | 5132 | 5146 | 6153 | 6154 | 6156
        | 7177 | 7178 | 7180 | 8201 | 8202 | 8204 | 9225 | 9226 | 9228 | 10249 | 10250 | 10252
        | 11273 | 11274 | 11276 | 12297 | 12298 | 12300 | 13321 | 13322 | 13324 | 14345 | 14346
        | 14348 | 15369 | 15370 | 15372 | 16393 | 16394 | 17417 | 17418 | 18441 | 18442 | 19466
        | 20490 | 21514 | 58378 | 58380 => Some((true, 0, false)),
        1028 | 1042 | 1144 | 2052 | 3076 | 4100 | 5124 => Some((true, 3, false)),
        1041 => Some((true, 3, true)),
        1054 | 1081 | 1093 | 1094 | 1095 | 1096 | 1097 | 1098 | 1099 | 1100 | 1101 | 1102
        | 1103 | 1105 | 1107 | 1108 | 1109 | 1111 | 1112 | 1113 | 1115 | 1121 | 2117 | 2128
        | 2144 | 2145 => Some((true, 6, false)),
        2057 | 2129 => Some((false, 0, false)),
        _ => None,
    };
    match locale {
        BibliographyFormattingLocale::Numeric(number) => numeric(*number),
        BibliographyFormattingLocale::Lexical(name) => {
            let mut choices = SOURCE_LOCALE_ENCODINGS
                .iter()
                .filter(|(_, encoding)| encoding.eq_ignore_ascii_case(name))
                .map(|(number, _)| numeric(*number));
            let first = choices.next()??;
            choices.all(|choice| choice == Some(first)).then_some(first)
        }
    }
}

pub(super) fn first_source_property(
    source: &BibliographySource,
    field: BibliographySourceField,
) -> &str {
    source
        .properties
        .iter()
        .find(|property| property.field == field)
        .map(|property| property.value.as_str())
        .unwrap_or("")
}
