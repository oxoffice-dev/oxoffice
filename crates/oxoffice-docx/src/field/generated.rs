//! Index, table of authorities and table of figures: inserting their fields and rebuilding their cached tables.

use super::*;

/// A nonprinting index marker and its page-number formatting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexEntry {
    pub levels: Vec<String>,
    pub identifier: Option<String>,
    pub page_range_bookmark: Option<String>,
    pub cross_reference: Option<String>,
    pub bold_page_numbers: bool,
    pub italic_page_numbers: bool,
}

/// A nonprinting authority occurrence. Short citations are category-local keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityEntry {
    pub long_citation: String,
    pub short_citation: String,
    pub category: u8,
    pub page_range_bookmark: Option<String>,
    pub bold_page_numbers: bool,
    pub italic_page_numbers: bool,
}

/// Native INDEX switches and the cached entry leader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOptions {
    pub identifier: Option<String>,
    pub heading_separator: Option<String>,
    pub entry_page_separator: String,
    pub page_separator: String,
    pub range_separator: String,
    pub run_in: bool,
    pub leader: crate::TabLeader,
    pub hyperlink: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            identifier: None,
            heading_separator: None,
            entry_page_separator: ", ".into(),
            page_separator: ", ".into(),
            range_separator: "–".into(),
            run_in: false,
            leader: crate::TabLeader::Dot,
            hyperlink: false,
        }
    }
}

/// A caption-selected TOC, independent of the TOC sequence page-prefix switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOfFiguresOptions {
    pub label: String,
    pub include_label_and_number: bool,
    pub hyperlink: bool,
    pub leader: crate::TabLeader,
    pub entry_page_separator: String,
}

impl Default for TableOfFiguresOptions {
    fn default() -> Self {
        Self {
            label: "Figure".into(),
            include_label_and_number: true,
            hyperlink: true,
            leader: crate::TabLeader::Dot,
            entry_page_separator: "\t".into(),
        }
    }
}

/// Native category, heading and passim selection for authorities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOfAuthoritiesOptions {
    pub category: Option<u8>,
    pub include_category_headings: bool,
    pub use_passim: bool,
    pub entry_page_separator: String,
    pub page_separator: String,
    pub range_separator: String,
    pub leader: crate::TabLeader,
}

impl Default for TableOfAuthoritiesOptions {
    fn default() -> Self {
        Self {
            category: None,
            include_category_headings: true,
            use_passim: true,
            entry_page_separator: "\t".into(),
            page_separator: ", ".into(),
            range_separator: "–".into(),
            leader: crate::TabLeader::Dot,
        }
    }
}

/// Counts of materialized entries and stable retained-cache diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedTablesReport {
    pub index_entries: usize,
    pub figure_entries: usize,
    pub authority_entries: usize,
    pub bookmark_count: usize,
    pub diagnostics: Vec<String>,
}

impl Document {
    /// Insert a checked complex XE marker without a printing cached result.
    pub fn insert_index_entry(
        &mut self,
        position: &crate::StoryRunPosition,
        entry: &IndexEntry,
    ) -> Result<()> {
        if entry.levels.is_empty()
            || entry
                .levels
                .iter()
                .any(|level| level.is_empty() || level.contains(':'))
        {
            return Err(Error::Other(
                "index hierarchy requires nonempty components without colons".into(),
            ));
        }
        if entry.page_range_bookmark.is_some() && entry.cross_reference.is_some() {
            return Err(Error::Other(
                "index marker cannot combine a range and cross-reference".into(),
            ));
        }
        let mut switches = Vec::new();
        for (name, value) in [
            ("f", &entry.identifier),
            ("r", &entry.page_range_bookmark),
            ("t", &entry.cross_reference),
        ] {
            if let Some(value) = value {
                generated_nonempty_operand(name, value)?;
                if name == "r" {
                    validate_reference_name(value)?;
                }
                switches.push(field_option_switch(name, Some(value.clone())));
            }
        }
        for (enabled, name) in [
            (entry.bold_page_numbers, "b"),
            (entry.italic_page_numbers, "i"),
        ] {
            if enabled {
                switches.push(field_option_switch(name, None));
            }
        }
        let instruction = FieldInstruction::new(
            "XE",
            vec![FieldArgument::Text(entry.levels.join(":"))],
            switches,
        )?;
        self.insert_checked_story_field(position, instruction)
    }

    /// Insert a checked complex TA occurrence with a category-local short key.
    pub fn insert_authority_entry(
        &mut self,
        position: &crate::StoryRunPosition,
        entry: &AuthorityEntry,
    ) -> Result<()> {
        generated_category(entry.category)?;
        generated_nonempty_operand("long citation", &entry.long_citation)?;
        generated_nonempty_operand("short citation", &entry.short_citation)?;
        let mut switches = vec![
            field_option_switch("l", Some(entry.long_citation.clone())),
            field_option_switch("s", Some(entry.short_citation.clone())),
            field_option_switch("c", Some(entry.category.to_string())),
        ];
        if let Some(bookmark) = &entry.page_range_bookmark {
            validate_reference_name(bookmark)?;
            switches.push(field_option_switch("r", Some(bookmark.clone())));
        }
        for (enabled, name) in [
            (entry.bold_page_numbers, "b"),
            (entry.italic_page_numbers, "i"),
        ] {
            if enabled {
                switches.push(field_option_switch(name, None));
            }
        }
        self.insert_checked_story_field(
            position,
            FieldInstruction::new("TA", Vec::new(), switches)?,
        )
    }

    /// Insert a dynamic INDEX at a checked story boundary.
    pub fn insert_index(
        &mut self,
        before: &crate::ContentLocation,
        options: &IndexOptions,
    ) -> Result<crate::ContentLocation> {
        let mut switches = vec![field_option_switch("z", Some("1033".into()))];
        for (name, value) in [
            ("f", &options.identifier),
            ("h", &options.heading_separator),
        ] {
            if let Some(value) = value {
                generated_nonempty_operand(name, value)?;
                switches.push(field_option_switch(name, Some(value.clone())));
            }
        }
        for (name, value) in [
            ("e", &options.entry_page_separator),
            ("l", &options.page_separator),
            ("g", &options.range_separator),
        ] {
            switches.push(field_option_switch(name, Some(value.clone())));
        }
        if options.run_in {
            switches.push(field_option_switch("r", None));
        }
        self.insert_generated_table_fields(
            before,
            &[FieldInstruction::new("INDEX", Vec::new(), switches)?],
            options.leader,
            options.hyperlink,
        )
    }

    /// Insert Word's caption-selected TOC instruction.
    pub fn insert_table_of_figures(
        &mut self,
        before: &crate::ContentLocation,
        options: &TableOfFiguresOptions,
    ) -> Result<crate::ContentLocation> {
        generated_nonempty_operand("caption label", &options.label)?;
        let selector = if options.include_label_and_number {
            "c"
        } else {
            "a"
        };
        let mut switches = vec![
            field_option_switch(selector, Some(options.label.clone())),
            field_option_switch("p", Some(options.entry_page_separator.clone())),
        ];
        if options.hyperlink {
            switches.push(field_option_switch("h", None));
        }
        self.insert_generated_table_fields(
            before,
            &[FieldInstruction::new("TOC", Vec::new(), switches)?],
            options.leader,
            options.hyperlink,
        )
    }

    /// Insert every populated numbered category atomically when category is None.
    pub fn insert_table_of_authorities(
        &mut self,
        before: &crate::ContentLocation,
        options: &TableOfAuthoritiesOptions,
    ) -> Result<crate::ContentLocation> {
        let categories = if let Some(category) = options.category {
            generated_category(category)?;
            vec![category]
        } else {
            generated_authority_categories(self)?
        };
        if categories.is_empty() {
            return Err(Error::Other("no populated authority category".into()));
        }
        let mut instructions = Vec::new();
        for category in categories {
            let mut switches = vec![field_option_switch("c", Some(category.to_string()))];
            for (enabled, name) in [
                (options.include_category_headings, "h"),
                (options.use_passim, "p"),
            ] {
                if enabled {
                    switches.push(field_option_switch(name, None));
                }
            }
            for (name, value) in [
                ("e", &options.entry_page_separator),
                ("l", &options.page_separator),
                ("g", &options.range_separator),
            ] {
                switches.push(field_option_switch(name, Some(value.clone())));
            }
            instructions.push(FieldInstruction::new("TOA", Vec::new(), switches)?);
        }
        self.insert_generated_table_fields(before, &instructions, options.leader, false)
    }

