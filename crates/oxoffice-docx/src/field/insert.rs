//! Inserting sequence, caption, cross-reference and plain fields.

use super::*;
use oxoffice_docx_oxml::text::bookmark_name_key;

/// Switches governing one authored sequence field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceOptions {
    pub restart: Option<i64>,
    pub repeat: bool,
    pub hidden: bool,
    pub restart_heading: Option<u8>,
    pub format: Option<String>,
}

/// Caption content and its three bookmark targets.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionOptions {
    pub label: String,
    pub text: String,
    pub bookmark: String,
    pub sequence: SequenceOptions,
    pub separator: String,
    pub properties: Option<CT_PPr>,
}

impl Default for CaptionOptions {
    fn default() -> Self {
        Self {
            label: "Figure".into(),
            text: String::new(),
            bookmark: "Caption".into(),
            sequence: SequenceOptions::default(),
            separator: ": ".into(),
            properties: None,
        }
    }
}

/// Checked caption location and the actual allocated bookmark names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionTarget {
    pub paragraph: crate::ContentLocation,
    pub entire_caption: String,
    pub label_and_number: String,
    pub number: String,
}

/// Numbering context requested by a cross-reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrossReferenceNumber {
    #[default]
    Text,
    Level,
    Relative,
    FullContext,
}

/// Checked REF switches, including typed note copying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrossReferenceOptions {
    pub number: CrossReferenceNumber,
    pub position: bool,
    pub hyperlink: bool,
    pub omit_non_numeric_text: bool,
    pub delimiter: Option<String>,
    pub copy_referenced_notes: bool,
}

impl Document {
    /// Insert a dynamic sequence field at a checked accepted run boundary.
    pub fn insert_sequence(
        &mut self,
        position: &crate::StoryRunPosition,
        identifier: &str,
        options: &SequenceOptions,
    ) -> Result<()> {
        let instruction = sequence_instruction(identifier, options)?;
        self.insert_checked_story_field(position, instruction)
    }

    /// Insert a dynamic cross-reference without flattening its target.
    pub fn insert_cross_reference(
        &mut self,
        position: &crate::StoryRunPosition,
        bookmark: &str,
        options: &CrossReferenceOptions,
    ) -> Result<()> {
        validate_reference_name(bookmark)?;
        let mut switches = Vec::new();
        let number = match options.number {
            CrossReferenceNumber::Text => None,
            CrossReferenceNumber::Level => Some("n"),
            CrossReferenceNumber::Relative => Some("r"),
            CrossReferenceNumber::FullContext => Some("w"),
        };
        switches.extend(number.map(|name| field_option_switch(name, None)));
        for (enabled, name) in [
            (options.position, "p"),
            (options.hyperlink, "h"),
            (options.omit_non_numeric_text, "t"),
            (options.copy_referenced_notes, "f"),
        ] {
            if enabled {
                switches.push(field_option_switch(name, None));
            }
        }
        if let Some(delimiter) = &options.delimiter {
            switches.push(field_option_switch("d", Some(delimiter.clone())));
        }
        let instruction =
            FieldInstruction::new("REF", vec![FieldArgument::Text(bookmark.into())], switches)?;
        self.insert_checked_story_field(position, instruction)
    }

    pub(crate) fn insert_checked_story_field(
        &mut self,
        position: &crate::StoryRunPosition,
        instruction: FieldInstruction,
    ) -> Result<()> {
        let field = Field::from_instruction(
            instruction,
            oxoffice_docx_oxml::text::FieldForm::Complex,
            Vec::new(),
        )?;
        self.insert_field(position, field)
    }

    /// Insert a field at one checked physical run boundary without replacing its owner.
    pub fn insert_field(&mut self, position: &crate::StoryRunPosition, field: Field) -> Result<()> {
        field
            .validate_for_attachment()
            .map_err(|error| Error::Other(error.to_string()))?;
        let mut run = CT_R::new("");
        run.content = vec![RunContent::Field(field)];
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.insert_story_run_staged(position, run)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// [`Self::insert_field`] on a direct body paragraph of the main part
    /// (`direct_body_paragraph_at`, `block` found by the caller),
    /// swapped in place under a `BodyBlocks` unit: `Some(changed)`. `None`, changing nothing, sends the
    /// caller to [`Self::insert_field`].
    #[doc(hidden)]
    pub fn insert_field_in_block(
        &mut self,
        position: &crate::StoryRunPosition,
        block: crate::ControlBlock,
        field: Field,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = field
            .validate_for_attachment()
            .map_err(|error| Error::Other(error.to_string()))
            .and_then(|()| {
                let mut run = CT_R::new("");
                run.content = vec![RunContent::Field(field.clone())];
                self.insert_story_run_in_block(position, block, run)
            });
        #[cfg(debug_assertions)]
        match &result {
            Ok(None) => assert!(
                self.clone_for_staging().to_bytes().ok()
                    == before.clone_for_staging().to_bytes().ok(),
                "a field insertion left to the staged route changed the document"
            ),
            _ => self.assert_in_place_matches_staged_declarations_aside(
                before,
                &result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| Error::Other(error.to_string())),
                |staged| {
                    let position = crate::StoryRunPosition {
                        location: staged.fingerprinted(&position.location),
                        run_index: position.run_index,
                    };
                    staged.insert_field(&position, field)
                },
            ),
        }
        result
    }