    fn insert_generated_table_fields(
        &mut self,
        before: &crate::ContentLocation,
        instructions: &[FieldInstruction],
        leader: crate::TabLeader,
        hyperlink: bool,
    ) -> Result<crate::ContentLocation> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let index = before
            .index_path()
            .first()
            .copied()
            .unwrap_or(candidate.story_items(before.story())?.len());
        // Keep the original checked sibling boundary while inserting in native category order.
        let mut original_index = index;
        let mut boundary = before.clone();
        for instruction in instructions {
            let previous_count = candidate.story_items(boundary.story())?.len();
            let mut paragraph = CT_P::new();
            let mut properties = CT_PPr {
                style_id: Some(
                    match instruction.name.as_str() {
                        "INDEX" => "Index1",
                        "TOA" => "TableofAuthorities",
                        _ => "TableofFigures",
                    }
                    .into(),
                ),
                tabs: Some(oxoffice_docx_oxml::borders::CT_Tabs {
                    tabs: vec![oxoffice_docx_oxml::borders::CT_TabStop {
                        val: ST_TabJc::Right,
                        pos: oxoffice_docx_oxml::units::Twips(9360),
                        leader: Some(generated_tab_leader(leader)),
                        source_occurrence: None,
                    }],
                }),
                ..Default::default()
            };
            if hyperlink && instruction.name == "INDEX" {
                properties.rpr = Some(CT_RPr {
                    style_id: Some("Hyperlink".into()),
                    ..Default::default()
                });
            }
            paragraph.properties = Some(properties);
            let mut run = CT_R::new("");
            if hyperlink {
                run.properties = Some(CT_RPr {
                    style_id: Some("Hyperlink".into()),
                    ..Default::default()
                });
            }
            run.content = vec![RunContent::Field(Field::from_instruction(
                instruction.clone(),
                oxoffice_docx_oxml::text::FieldForm::Complex,
                Vec::new(),
            )?)];
            paragraph.runs.push(run);
            candidate.insert_content(&boundary, crate::ContentFragment::paragraph(paragraph)?)?;
            let story = candidate
                .stories()?
                .into_iter()
                .find(|story| {
                    story.kind() == before.story().kind()
                        && story.part_name() == before.story().part_name()
                        && story.owner_index() == before.story().owner_index()
                })
                .ok_or_else(|| Error::Other("generated table story disappeared".into()))?;
            let new_count = candidate.story_items(&story)?.len();
            original_index += new_count
                .checked_sub(previous_count)
                .ok_or_else(|| Error::Other("generated insertion removed story items".into()))?;
            boundary = if before.index_path().is_empty() {
                crate::ContentLocation::end(story)
            } else {
                crate::ContentLocation::new(story, before.item_kind(), vec![original_index])
            };
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        let story = reopened
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind() == before.story().kind()
                    && story.part_name() == before.story().part_name()
                    && story.owner_index() == before.story().owner_index()
            })
            .ok_or_else(|| Error::Other("inserted generated story disappeared".into()))?;
        let first = reopened
            .story_items(&story)?
            .get(index)
            .ok_or_else(|| Error::Other("inserted generated table has no checked location".into()))?
            .location()
            .clone();
        self.commit_staged_mutation(reopened);
        Ok(first)
    }
}

fn generated_nonempty_operand(name: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::Other(format!("{name} must not be empty")));
    }
    oxoffice_core::xml::reject_non_xml_characters(name, value)?;
    Ok(())
}

fn generated_category(category: u8) -> Result<()> {
    if !(1..=16).contains(&category) {
        return Err(Error::Other(
            "authority category must be 1 through 16".into(),
        ));
    }
    Ok(())
}

fn generated_tab_leader(leader: crate::TabLeader) -> oxoffice_docx_oxml::shared::ST_TabLeader {
    match leader {
        crate::TabLeader::None => oxoffice_docx_oxml::shared::ST_TabLeader::None,
        crate::TabLeader::Dot => oxoffice_docx_oxml::shared::ST_TabLeader::Dot,
        crate::TabLeader::Hyphen => oxoffice_docx_oxml::shared::ST_TabLeader::Hyphen,
        crate::TabLeader::Underscore => oxoffice_docx_oxml::shared::ST_TabLeader::Underscore,
        crate::TabLeader::Heavy => oxoffice_docx_oxml::shared::ST_TabLeader::Heavy,
        crate::TabLeader::MiddleDot => oxoffice_docx_oxml::shared::ST_TabLeader::MiddleDot,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DynamicOwnerPolicy {
    Toc,
    GeneratedTables,
    Bibliography,
}

pub(super) fn generated_table_opcode(name: &str, policy: DynamicOwnerPolicy) -> bool {
    name == "TOC"
        || policy != DynamicOwnerPolicy::Toc && matches!(name, "INDEX" | "TOA")
        || policy == DynamicOwnerPolicy::Bibliography && matches!(name, "CITATION" | "BIBLIOGRAPHY")
}

fn generated_authority_categories(document: &Document) -> Result<Vec<u8>> {
    let snapshot =
        oxoffice_docx_layout::engine::evaluate_sequence_fields(&document.build_layout_input()?)
            .map_err(|error| {
                Error::Other(format!("authority source evaluation failed: {error}"))
            })?;
    let mut categories = BTreeSet::new();
    for index in 1u32.. {
        let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
            break;
        };
        if snapshot.source_node(node).is_none() {
            break;
        }
        for index in 0u32.. {
            let source = oxoffice_layout::FieldSource { node, index };
            let Some((instruction, _, cached)) = snapshot.source_field_context(source) else {
                break;
            };
            if cached {
                continue;
            }
            let field = Field::new(instruction, "");
            if field.instruction.name == "TA"
                && let Some(category) =
                    switch_text(&field.instruction, "c").and_then(|value| value.parse::<u8>().ok())
            {
                generated_category(category)?;
                categories.insert(category);
            }
        }
    }
    Ok(categories.into_iter().collect())
}

#[derive(Clone)]
enum GeneratedTableDefinition {
    Index {
        options: IndexOptions,
        sequence: Option<String>,
        cross_reference_separator: String,
    },
    Figures {
        options: TableOfFiguresOptions,
        styles: Vec<(String, u8)>,
        sequence: Option<String>,
        page_prefix_separator: String,
    },
    Authorities(TableOfAuthoritiesOptions),
}

#[derive(Clone)]
struct GeneratedTableOwner {
    story_index: usize,
    span_index: usize,
    definition: GeneratedTableDefinition,
    properties: CT_PPr,
}

#[derive(Clone)]
enum GeneratedSourceKind {
    Index(IndexEntry),
    Authority(AuthorityEntry),
    Caption {
        label: Option<String>,
        runs: Vec<CT_R>,
        tail: Vec<CT_R>,
        style: Option<String>,
    },
}

#[derive(Clone)]
pub(super) struct GeneratedTableSource {
    location: crate::ContentLocation,
    run: usize,
    kind: GeneratedSourceKind,
    properties: Option<CT_RPr>,
    target: String,
    last_target: Option<String>,
    sequence_values: BTreeMap<String, i64>,
    end_sequence_values: BTreeMap<String, i64>,
}

impl Document {
    /// Rebuild supported INDEX, caption-selected TOC and TOA caches atomically.
    /// Every target is resolved from one immutable layout after provisional insertion.
    /// Producer namespace bindings remain intact. Target insertion that requires
    /// unsafe canonical serialization refuses without changing the document.
    pub fn rebuild_generated_tables(&mut self) -> Result<GeneratedTablesReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let mut report = GeneratedTablesReport::default();
        let original_stories =
            generated_story_inventory(&candidate, DynamicOwnerPolicy::GeneratedTables)?;
        let original_sources = generated_table_sources(&candidate, &original_stories)?;
        generated_normalize_simple_tables(
            &mut candidate,
            SimpleGeneratedOwnerContext::Tables(&original_sources),
        )?;
        candidate.prepare_staged_package()?;
        let stories = generated_story_inventory(&candidate, DynamicOwnerPolicy::GeneratedTables)?;
        let mut owners = generated_table_owners(
            &candidate,
            &stories,
            &original_sources,
            &mut report.diagnostics,
        )?;
        if owners.is_empty() {
            return Ok(report);
        }
        let mut sources = generated_table_sources(&candidate, &stories)?;
        owners.retain(|owner| {
            let supported = generated_collation_supported(&candidate, &owner.definition, &sources);
            if !supported {
                report.diagnostics.push(
                    "generated table retains source keys outside captured en-US ASCII collation"
                        .into(),
                );
            }
            supported
        });
        if owners.is_empty() {
            return Ok(report);
        }
        sources.retain(|source| {
            owners
                .iter()
                .any(|owner| generated_source_matches(&candidate, &owner.definition, source))
        });
        generated_source_targets(&mut candidate, &mut sources, &mut report)?;
        generated_ensure_styles(&mut candidate, &sources);
        candidate.flush_to_package()?;
        let stories = generated_story_inventory(&candidate, DynamicOwnerPolicy::GeneratedTables)?;
        generated_publish_caches(
            &mut candidate,
            &stories,
            &owners,
            &sources,
            None,
            &mut report,
        )?;
        candidate = reopen_staged_document(candidate)?;
        // This is the only pagination call in the generated-table transaction.
        let snapshot = candidate.layout_deterministic()?;
        let stories = generated_story_inventory(&candidate, DynamicOwnerPolicy::GeneratedTables)?;
        generated_publish_caches(
            &mut candidate,
            &stories,
            &owners,
            &sources,
            Some(&snapshot),
            &mut report,
        )?;
        let completed = reopen_staged_document(candidate)?;
        completed.story_ranges()?;
        self.commit_staged_mutation(completed);
        Ok(report)
    }
}

pub(super) fn generated_story_inventory(
    document: &Document,
    policy: DynamicOwnerPolicy,
) -> Result<Vec<GeneratedStorySource>> {
    let mut result = Vec::new();
    for owner in document.generated_table_story_sources()? {
        let crate::document::GeneratedStoryOwner {
            story,
            range,
            xml: original,
            namespaces: scope,
        } = owner;
        let mut word_prefix = "generatedWord".to_owned();
        while scope.contains_key(&word_prefix) {
            word_prefix.push('_');
        }
        let mut wrapper = format!("<{word_prefix}:document xmlns:{word_prefix}=\"{W_NS}\"");
        for (prefix, namespace) in scope {
            if prefix == "xml" {
                continue;
            }
            if prefix.is_empty() {
                wrapper.push_str(" xmlns=\"");
            } else {
                wrapper.push_str(&format!(" xmlns:{prefix}=\""));
            }
            wrapper.push_str(&xml_escape_attribute(&namespace));
            wrapper.push('"');
        }
        wrapper.push_str(&format!("><{word_prefix}:body>"));
        let wrapper_len = wrapper.len();
        let mut xml = wrapper.into_bytes();
        xml.extend_from_slice(&original[range.clone()]);
        xml.extend_from_slice(format!("</{word_prefix}:body></{word_prefix}:document>").as_bytes());
        let mut parsed = CT_Document::from_xml(&xml)?;
        prepare_physical_story_projection(&mut parsed.body, &mut [])?;
        let spans = scan_dynamic_table_spans(&xml, policy)?;
        result.push(GeneratedStorySource {
            story,
            range,
            wrapper_len,
            xml,
            body: parsed.body,
            spans,
        });
    }
    Ok(result)
}