    /// Insert a caption and allocate separate whole, label and number targets.
    pub fn insert_caption(
        &mut self,
        destination: &crate::ContentLocation,
        options: &CaptionOptions,
    ) -> Result<CaptionTarget> {
        let instruction = sequence_instruction(&options.label, &options.sequence)?;
        validate_reference_name(&options.bookmark)?;
        oxoffice_core::xml::reject_non_xml_characters("caption text", &options.text)?;
        oxoffice_core::xml::reject_non_xml_characters("caption separator", &options.separator)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let mut names = caption_bookmark_names(&candidate)?;
        let mut allocated = Vec::new();
        for suffix in ["", "_Label", "_Number"] {
            let base = format!("{}{suffix}", options.bookmark);
            validate_reference_name(&base)?;
            let mut name = base.clone();
            let mut counter = 1u32;
            while names.contains(&bookmark_name_key(&name)) {
                name = format!("{base}_{counter}");
                validate_reference_name(&name)?;
                counter = counter
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("caption bookmark names exhausted".into()))?;
            }
            names.insert(bookmark_name_key(&name));
            allocated.push(name);
        }
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        let mut paragraph = CT_P::new();
        paragraph.properties = options.properties.clone();
        paragraph.add_run(&format!("{} ", options.label));
        let mut run = CT_R::new("");
        run.content = vec![RunContent::Field(Field::from_instruction(
            instruction,
            oxoffice_docx_oxml::text::FieldForm::Complex,
            Vec::new(),
        )?)];
        paragraph.runs.push(run);
        paragraph.add_run(&format!("{}{}", options.separator, options.text));
        for (name, start, end) in [
            (&allocated[0], 0, 3),
            (&allocated[1], 0, 2),
            (&allocated[2], 1, 2),
        ] {
            let id = candidate.identifiers.reserve_bookmark_id()?;
            paragraph
                .anchor_accepted_range(
                    Some(start),
                    Some(end),
                    oxoffice_docx_oxml::text::RangeAnchor::Bookmark { id, name },
                )
                .map_err(|error| Error::Other(format!("caption target: {error}")))?;
        }
        let index = destination
            .index_path()
            .first()
            .copied()
            .unwrap_or(candidate.story_items(destination.story())?.len());
        candidate.insert_content(destination, crate::ContentFragment::paragraph(paragraph)?)?;
        let story = candidate
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind() == destination.story().kind()
                    && story.part_name() == destination.story().part_name()
                    && story.owner_index() == destination.story().owner_index()
            })
            .ok_or_else(|| Error::Other("caption story disappeared after insertion".into()))?;
        let target = CaptionTarget {
            paragraph: crate::ContentLocation::new(
                story,
                crate::StoryItemKind::Paragraph,
                vec![index],
            ),
            entire_caption: allocated[0].clone(),
            label_and_number: allocated[1].clone(),
            number: allocated[2].clone(),
        };
        candidate.story_ranges()?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(target)
    }
}

pub(super) fn field_option_switch(
    name: &str,
    argument: Option<String>,
) -> oxoffice_docx_oxml::text::FieldSwitch {
    oxoffice_docx_oxml::text::FieldSwitch {
        name: name.into(),
        argument: argument.map(FieldArgument::Text),
    }
}

pub(super) fn validate_reference_name(name: &str) -> Result<()> {
    let mut characters = name.chars();
    if name.len() > 40
        || !characters
            .next()
            .is_some_and(|value| value.is_ascii_alphabetic() || value == '_')
        || !characters.all(|value| value.is_ascii_alphanumeric() || value == '_')
    {
        return Err(Error::Other(format!("invalid Word reference name {name}")));
    }
    Ok(())
}

fn sequence_instruction(identifier: &str, options: &SequenceOptions) -> Result<FieldInstruction> {
    validate_reference_name(identifier)?;
    if !identifier.as_bytes()[0].is_ascii_alphabetic() {
        return Err(Error::Other(
            "SEQ identifier must begin with a letter".into(),
        ));
    }
    if options.repeat && options.restart.is_some() {
        return Err(Error::Other(
            "SEQ repeat and restart are mutually exclusive".into(),
        ));
    }
    if options
        .restart_heading
        .is_some_and(|level| !(1..=9).contains(&level))
    {
        return Err(Error::Other("SEQ heading level must be 1 through 9".into()));
    }
    let mut switches = Vec::new();
    for (enabled, name) in [(options.repeat, "c"), (options.hidden, "h")] {
        if enabled {
            switches.push(field_option_switch(name, None));
        }
    }
    if let Some(restart) = options.restart {
        switches.push(field_option_switch("r", Some(restart.to_string())));
    }
    if let Some(level) = options.restart_heading {
        switches.push(field_option_switch("s", Some(level.to_string())));
    }
    if let Some(format) = &options.format {
        switches.push(field_option_switch("*", Some(format.clone())));
    }
    let instruction = FieldInstruction::new(
        "SEQ",
        vec![FieldArgument::Text(identifier.into())],
        switches,
    )?;
    oxoffice_docx_layout::engine::format_numeric_field_general(&instruction, "1")
        .map_err(Error::Other)?;
    Ok(instruction)
}

/// The bookmark names of every part, keyed by `bookmark_name_key`: a new
/// name must differ from each in more than case.
pub(super) fn caption_bookmark_names(document: &Document) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    for (part_name, xml) in &document.package.parts {
        if !document
            .package
            .content_types
            .content_type_for(part_name)
            .is_some_and(|kind| kind.ends_with("xml"))
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            let (namespace, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("caption bookmark inventory: {error}")))?;
            let word = namespace_is_word(&namespace);
            match event {
                Event::Start(element) | Event::Empty(element)
                    if word && element.local_name().as_ref() == b"bookmarkStart" =>
                {
                    if let Some((_, name)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"name",
                        AttributeNamespace::Word,
                    )? {
                        names.insert(bookmark_name_key(&name));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(names)
}