fn generated_table_owners(
    document: &Document,
    stories: &[GeneratedStorySource],
    original_sources: &[GeneratedTableSource],
    diagnostics: &mut Vec<String>,
) -> Result<Vec<GeneratedTableOwner>> {
    let mut result = Vec::new();
    for (story_index, story) in stories.iter().enumerate() {
        let mut paragraphs = Vec::new();
        collect_body_paragraphs(&story.body, &mut paragraphs, None);
        for (span_index, span) in story.spans.iter().enumerate() {
            let field = parse_dynamic_toc_field(&story.xml, span)?;
            if span.simple_field.is_some() && field.locked() != Some(true) {
                if generated_table_definition(&field.effective_instruction()).is_ok_and(
                    |definition| {
                        definition.is_some_and(|definition| {
                            !generated_collation_supported(document, &definition, original_sources)
                        })
                    },
                ) {
                    diagnostics.push("generated table retains source keys outside captured en-US ASCII collation".into());
                } else {
                    diagnostics.push(format!(
                        "generated {} retains unsupported simple owner formatting",
                        field.instruction.name
                    ));
                }
                continue;
            }
            if field.locked() == Some(true) {
                diagnostics.push(format!(
                    "generated {} field retains its locked cache",
                    field.instruction.name
                ));
                continue;
            }
            match generated_table_definition(&field.effective_instruction()) {
                Ok(Some(mut definition)) => {
                    if let GeneratedTableDefinition::Index { options, .. } = &mut definition {
                        options.hyperlink = paragraphs
                            .get(span.begin_paragraph)
                            .and_then(|paragraph| paragraph.properties.as_ref())
                            .and_then(|properties| properties.rpr.as_ref())
                            .and_then(|properties| properties.style_id.as_deref())
                            == Some("Hyperlink")
                            || field
                                .cached_display_segments()
                                .iter()
                                .any(|(_, properties)| {
                                    properties.is_some_and(|properties| {
                                        properties.style_id.as_deref() == Some("Hyperlink")
                                    })
                                });
                    }
                    result.push(GeneratedTableOwner {
                        story_index,
                        span_index,
                        definition,
                        properties: paragraphs
                            .get(span.begin_paragraph)
                            .and_then(|paragraph| paragraph.properties.clone())
                            .unwrap_or_default(),
                    });
                }
                Ok(None) => {}
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
    }
    Ok(result)
}

fn generated_table_definition(
    instruction: &FieldInstruction,
) -> std::result::Result<Option<GeneratedTableDefinition>, String> {
    if !instruction.arguments.is_empty() || !instruction.quotes_are_balanced() {
        return Err(format!(
            "generated {} retains malformed instruction",
            instruction.name
        ));
    }
    let allowed: &[&str] = match instruction.name.as_str() {
        "INDEX" => &["z", "f", "h", "e", "l", "g", "k", "s", "r", "*"],
        "TOA" => &["c", "h", "p", "e", "l", "g", "*"],
        "TOC" => &["c", "a", "t", "h", "p", "s", "d", "*"],
        _ => return Ok(None),
    };
    if instruction.name == "TOC"
        && !has_switch(instruction, "c")
        && !has_switch(instruction, "a")
        && !has_switch(instruction, "t")
    {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    for switch in &instruction.switches {
        if !allowed.contains(&switch.name.as_str()) || !seen.insert(&switch.name) {
            return Err(format!(
                "generated {} retains unsupported or duplicate switch \\{}",
                instruction.name, switch.name
            ));
        }
        let flag = match instruction.name.as_str() {
            "INDEX" => switch.name == "r",
            "TOA" => matches!(switch.name.as_str(), "h" | "p"),
            _ => switch.name == "h",
        };
        if flag != switch.argument.is_none()
            || switch
                .argument
                .as_ref()
                .is_some_and(|argument| !matches!(argument, FieldArgument::Text(_)))
        {
            return Err(format!(
                "generated {} retains malformed switch \\{}",
                instruction.name, switch.name
            ));
        }
        if switch.name == "*" && switch_text(instruction, "*") != Some("MERGEFORMAT") {
            return Err(format!(
                "generated {} retains unsupported formatting switch",
                instruction.name
            ));
        }
    }
    let separator =
        |name: &str, default: &str| switch_text(instruction, name).unwrap_or(default).to_owned();
    match instruction.name.as_str() {
        "INDEX" => {
            if switch_text(instruction, "z").is_some_and(|locale| locale != "1033") {
                return Err("INDEX retains unsupported collation locale".into());
            }
            let options = IndexOptions {
                identifier: switch_text(instruction, "f").map(str::to_owned),
                heading_separator: switch_text(instruction, "h").map(str::to_owned),
                entry_page_separator: separator("e", ", "),
                page_separator: separator("l", ", "),
                range_separator: separator("g", "–"),
                run_in: has_switch(instruction, "r"),
                ..Default::default()
            };
            Ok(Some(GeneratedTableDefinition::Index {
                options,
                sequence: switch_text(instruction, "s").map(str::to_owned),
                cross_reference_separator: separator("e", ". "),
            }))
        }
        "TOA" => {
            let category = switch_text(instruction, "c")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|value| (1..=16).contains(value))
                .ok_or("TOA retains invalid or omitted category")?;
            Ok(Some(GeneratedTableDefinition::Authorities(
                TableOfAuthoritiesOptions {
                    category: Some(category),
                    include_category_headings: has_switch(instruction, "h"),
                    use_passim: has_switch(instruction, "p"),
                    entry_page_separator: separator("e", "\t"),
                    page_separator: separator("l", ", "),
                    range_separator: separator("g", "–"),
                    ..Default::default()
                },
            )))
        }
        _ => {
            if has_switch(instruction, "c") && has_switch(instruction, "a") {
                return Err("TOC retains conflicting caption selectors".into());
            }
            let styles = switch_text(instruction, "t")
                .map(parse_custom_styles)
                .transpose()?
                .unwrap_or_default();
            Ok(Some(GeneratedTableDefinition::Figures {
                options: TableOfFiguresOptions {
                    label: switch_text(instruction, "c")
                        .or_else(|| switch_text(instruction, "a"))
                        .unwrap_or("")
                        .into(),
                    include_label_and_number: !has_switch(instruction, "a"),
                    hyperlink: has_switch(instruction, "h"),
                    entry_page_separator: separator("p", "\t"),
                    ..Default::default()
                },
                styles,
                sequence: switch_text(instruction, "s").map(str::to_owned),
                page_prefix_separator: separator("d", "-"),
            }))
        }
    }
}

fn generated_source_location(
    document: &Document,
    location: &crate::ContentLocation,
) -> Result<crate::ContentLocation> {
    let story = document
        .stories()?
        .into_iter()
        .find(|story| {
            story.kind() == location.story().kind()
                && story.part_name() == location.story().part_name()
                && story.owner_index() == location.story().owner_index()
        })
        .ok_or_else(|| Error::Other("generated source owner disappeared".into()))?;
    Ok(crate::ContentLocation::new(
        story,
        crate::StoryItemKind::Paragraph,
        location.index_path().to_vec(),
    ))
}

fn generated_source_targets(
    document: &mut Document,
    sources: &mut [GeneratedTableSource],
    report: &mut GeneratedTablesReport,
) -> Result<()> {
    let mut names = caption_bookmark_names(document)?;
    let original_ranges = document.story_ranges()?;
    let mut targets = HashMap::<(StoryKind, String, usize, Vec<usize>, usize), String>::new();
    for source in sources {
        let range = match &source.kind {
            GeneratedSourceKind::Index(entry) => entry.page_range_bookmark.as_deref(),
            GeneratedSourceKind::Authority(entry) => entry.page_range_bookmark.as_deref(),
            _ => None,
        };
        let cross_reference = matches!(&source.kind, GeneratedSourceKind::Index(entry) if entry.cross_reference.is_some());
        if cross_reference {
            continue;
        }
        if let Some(name) = range {
            let matching = original_ranges.iter().filter(|range| matches!(range.kind(), crate::StoryRangeKind::Bookmark { name: actual, .. } if actual == name)).collect::<Vec<_>>();
            let [range] = matching.as_slice() else {
                return Err(Error::Other(format!(
                    "generated range target {name} is missing or ambiguous"
                )));
            };
            source.target = name.into();
            let endpoint = &range.range().end;
            let key = (
                endpoint.location.story().kind(),
                endpoint.location.story().part_name().into(),
                endpoint.location.story().owner_index(),
                endpoint.location.index_path().to_vec(),
                endpoint.run_index,
            );
            if let Some(target) = targets.get(&key) {
                source.last_target = Some(target.clone());
                continue;
            }
            let existing =
                generated_existing_target(&original_ranges, &endpoint.location, endpoint.run_index);
            let target = if let Some(target) = existing {
                target
            } else {
                let target = generated_target_name(&mut names)?;
                let position = crate::StoryRunPosition {
                    location: generated_source_location(document, &endpoint.location)?,
                    run_index: endpoint.run_index,
                };
                document.add_story_bookmark(
                    &target,
                    crate::StoryRunRange {
                        start: position.clone(),
                        end: position,
                    },
                )?;
                report.bookmark_count += 1;
                target
            };
            targets.insert(key, target.clone());
            source.last_target = Some(target);
        } else {
            let key = (
                source.location.story().kind(),
                source.location.story().part_name().into(),
                source.location.story().owner_index(),
                source.location.index_path().to_vec(),
                source.run,
            );
            if let Some(target) = targets.get(&key) {
                source.target = target.clone();
                continue;
            }
            let target = generated_existing_target(&original_ranges, &source.location, source.run);
            let target = if let Some(target) = target {
                target
            } else {
                let target = generated_target_name(&mut names)?;
                let position = crate::StoryRunPosition {
                    location: generated_source_location(document, &source.location)?,
                    run_index: source.run,
                };
                document.add_story_bookmark(
                    &target,
                    crate::StoryRunRange {
                        start: position.clone(),
                        end: position,
                    },
                )?;
                report.bookmark_count += 1;
                target
            };
            targets.insert(key, target.clone());
            source.target = target;
        }
    }
    Ok(())
}

fn generated_existing_target(
    ranges: &[crate::StoryRangeRef],
    location: &crate::ContentLocation,
    run: usize,
) -> Option<String> {
    ranges.iter().find_map(|range| {
        let crate::StoryRangeKind::Bookmark { name, .. } = range.kind() else {
            return None;
        };
        let same_position = |position: &crate::StoryRunPosition| {
            position.location.story().kind() == location.story().kind()
                && position.location.story().part_name() == location.story().part_name()
                && position.location.story().owner_index() == location.story().owner_index()
                && position.location.index_path() == location.index_path()
                && position.run_index == run
        };
        (same_position(&range.range().start) && same_position(&range.range().end))
            .then_some(name.clone())
    })
}

fn generated_target_name(names: &mut HashSet<String>) -> Result<String> {
    for index in 0u32.. {
        let name = format!("GeneratedTable_{index}");
        if names.insert(name.clone()) {
            return Ok(name);
        }
    }
    Err(Error::Other(
        "generated table bookmark names exhausted".into(),
    ))
}

fn generated_ensure_styles(document: &mut Document, sources: &[GeneratedTableSource]) {
    let mut names = BTreeSet::new();
    for source in sources {
        match &source.kind {
            GeneratedSourceKind::Index(entry) => {
                for level in 1..=entry.levels.len() {
                    names.insert(format!("Index{level}"));
                }
                names.insert("IndexHeading".into());
            }
            GeneratedSourceKind::Authority(_) => {
                names.insert("TableofAuthorities".into());
                names.insert("TOAHeading".into());
            }
            GeneratedSourceKind::Caption { .. } => {
                names.insert("TableofFigures".into());
            }
        }
    }
    for name in names {
        if document.styles.get_by_id(&name).is_some() {
            continue;
        }
        let (mut style, _, _) = style::StyleBuilder::paragraph(&name, &name).build();
        if let Some(level) = name
            .strip_prefix("Index")
            .and_then(|level| level.parse::<i32>().ok())
        {
            style.ppr = Some(CT_PPr {
                ind_left: Some(oxoffice_docx_oxml::units::Twips(level.saturating_mul(220))),
                ind_hanging: Some(oxoffice_docx_oxml::units::Twips(220)),
                ..Default::default()
            });
        } else if name == "TableofAuthorities" {
            style.ppr = Some(CT_PPr {
                ind_left: Some(oxoffice_docx_oxml::units::Twips(220)),
                ind_hanging: Some(oxoffice_docx_oxml::units::Twips(220)),
                space_after: Some(oxoffice_docx_oxml::units::Twips(0)),
                ..Default::default()
            });
        }
        std::sync::Arc::make_mut(&mut document.styles)
            .styles
            .push(style);
    }
}

fn generated_publish_caches(
    document: &mut Document,
    stories: &[GeneratedStorySource],
    owners: &[GeneratedTableOwner],
    sources: &[GeneratedTableSource],
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
    report: &mut GeneratedTablesReport,
) -> Result<()> {
    let mut edits = BTreeMap::<String, Vec<FieldSourceEdit>>::new();
    for owner in owners {
        let story = stories
            .get(owner.story_index)
            .ok_or_else(|| Error::Other("generated story ownership changed".into()))?;
        let span = story
            .spans
            .get(owner.span_index)
            .ok_or_else(|| Error::Other("generated field ownership changed".into()))?;
        let generated = generated_render_table(document, owner, sources, snapshot, report)?;
        let replacement = if span.begin_paragraph == span.end_paragraph {
            let mut replacement = Vec::new();
            append_toc_wrapper_closures(&mut replacement, &span.separator_wrapper_names);
            replacement.extend_from_slice(format!("</{}>", span.start_paragraph_name).as_bytes());
            replacement.extend_from_slice(&generated);
            replacement.extend_from_slice(
                &story.xml[span.end_paragraph_start..span.end_paragraph_content_start],
            );
            for &(start, end) in &span.end_wrapper_prefixes {
                replacement.extend_from_slice(&story.xml[start..end]);
            }
            replacement
        } else {
            dynamic_toc_replacement(&story.xml, span, &generated, &[])?
        };
        let offset = |value: usize| {
            value
                .checked_sub(story.wrapper_len)
                .and_then(|value| value.checked_add(story.range.start))
                .ok_or_else(|| {
                    Error::Other(
                        "generated field source offset is outside its physical owner".into(),
                    )
                })
        };
        edits
            .entry(story.story.part_name().into())
            .or_default()
            .push(FieldSourceEdit {
                start: offset(span.result_start)?,
                end: offset(span.result_end)?,
                replacement,
            });
    }
    for (part, mut edits) in edits {
        edits.sort_by_key(|edit| edit.start);
        if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(Error::Other("generated field cache edits overlap".into()));
        }
        let mut xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("generated story part disappeared".into()))?
            .to_vec();
        for edit in edits.into_iter().rev() {
            xml.splice(edit.start..edit.end, edit.replacement);
        }
        validate_strict_xml_1_0(&xml)
            .map_err(|error| Error::Other(format!("invalid generated story XML: {error:?}")))?;
        document.package.set_part(&part, xml);
    }
    Ok(())
}

fn generated_table_sources(
    document: &Document,
    stories: &[GeneratedStorySource],
) -> Result<Vec<GeneratedTableSource>> {
    let paragraphs = document.generated_table_paragraph_positions()?;
    let snapshot =
        oxoffice_docx_layout::engine::evaluate_sequence_fields(&document.build_layout_input()?)
            .map_err(|error| {
                Error::Other(format!(
                    "generated sequence source evaluation failed: {error}"
                ))
            })?;
    let mut result = Vec::new();
    let mut sequence_values = BTreeMap::new();
    for story in stories {
        let mut typed = Vec::new();
        collect_body_paragraphs(&story.body, &mut typed, None);
        let locations = paragraphs
            .iter()
            .filter(|(location, range)| {
                location.story().part_name() == story.story.part_name()
                    && story.range.start <= range.start
                    && range.end <= story.range.end
                    && (location.story().kind() != StoryKind::TextBox
                        || story.story.kind() == StoryKind::TextBox)
            })
            .collect::<Vec<_>>();
        if locations.len() != typed.len() {
            return Err(Error::Other(format!(
                "generated physical paragraph inventory disagrees with accepted ownership: {:?} locations {} paragraphs {}",
                story.story.kind(),
                locations.len(),
                typed.len()
            )));
        }
        for (paragraph_index, (paragraph, (location, _))) in
            typed.iter().zip(&locations).enumerate()
        {
            let mut caption_runs = Vec::new();
            let mut caption_label = None;
            let mut caption_tail = Vec::new();
            let mut after_sequence = false;
            let mut caption_has_source = false;
            // Physical field identities count every field of the paragraph,
            // deleted ones included, as the sequence snapshot does.
            let mut physical_indices = HashMap::new();
            let mut next_index = 0u32;
            for run in paragraph.source_runs() {
                for content in &run.content {
                    let RunContent::Field(field) = content else {
                        continue;
                    };
                    physical_indices.insert(std::ptr::from_ref(field), next_index);
                    next_index = next_index
                        .checked_add(
                            1 + u32::try_from(field.all_nested_fields_in_source_order().len())
                                .map_err(|_| {
                                    Error::Other(
                                        "generated source field inventory is too large".into(),
                                    )
                                })?,
                        )
                        .ok_or_else(|| {
                            Error::Other("generated source field inventory overflow".into())
                        })?;
                }
            }
            for (accepted_index, accepted) in accepted_toc_runs(paragraph).into_iter().enumerate() {
                let position = TocOwnedPosition {
                    paragraph: paragraph_index,
                    run: TocRunPosition {
                        run_boundary: accepted.run_boundary,
                        raw_order: accepted.raw_order,
                        nested_order: accepted.nested_order,
                    },
                };
                let owned = toc_source_position_is_owned(&story.spans, position);
                let mut display_run = accepted.run.clone();
                display_run.content.clear();
                let mut tail_run = display_run.clone();
                for content in &accepted.run.content {
                    let RunContent::Field(field) = content else {
                        if !owned {
                            display_run.content.push(content.clone());
                            if after_sequence {
                                tail_run.content.push(content.clone());
                            }
                        }
                        continue;
                    };
                    let instruction = field.effective_instruction();
                    let physical_field_index = physical_indices
                        .get(&std::ptr::from_ref(field))
                        .copied()
                        .ok_or_else(|| {
                            Error::Other("generated source field has no physical identity".into())
                        })?;
                    if owned {
                        continue;
                    }
                    let kind = match instruction.name.as_str() {
                        "XE" => Some(GeneratedSourceKind::Index(generated_parse_index_marker(
                            &instruction,
                        )?)),
                        "TA" => Some(GeneratedSourceKind::Authority(
                            generated_parse_authority_marker(&instruction)?,
                        )),
                        "SEQ" => {
                            let label = text_argument(&instruction, 0).ok_or_else(|| {
                                Error::Other("caption sequence has no identifier".into())
                            })?;
                            let value = generated_sequence_value(
                                &snapshot,
                                document,
                                &story.story,
                                paragraph_index,
                                physical_field_index,
                                field,
                            )?;
                            if let Ok(number) = value.parse::<i64>() {
                                sequence_values.insert(label.to_ascii_lowercase(), number);
                            }
                            caption_label = Some(label.into());
                            after_sequence = true;
                            display_run
                                .content
                                .push(RunContent::Text(CT_Text::new(&value)));
                            None
                        }
                        _ => {
                            if !matches!(instruction.name.as_str(), "INDEX" | "TOA" | "TOC") {
                                display_run.content.push(content.clone());
                                if after_sequence {
                                    tail_run.content.push(content.clone());
                                }
                            }
                            None
                        }
                    };
                    if let Some(kind) = kind {
                        result.push(GeneratedTableSource {
                            location: (*location).clone(),
                            run: accepted_index,
                            kind,
                            properties: accepted.run.properties.clone(),
                            target: String::new(),
                            last_target: None,
                            sequence_values: sequence_values.clone(),
                            end_sequence_values: sequence_values.clone(),
                        });
                    }
                }
                if !owned {
                    caption_has_source = true;
                }
                if !tail_run.content.is_empty() {
                    caption_tail.push(tail_run);
                }
                if !display_run.content.is_empty() {
                    caption_runs.push(display_run);
                }
            }
            if caption_has_source
                && (caption_label.is_some()
                    || paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.style_id.as_ref())
                        .is_some())
            {
                if let Some(first) = caption_tail.first_mut() {
                    for content in &mut first.content {
                        if let RunContent::Text(text) = content {
                            text.text = text
                                .text
                                .trim_start_matches([' ', ':', '.', '\t'])
                                .to_owned();
                            break;
                        }
                    }
                }
                result.push(GeneratedTableSource {
                    location: (*location).clone(),
                    run: 0,
                    kind: GeneratedSourceKind::Caption {
                        label: caption_label,
                        runs: caption_runs,
                        tail: caption_tail,
                        style: paragraph
                            .properties
                            .as_ref()
                            .and_then(|properties| properties.style_id.clone()),
                    },
                    properties: None,
                    target: String::new(),
                    last_target: None,
                    sequence_values: sequence_values.clone(),
                    end_sequence_values: sequence_values.clone(),
                });
            }
        }
    }
    let ranges = document.story_ranges()?;
    for source in &mut result {
        let name = match &source.kind {
            GeneratedSourceKind::Index(entry) => entry.page_range_bookmark.as_deref(),
            GeneratedSourceKind::Authority(entry) => entry.page_range_bookmark.as_deref(),
            _ => None,
        };
        let Some(name) = name else {
            continue;
        };
        let matching = ranges.iter().filter(|range| matches!(range.kind(), crate::StoryRangeKind::Bookmark { name: actual, .. } if actual == name)).collect::<Vec<_>>();
        let [range] = matching.as_slice() else {
            return Err(Error::Other(format!(
                "generated range target {name} is missing or ambiguous"
            )));
        };
        source.sequence_values =
            generated_sequence_context_at(&snapshot, document, &range.range().start)?;
        source.end_sequence_values =
            generated_sequence_context_at(&snapshot, document, &range.range().end)?;
    }
    Ok(result)
}

fn generated_sequence_context_at(
    snapshot: &oxoffice_docx_layout::WordSequenceSnapshot,
    document: &Document,
    position: &crate::StoryRunPosition,
) -> Result<BTreeMap<String, i64>> {
    let mut values = BTreeMap::new();
    if position.location.story().kind() != StoryKind::Body {
        return Ok(values);
    }
    let path = generated_main_source_path(document, &position.location)?;
    for event in snapshot.main_events() {
        let Some(source) = snapshot.source_node(event.source.node) else {
            return Err(Error::Other(
                "sequence event lost its physical source".into(),
            ));
        };
        if (source.children.as_slice(), event.accepted_run) < (path.as_slice(), position.run_index)
        {
            values.insert(event.identifier.to_ascii_lowercase(), event.value);
        }
    }
    Ok(values)
}

fn generated_main_source_path(
    document: &Document,
    location: &crate::ContentLocation,
) -> Result<Vec<usize>> {
    if location.story().kind() == StoryKind::Body && location.index_path().len() == 1 {
        Ok(document
            .story_items(location.story())?
            .into_iter()
            .find(|item| item.location() == location)
            .ok_or_else(|| {
                Error::Other("generated source paragraph is absent from its checked owner".into())
            })?
            .direct_body_index()?
            .map(|index| vec![index])
            .unwrap_or_default())
    } else {
        Ok(location.index_path().to_vec())
    }
}

fn generated_sequence_value(
    snapshot: &oxoffice_docx_layout::WordSequenceSnapshot,
    document: &Document,
    story: &crate::StoryId,
    paragraph_index: usize,
    field_index: u32,
    field: &Field,
) -> Result<String> {
    let input = document.build_layout_input()?;
    let mut nodes = Vec::new();
    for index in 1u32.. {
        let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
            break;
        };
        let Some(path) = snapshot.source_node(node) else {
            break;
        };
        let same_story = match &path.story {
            oxoffice_docx_layout::WordStory::Document => {
                story.kind() == StoryKind::Body && story.part_name() == document.doc_part_name
            }
            oxoffice_docx_layout::WordStory::Header { .. } => {
                story.kind() == StoryKind::Header
                    && input.story_part_names.get(&path.story).map(String::as_str)
                        == Some(story.part_name())
            }
            oxoffice_docx_layout::WordStory::Footer { .. } => {
                story.kind() == StoryKind::Footer
                    && input.story_part_names.get(&path.story).map(String::as_str)
                        == Some(story.part_name())
            }
            oxoffice_docx_layout::WordStory::Footnote { id } => {
                document.footnote_story(*id)?.is_some_and(|owner| {
                    owner.kind() == story.kind()
                        && owner.part_name() == story.part_name()
                        && owner.owner_index() == story.owner_index()
                })
            }
            oxoffice_docx_layout::WordStory::Endnote { id } => {
                document.endnote_story(*id)?.is_some_and(|owner| {
                    owner.kind() == story.kind()
                        && owner.part_name() == story.part_name()
                        && owner.owner_index() == story.owner_index()
                })
            }
            oxoffice_docx_layout::WordStory::TextBox { part_name, .. } => {
                story.kind() == StoryKind::TextBox
                    && part_name == story.part_name()
                    && snapshot.text_box_owner_index(&path.story) == Some(story.owner_index())
            }
        };
        if same_story {
            nodes.push(node);
        }
    }
    let node = *nodes
        .get(paragraph_index)
        .ok_or_else(|| Error::Other("caption source has no qualified physical paragraph".into()))?;
    let source = oxoffice_layout::FieldSource {
        node,
        index: field_index,
    };
    let instruction = field.effective_instruction_text();
    if !snapshot
        .source_field_context(source)
        .is_some_and(|(actual, _, cached)| !cached && actual.trim() == instruction.trim())
    {
        return Err(Error::Other(
            "caption source disagrees with its physical field identity".into(),
        ));
    }
    snapshot
        .field_value(source)
        .ok_or_else(|| Error::Other("caption source has no physical sequence decision".into()))?
        .map(str::to_owned)
        .map_err(|diagnostic| Error::Other(diagnostic.into()))
}

fn generated_parse_index_marker(instruction: &FieldInstruction) -> Result<IndexEntry> {
    generated_marker_switches(instruction, &["f", "r", "t", "b", "i"])?;
    if instruction.arguments.len() != 1 {
        return Err(Error::Other("XE requires one hierarchy operand".into()));
    }
    let levels = text_argument(instruction, 0)
        .ok_or_else(|| Error::Other("XE hierarchy must be text".into()))?
        .split(':')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if levels.iter().any(String::is_empty) {
        return Err(Error::Other(
            "XE contains an empty hierarchy component".into(),
        ));
    }
    let entry = IndexEntry {
        levels,
        identifier: switch_text(instruction, "f").map(str::to_owned),
        page_range_bookmark: switch_text(instruction, "r").map(str::to_owned),
        cross_reference: switch_text(instruction, "t").map(str::to_owned),
        bold_page_numbers: has_switch(instruction, "b"),
        italic_page_numbers: has_switch(instruction, "i"),
    };
    if entry.page_range_bookmark.is_some() && entry.cross_reference.is_some() {
        return Err(Error::Other(
            "XE combines a range and cross-reference".into(),
        ));
    }
    Ok(entry)
}

fn generated_parse_authority_marker(instruction: &FieldInstruction) -> Result<AuthorityEntry> {
    generated_marker_switches(instruction, &["l", "s", "c", "r", "b", "i"])?;
    if !instruction.arguments.is_empty() {
        return Err(Error::Other("TA does not take positional operands".into()));
    }
    let long_citation = switch_text(instruction, "l")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Other("TA requires a long citation".into()))?
        .into();
    let short_citation = switch_text(instruction, "s")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Other("TA requires a short grouping citation".into()))?
        .into();
    let category = switch_text(instruction, "c")
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| Error::Other("TA requires a numbered category".into()))?;
    generated_category(category)?;
    Ok(AuthorityEntry {
        long_citation,
        short_citation,
        category,
        page_range_bookmark: switch_text(instruction, "r").map(str::to_owned),
        bold_page_numbers: has_switch(instruction, "b"),
        italic_page_numbers: has_switch(instruction, "i"),
    })
}

fn generated_marker_switches(instruction: &FieldInstruction, allowed: &[&str]) -> Result<()> {
    let mut seen = HashSet::new();
    for switch in &instruction.switches {
        if !allowed.contains(&switch.name.as_str()) || !seen.insert(&switch.name) {
            return Err(Error::Other(format!(
                "{} source contains unsupported or duplicate switch \\{}",
                instruction.name, switch.name
            )));
        }
        let flag = matches!(switch.name.as_str(), "b" | "i");
        if flag != switch.argument.is_none()
            || switch
                .argument
                .as_ref()
                .is_some_and(|argument| !matches!(argument, FieldArgument::Text(_)))
        {
            return Err(Error::Other(format!(
                "{} source contains malformed switch \\{}",
                instruction.name, switch.name
            )));
        }
    }
    Ok(())
}

fn generated_collation_supported(
    document: &Document,
    definition: &GeneratedTableDefinition,
    sources: &[GeneratedTableSource],
) -> bool {
    sources
        .iter()
        .filter(|source| generated_source_matches(document, definition, source))
        .all(|source| match &source.kind {
            GeneratedSourceKind::Index(entry) => entry.levels.iter().all(|level| level.is_ascii()),
            GeneratedSourceKind::Authority(entry) => entry.long_citation.is_ascii(),
            GeneratedSourceKind::Caption { .. } => true,
        })
}

fn generated_source_matches(
    document: &Document,
    definition: &GeneratedTableDefinition,
    source: &GeneratedTableSource,
) -> bool {
    match (definition, &source.kind) {
        (GeneratedTableDefinition::Index { options, .. }, GeneratedSourceKind::Index(entry)) => {
            entry.identifier == options.identifier
        }
        (GeneratedTableDefinition::Authorities(options), GeneratedSourceKind::Authority(entry)) => {
            Some(entry.category) == options.category
        }
        (
            GeneratedTableDefinition::Figures {
                options, styles, ..
            },
            GeneratedSourceKind::Caption { label, style, .. },
        ) => {
            if styles.is_empty() {
                label.as_deref() == Some(options.label.as_str())
            } else {
                style.as_deref().is_some_and(|id| {
                    styles.iter().any(|(name, _)| {
                        name.eq_ignore_ascii_case(id)
                            || document
                                .styles
                                .get_by_id(id)
                                .and_then(|style| style.name.as_deref())
                                .is_some_and(|actual| actual.eq_ignore_ascii_case(name))
                    })
                })
            }
        }
        _ => false,
    }
}

fn generated_render_table(
    document: &Document,
    owner: &GeneratedTableOwner,
    sources: &[GeneratedTableSource],
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
    report: &mut GeneratedTablesReport,
) -> Result<Vec<u8>> {
    let section_formats = section_page_formats(&document.document);
    let selected = sources
        .iter()
        .filter(|source| generated_source_matches(document, &owner.definition, source))
        .collect::<Vec<_>>();
    match &owner.definition {
        GeneratedTableDefinition::Index {
            options,
            sequence,
            cross_reference_separator,
        } => generated_render_index(
            &section_formats,
            owner,
            &selected,
            options,
            sequence.as_deref(),
            cross_reference_separator,
            snapshot,
            report,
        ),
        GeneratedTableDefinition::Authorities(options) => generated_render_authorities(
            &section_formats,
            owner,
            &selected,
            options,
            snapshot,
            report,
        ),
        GeneratedTableDefinition::Figures {
            options,
            sequence,
            page_prefix_separator,
            ..
        } => {
            let mut output = Vec::new();
            for source in &selected {
                let GeneratedSourceKind::Caption { runs, tail, .. } = &source.kind else {
                    continue;
                };
                let mut paragraph = generated_entry_paragraph(owner, "TableofFigures", 1);
                paragraph.runs = if options.include_label_and_number {
                    runs.clone()
                } else {
                    tail.clone()
                };
                let label_end = paragraph.runs.len();
                generated_append_separator(&mut paragraph, &options.entry_page_separator);
                paragraph.runs.extend(generated_page_run(
                    &section_formats,
                    source,
                    None,
                    snapshot,
                    sequence.as_deref(),
                    page_prefix_separator,
                    false,
                    false,
                )?);
                if options.hyperlink {
                    let end = label_end;
                    paragraph
                        .hyperlinks
                        .push(oxoffice_docx_oxml::text::HyperlinkSpan {
                            rel_id: None,
                            anchor: Some(source.target.clone()),
                            tooltip: None,
                            doc_location: None,
                            run_start: 0,
                            run_end: end,
                            extra_attributes: Vec::new(),
                            extra_xml: Vec::new(),
                            preserved_raw_before: None,
                        });
                }
                generated_write_paragraph(&paragraph, &mut output)?;
            }
            if selected.is_empty() {
                let mut paragraph = generated_entry_paragraph(owner, "TableofFigures", 1);
                paragraph.add_run("No table of figures entries found.");
                generated_write_paragraph(&paragraph, &mut output)?;
            }
            if snapshot.is_some() {
                report.figure_entries += selected.len();
            }
            Ok(output)
        }
    }
}

fn generated_entry_paragraph(owner: &GeneratedTableOwner, style: &str, _level: usize) -> CT_P {
    let mut paragraph = CT_P::new();
    let mut properties = owner.properties.clone();
    properties.style_id = Some(style.into());
    properties.num_id = None;
    properties.num_ilvl = None;
    properties.outline_lvl = None;
    if properties.tabs.is_none() {
        properties.tabs = Some(oxoffice_docx_oxml::borders::CT_Tabs {
            tabs: vec![oxoffice_docx_oxml::borders::CT_TabStop {
                val: ST_TabJc::Right,
                pos: oxoffice_docx_oxml::units::Twips(9350),
                leader: Some(oxoffice_docx_oxml::shared::ST_TabLeader::Dot),
                source_occurrence: None,
            }],
        });
    }
    paragraph.properties = Some(properties);
    paragraph
}

fn generated_append_separator(paragraph: &mut CT_P, separator: &str) {
    let mut run = CT_R::new("");
    let segments = separator.split('\t').collect::<Vec<_>>();
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            run.content.push(RunContent::Tab);
        }
        if !segment.is_empty() {
            run.content.push(RunContent::Text(CT_Text::new(segment)));
        }
    }
    paragraph.runs.push(run);
}

fn generated_write_paragraph(paragraph: &CT_P, output: &mut Vec<u8>) -> Result<()> {
    let mut writer = quick_xml::Writer::new(Vec::new());
    paragraph.to_xml(&mut writer)?;
    let xml = writer.into_inner();
    output.extend_from_slice(&xml_fragment_with_namespaces(
        &xml,
        &BTreeMap::from([("w".into(), W_NS.into())]),
        "generated entry paragraph",
    )?);
    Ok(())
}

fn generated_page_run(
    section_formats: &[Option<String>],
    source: &GeneratedTableSource,
    last: Option<&str>,
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
    sequence: Option<&str>,
    prefix_separator: &str,
    bold: bool,
    italic: bool,
) -> Result<Vec<CT_R>> {
    let target = last.unwrap_or(&source.target);
    let mut properties = source.properties.clone().unwrap_or_default();
    properties.vanish = Some(false);
    if bold {
        properties.bold = Some(true);
        properties.bold_cs = Some(true);
    }
    if italic {
        properties.italic = Some(true);
        properties.italic_cs = Some(true);
    }
    let mut runs = Vec::new();
    let mut run = CT_R::new("");
    run.properties = Some(properties.clone());
    let value = if let Some(snapshot) = snapshot {
        let page = snapshot.bookmark_page_section(target).ok_or_else(|| {
            Error::Other(format!(
                "generated page target {target} is missing, ambiguous or unplaced"
            ))
        })?;
        let prefix = sequence
            .map(|identifier| {
                (if last.is_some() {
                    &source.end_sequence_values
                } else {
                    &source.sequence_values
                })
                .get(&identifier.to_ascii_lowercase())
                .copied()
                .ok_or_else(|| {
                    Error::Other(format!(
                        "generated sequence prefix {identifier} has no source decision"
                    ))
                })
            })
            .transpose()?;
        let label = oxoffice_docx_layout::engine::format_page_field_result(
            &FieldInstruction::new(
                "PAGEREF",
                vec![FieldArgument::Text(target.into())],
                Vec::new(),
            )?,
            page.displayed_page,
            section_formats
                .get(page.section_index)
                .and_then(Option::as_deref),
        )
        .map_err(Error::Other)?;
        // The chapter prefix goes before the field, as Word writes it for a
        // `TOC \s`: the page reference's result is the page alone.
        if let Some(prefix) = prefix {
            let mut before = CT_R::new(&format!("{prefix}{prefix_separator}"));
            before.properties = Some(properties.clone());
            runs.push(before);
        }
        label
    } else {
        "99".into()
    };
    let mut cached = CT_R::new(&value);
    cached.properties = Some(properties);
    let field = Field::from_instruction(
        FieldInstruction::new(
            "PAGEREF",
            vec![FieldArgument::Text(target.into())],
            Vec::new(),
        )?,
        oxoffice_docx_oxml::text::FieldForm::Simple,
        vec![cached],
    )?;
    run.content = vec![RunContent::Field(field)];
    runs.push(run);
    Ok(runs)
}

fn generated_collation(left: &[String], right: &[String]) -> std::cmp::Ordering {
    for (left, right) in left.iter().zip(right) {
        let folded = left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase());
        if folded != std::cmp::Ordering::Equal {
            return folded;
        }
        // The pinned en-US captures place the distinct lowercase entry before its uppercase counterpart.
        let case = right.cmp(left);
        if case != std::cmp::Ordering::Equal {
            return case;
        }
    }
    left.len().cmp(&right.len())
}

fn generated_append_pages(
    section_formats: &[Option<String>],
    paragraph: &mut CT_P,
    occurrences: &[&GeneratedTableSource],
    page_separator: &str,
    range_separator: &str,
    sequence: Option<&str>,
    passim: bool,
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    for source in occurrences {
        if let Some(snapshot) = snapshot {
            let first = snapshot
                .bookmark_page_section(&source.target)
                .ok_or_else(|| {
                    Error::Other(format!("generated target {} is unplaced", source.target))
                })?;
            let last = source
                .last_target
                .as_deref()
                .map(|target| {
                    snapshot.bookmark_page_section(target).ok_or_else(|| {
                        Error::Other(format!("generated range endpoint {target} is unplaced"))
                    })
                })
                .transpose()?;
            if last.is_some_and(|last| last.physical_page < first.physical_page) {
                return Err(Error::Other(
                    "generated range endpoint precedes its start".into(),
                ));
            }
            let key = (first.physical_page, last.map(|last| last.physical_page));
            if !seen.insert(key) {
                continue;
            }
        }
        selected.push(*source);
    }
    if passim
        && snapshot.is_some()
        && selected
            .iter()
            .filter(|source| source.last_target.is_none())
            .count()
            >= 5
    {
        paragraph.add_run("passim");
        return Ok(());
    }
    for (index, source) in selected.iter().enumerate() {
        if index != 0 {
            generated_append_separator(paragraph, page_separator);
        }
        let (bold, italic) = match &source.kind {
            GeneratedSourceKind::Index(entry) => {
                (entry.bold_page_numbers, entry.italic_page_numbers)
            }
            GeneratedSourceKind::Authority(entry) => {
                (entry.bold_page_numbers, entry.italic_page_numbers)
            }
            _ => (false, false),
        };
        paragraph.runs.extend(generated_page_run(
            section_formats,
            source,
            None,
            snapshot,
            sequence,
            "-",
            bold,
            italic,
        )?);
        if let Some(last) = &source.last_target {
            generated_append_separator(paragraph, range_separator);
            paragraph.runs.extend(generated_page_run(
                section_formats,
                source,
                Some(last),
                snapshot,
                sequence,
                "-",
                bold,
                italic,
            )?);
        }
    }
    Ok(())
}

fn generated_render_index(
    section_formats: &[Option<String>],
    owner: &GeneratedTableOwner,
    selected: &[&GeneratedTableSource],
    options: &IndexOptions,
    sequence: Option<&str>,
    cross_reference_separator: &str,
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
    report: &mut GeneratedTablesReport,
) -> Result<Vec<u8>> {
    let mut groups = BTreeMap::<Vec<String>, Vec<&GeneratedTableSource>>::new();
    for source in selected {
        let GeneratedSourceKind::Index(entry) = &source.kind else {
            continue;
        };
        for length in 1..=entry.levels.len() {
            groups.entry(entry.levels[..length].to_vec()).or_default();
        }
        groups.entry(entry.levels.clone()).or_default().push(source);
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|left, right| generated_collation(&left.0, &right.0));
    let mut output = Vec::new();
    let mut previous_heading = None;
    let mut run_in = None::<CT_P>;
    let mut root = None::<String>;
    let mut previous_depth = 0usize;
    for (levels, occurrences) in &groups {
        let heading = levels
            .first()
            .and_then(|level| level.chars().next())
            .map(|letter| letter.to_ascii_uppercase());
        if options.heading_separator.is_some() && previous_heading != heading {
            if let Some(paragraph) = run_in.take() {
                generated_write_paragraph(&paragraph, &mut output)?;
            }
            let mut paragraph = generated_entry_paragraph(owner, "IndexHeading", 1);
            paragraph.add_run(&heading.map(|letter| letter.to_string()).unwrap_or_default());
            generated_write_paragraph(&paragraph, &mut output)?;
            previous_heading = heading;
        }
        let same_root = root.as_deref() == levels.first().map(String::as_str);
        let mut paragraph = if options.run_in && same_root {
            let mut paragraph = run_in
                .take()
                .ok_or_else(|| Error::Other("run-in index lost its parent".into()))?;
            generated_append_separator(
                &mut paragraph,
                if previous_depth == 1 && levels.len() == 2 {
                    ": "
                } else {
                    "; "
                },
            );
            paragraph
        } else {
            if let Some(paragraph) = run_in.take() {
                generated_write_paragraph(&paragraph, &mut output)?;
            }
            generated_entry_paragraph(
                owner,
                &format!("Index{}", if options.run_in { 1 } else { levels.len() }),
                if options.run_in { 1 } else { levels.len() },
            )
        };
        let mut label = CT_R::new(levels.last().map_or("", String::as_str));
        label.properties = occurrences
            .first()
            .and_then(|source| source.properties.clone());
        if let Some(properties) = &mut label.properties {
            properties.vanish = Some(false);
        }
        let label_start = paragraph.runs.len();
        if options.hyperlink {
            label
                .properties
                .get_or_insert_with(CT_RPr::default)
                .style_id = Some("Hyperlink".into());
        }
        paragraph.runs.push(label);
        if options.hyperlink
            && let Some(source) = occurrences.iter().find(|source| !source.target.is_empty())
        {
            paragraph
                .hyperlinks
                .push(oxoffice_docx_oxml::text::HyperlinkSpan {
                    rel_id: None,
                    anchor: Some(source.target.clone()),
                    tooltip: None,
                    doc_location: None,
                    run_start: label_start,
                    run_end: label_start + 1,
                    extra_attributes: Vec::new(),
                    extra_xml: Vec::new(),
                    preserved_raw_before: None,
                });
        }
        let cross_references = occurrences
            .iter()
            .filter_map(|source| match &source.kind {
                GeneratedSourceKind::Index(entry) => entry.cross_reference.as_deref(),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if !cross_references.is_empty() {
            generated_append_separator(&mut paragraph, cross_reference_separator);
            paragraph.add_run(
                &cross_references
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(&options.page_separator),
            );
        }
        let pages = occurrences.iter().copied().filter(|source| matches!(&source.kind, GeneratedSourceKind::Index(entry) if entry.cross_reference.is_none())).collect::<Vec<_>>();
        if !pages.is_empty() {
            generated_append_separator(&mut paragraph, &options.entry_page_separator);
            generated_append_pages(
                section_formats,
                &mut paragraph,
                &pages,
                &options.page_separator,
                &options.range_separator,
                sequence,
                false,
                snapshot,
            )?;
        }
        root = levels.first().cloned();
        previous_depth = levels.len();
        if options.run_in {
            run_in = Some(paragraph);
        } else {
            generated_write_paragraph(&paragraph, &mut output)?;
        }
    }
    if let Some(paragraph) = run_in {
        generated_write_paragraph(&paragraph, &mut output)?;
    }
    if snapshot.is_some() {
        report.index_entries += groups
            .iter()
            .filter(|(_, occurrences)| !occurrences.is_empty())
            .count();
    }
    Ok(output)
}

fn generated_render_authorities(
    section_formats: &[Option<String>],
    owner: &GeneratedTableOwner,
    selected: &[&GeneratedTableSource],
    options: &TableOfAuthoritiesOptions,
    snapshot: Option<&oxoffice_docx_layout::WordLayoutResult>,
    report: &mut GeneratedTablesReport,
) -> Result<Vec<u8>> {
    let mut groups = BTreeMap::<String, Vec<&GeneratedTableSource>>::new();
    for source in selected {
        if let GeneratedSourceKind::Authority(entry) = &source.kind {
            groups
                .entry(entry.short_citation.clone())
                .or_default()
                .push(source);
        }
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    let long = |group: &[&GeneratedTableSource]| {
        group
            .first()
            .and_then(|source| match &source.kind {
                GeneratedSourceKind::Authority(entry) => Some(entry.long_citation.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };
    groups.sort_by(|left, right| generated_collation(&[long(left)], &[long(right)]));
    let mut output = Vec::new();
    if options.include_category_headings {
        let mut paragraph = generated_entry_paragraph(owner, "TOAHeading", 1);
        paragraph.add_run(match options.category {
            Some(1) => "Cases",
            Some(2) => "Statutes",
            Some(3) => "Other Authorities",
            Some(4) => "Rules",
            Some(5) => "Treatises",
            Some(6) => "Regulations",
            Some(7) => "Constitutional Provisions",
            Some(8) => "8",
            Some(9) => "9",
            Some(10) => "10",
            Some(11) => "11",
            Some(12) => "12",
            Some(13) => "13",
            Some(14) => "14",
            Some(15) => "15",
            Some(16) => "16",
            _ => {
                return Err(Error::Other(
                    "authority heading has no numbered category".into(),
                ));
            }
        });
        generated_write_paragraph(&paragraph, &mut output)?;
    }
    for occurrences in &groups {
        let mut paragraph = generated_entry_paragraph(owner, "TableofAuthorities", 1);
        let mut label = CT_R::new(&long(occurrences));
        label.properties = occurrences
            .first()
            .and_then(|source| source.properties.clone());
        if let Some(properties) = &mut label.properties {
            properties.vanish = Some(false);
        }
        paragraph.runs.push(label);
        generated_append_separator(&mut paragraph, &options.entry_page_separator);
        generated_append_pages(
            section_formats,
            &mut paragraph,
            occurrences,
            &options.page_separator,
            &options.range_separator,
            None,
            options.use_passim,
            snapshot,
        )?;
        generated_write_paragraph(&paragraph, &mut output)?;
    }
    if snapshot.is_some() {
        report.authority_entries += groups.len();
    }
    Ok(output)
}

pub(super) fn generated_simple_span(
    xml: &[u8],
    closed: &DynamicXmlElement,
    elements: &[DynamicXmlElement],
    closing_start: usize,
    end: usize,
    policy: DynamicOwnerPolicy,
) -> Result<Option<DynamicTocSpan>> {
    if !closed.is_word
        || closed.local_name != b"fldSimple"
        || !closed
            .paragraph
            .is_some_and(|paragraph| accepted_simple_field_parent(elements, paragraph))
    {
        return Ok(None);
    }
    let Some(paragraph_index) = closed.paragraph else {
        return Ok(None);
    };
    let raw = xml_fragment_with_namespaces(
        &xml[closed.start..end],
        &closed.inherited_namespaces,
        "simple generated field",
    )?;
    let mut fragment = format!("<w:p xmlns:w=\"{W_NS}\">").into_bytes();
    fragment.extend_from_slice(&raw);
    fragment.extend_from_slice(b"</w:p>");
    let paragraph = CT_P::from_xml_fragment(&fragment)?;
    let field = accepted_toc_runs(&paragraph)
        .into_iter()
        .flat_map(|run| &run.run.content)
        .find_map(|content| match content {
            RunContent::Field(field) => Some(field.clone()),
            _ => None,
        });
    let field = if field.is_none() && policy == DynamicOwnerPolicy::Bibliography {
        // Producer attributes can keep a simple field outside the typed projection.
        // Its qualified instruction still owns source references and global selectors.
        let mut reader = NsReader::from_reader(raw.as_slice());
        let mut buffer = Vec::new();
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid bibliography simple owner: {error}")))?;
        let start = match event {
            Event::Start(start) | Event::Empty(start) => start,
            _ => return Ok(None),
        };
        let instruction = resolved_element_attribute(
            &start,
            reader.resolver(),
            b"instr",
            AttributeNamespace::Word,
        )?
        .map(|(_, value)| value);
        instruction
            .map(|instruction| {
                let mut field = Field::new(&instruction, "");
                for (name, locked) in [(b"fldLock".as_slice(), true), (b"dirty".as_slice(), false)]
                {
                    if let Some((_, value)) = resolved_element_attribute(
                        &start,
                        reader.resolver(),
                        name,
                        AttributeNamespace::Word,
                    )? {
                        let Some(value) = oxoffice_docx_oxml::shared::ST_OnOff::parse(&value)
                        else {
                            return Err(Error::Other(
                                "ambiguous bibliography simple control".into(),
                            ));
                        };
                        if locked {
                            field.set_locked(Some(value));
                        } else {
                            field.dirty = Some(value);
                        }
                    }
                }
                Ok(field)
            })
            .transpose()?
    } else {
        field
    };
    let Some(field) = field.filter(|field| generated_table_opcode(&field.instruction.name, policy))
    else {
        return Ok(None);
    };
    let parent = elements
        .iter()
        .rev()
        .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph_index))
        .ok_or_else(|| Error::Other("simple generated field has no typed paragraph".into()))?;
    let position = closed.run_position.ok_or_else(|| {
        Error::Other("simple generated field has no accepted run boundary".into())
    })?;
    Ok(Some(DynamicTocSpan {
        instruction: field.effective_instruction_text(),
        field_start: closed.start,
        field_end: end,
        begin_paragraph: paragraph_index,
        end_paragraph: paragraph_index,
        begin_run_start: closed.start,
        instruction_paragraph_start: parent.start,
        result_start: closed.start_tag_end,
        result_end: closing_start,
        result_start_position: position,
        result_end_position: position,
        end_run_end: end,
        start_paragraph_name: parent.qualified_name.clone(),
        start_paragraph_namespaces: parent.inherited_namespaces.clone(),
        separator_wrapper_names: Vec::new(),
        instruction_runs: Vec::new(),
        end_paragraph_start: parent.start,
        end_paragraph_content_start: parent.start_tag_end,
        end_wrapper_prefixes: Vec::new(),
        simple_field: Some(field),
    }))
}

fn generated_simple_cache_xml(raw: &[u8], bindings: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut copied = 0usize;
    let mut result = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid simple generated cache: {error}")))?;
        let after = reader.buffer_position() as usize;
        let complete = match event {
            Event::Start(_) => {
                if depth == 0 {
                    start = before;
                }
                depth += 1;
                false
            }
            Event::Empty(_) if depth == 0 => {
                start = before;
                true
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("simple generated cache has unmatched end".into())
                })?;
                depth == 0
            }
            Event::Eof => {
                if depth != 0 {
                    return Err(Error::Other("simple generated cache is unbalanced".into()));
                }
                result.extend_from_slice(&raw[copied..]);
                return Ok(result);
            }
            _ => false,
        };
        if complete {
            result.extend_from_slice(&raw[copied..start]);
            result.extend_from_slice(&xml_fragment_with_namespaces(
                &raw[start..after],
                bindings,
                "simple generated cache subtree",
            )?);
            copied = after;
        }
        buffer.clear();
    }
}

#[derive(Clone, Copy)]
pub(super) enum SimpleGeneratedOwnerContext<'a> {
    Tables(&'a [GeneratedTableSource]),
    Bibliography(&'a crate::bibliography::BibliographyUpdateState),
}

pub(super) fn generated_normalize_simple_tables(
    document: &mut Document,
    context: SimpleGeneratedOwnerContext<'_>,
) -> Result<()> {
    let policy = match context {
        SimpleGeneratedOwnerContext::Tables(_) => DynamicOwnerPolicy::GeneratedTables,
        SimpleGeneratedOwnerContext::Bibliography(_) => DynamicOwnerPolicy::Bibliography,
    };
    let stories = generated_story_inventory(document, policy)?;
    let mut edits = BTreeMap::<String, Vec<FieldSourceEdit>>::new();
    for story in &stories {
        for span in &story.spans {
            let Some(field) = &span.simple_field else {
                continue;
            };
            if field.locked() == Some(true) {
                continue;
            }
            match context {
                SimpleGeneratedOwnerContext::Tables(sources) => {
                    let Ok(Some(definition)) =
                        generated_table_definition(&field.effective_instruction())
                    else {
                        continue;
                    };
                    if !generated_collation_supported(document, &definition, sources) {
                        continue;
                    }
                }
                SimpleGeneratedOwnerContext::Bibliography(state) => {
                    let instruction = field.effective_instruction();
                    if instruction.name != "BIBLIOGRAPHY"
                        || state
                            .bibliography_blocks(
                                &instruction,
                                bibliography_story_text_width(document, story, span),
                            )?
                            .is_none()
                    {
                        continue;
                    }
                }
            }
            let raw = xml_fragment_with_namespaces(
                &story.xml[span.field_start..span.field_end],
                &span.start_paragraph_namespaces,
                "simple generated owner attributes",
            )?;
            let mut reader = NsReader::from_reader(raw.as_slice());
            let mut buffer = Vec::new();
            let (_, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| {
                    Error::Other(format!("invalid simple generated field: {error}"))
                })?;
            let start = match event {
                Event::Start(start) | Event::Empty(start) => start,
                _ => continue,
            };
            let mut cache_bindings = span.start_paragraph_namespaces.clone();
            let mut convertible = true;
            for attribute in start.attributes() {
                let attribute = attribute.map_err(|error| {
                    Error::Other(format!("invalid simple generated attribute: {error}"))
                })?;
                if attribute.key.as_ref() == b"xmlns"
                    || attribute.key.as_ref().starts_with(b"xmlns:")
                {
                    let prefix = attribute
                        .key
                        .as_ref()
                        .strip_prefix(b"xmlns:")
                        .map(|prefix| String::from_utf8_lossy(prefix).into_owned())
                        .unwrap_or_default();
                    let namespace = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                        .map_err(|error| {
                            Error::Other(format!("invalid simple generated namespace: {error}"))
                        })?;
                    cache_bindings.insert(prefix, namespace.into_owned());
                    continue;
                }
                let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                if !namespace_is_word(&namespace)
                    || !matches!(local.as_ref(), b"instr" | b"dirty" | b"fldLock")
                {
                    convertible = false;
                }
            }
            if !convertible {
                continue;
            }
            // Retain the exact producer instruction and result XML during form expansion.
            let mut begin = format!("<w:r xmlns:w=\"{W_NS}\"><w:fldChar w:fldCharType=\"begin\"");
            if let Some(locked) = field.locked() {
                begin.push_str(if locked {
                    " w:fldLock=\"1\""
                } else {
                    " w:fldLock=\"0\""
                });
            }
            if let Some(dirty) = field.dirty {
                begin.push_str(if dirty {
                    " w:dirty=\"1\""
                } else {
                    " w:dirty=\"0\""
                });
            }
            begin.push_str("/></w:r>");
            let mut replacement = begin.into_bytes();
            replacement.extend_from_slice(format!("<w:r xmlns:w=\"{W_NS}\"><w:instrText xml:space=\"preserve\">{}</w:instrText></w:r><w:r xmlns:w=\"{W_NS}\"><w:fldChar w:fldCharType=\"separate\"/></w:r>", xml_escape_text(&span.instruction)).as_bytes());
            replacement.extend_from_slice(&generated_simple_cache_xml(
                &story.xml[span.result_start..span.result_end],
                &cache_bindings,
            )?);
            replacement.extend_from_slice(
                format!("<w:r xmlns:w=\"{W_NS}\"><w:fldChar w:fldCharType=\"end\"/></w:r>")
                    .as_bytes(),
            );
            let offset = |value: usize| {
                value
                    .checked_sub(story.wrapper_len)
                    .and_then(|value| value.checked_add(story.range.start))
                    .ok_or_else(|| {
                        Error::Other("simple generated owner is outside its physical story".into())
                    })
            };
            edits
                .entry(story.story.part_name().into())
                .or_default()
                .push(FieldSourceEdit {
                    start: offset(span.field_start)?,
                    end: offset(span.field_end)?,
                    replacement,
                });
        }
    }
    for (part, mut edits) in edits {
        edits.sort_by_key(|edit| edit.start);
        if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(Error::Other("simple generated owners overlap".into()));
        }
        let mut xml = document
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("simple generated part disappeared".into()))?
            .to_vec();
        for edit in edits.into_iter().rev() {
            xml.splice(edit.start..edit.end, edit.replacement);
        }
        validate_story_document_declarations_and_doctype(&xml)?;
        document.package.set_part(&part, xml);
    }
    let reopened = document.clone_for_staging().reopen_prepared_staged()?;
    *document = reopened;
    Ok(())
}
