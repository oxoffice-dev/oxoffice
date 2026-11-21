//! The text of every story as one report, with what could not be read.

use super::*;

/// The accepted-view text of a story that the body view leaves out, or of a
/// stretch of it: [`Document::other_story_texts`] splits a story where a
/// table's cells or a text box come in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryText {
    /// The story the items belong to.
    pub story: StoryId,
    /// Each direct paragraph and block content control of the story in that
    /// stretch, in reading order.
    pub items: Vec<StoryTextItem>,
}

/// One direct paragraph or block content control of a [`StoryText`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryTextItem {
    /// The index path of the item inside its story.
    pub index_path: Vec<usize>,
    /// [`StoryItemKind::Paragraph`] or [`StoryItemKind::ContentControl`].
    pub kind: StoryItemKind,
    /// The accepted-view text of the item.
    pub text: String,
}

/// A diagnostic accompanying an owned accepted text projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiagnostic {
    pub message: String,
}

/// The CLI-compatible text presentation and its explicit source scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextReport {
    pub text: String,
    pub scope: &'static str,
    pub revision_view: &'static str,
    pub diagnostics: Vec<TextDiagnostic>,
}

pub(super) const STORY_TEXT_RELATIONSHIPS: [&str; 5] = [
    rel_types::HEADER,
    rel_types::FOOTER,
    rel_types::FOOTNOTES,
    rel_types::ENDNOTES,
    rel_types::COMMENTS,
];

impl Document {
    /// Read every story but the main body and its table cells, which the body
    /// view already covers, in reading order: the stories of each part in
    /// [`Document::stories`] order of their parts, and inside a part each item
    /// where it sits in the part's XML. A table's cells come where the table
    /// sits among its story's paragraphs, and a text box after the paragraph
    /// that anchors it, so a story interrupted by a table or a text box gives
    /// several entries. A block content control is one item, so the cells of
    /// a table inside it come after the control's line. The first entry of a part is its first story, empty
    /// when that story starts with a table. The cells of a table in a body
    /// text box are read too: the body view leaves text boxes out.
    ///
    /// Only the direct items of a story are read, so the text of an inline
    /// control or a field is not repeated after its paragraph. A table is left
    /// out because each of its cells is a table-cell story of its own.
    ///
    /// A story part that cannot be read, such as a truncated header or a
    /// header relationship to a missing part, returns the reason, naming the
    /// first part that is not well-formed XML when there is one. `validate`
    /// reports the same part as an error.
    pub fn other_story_texts(&self) -> std::result::Result<Vec<StoryText>, String> {
        // A part cut short inside its text still opens, as an empty story:
        // it is named instead of left out without a word.
        if let Some(malformed) = self.malformed_story_part() {
            return Err(malformed);
        }
        self.read_other_story_texts()
            .map_err(|error| error.to_string())
    }

    fn read_other_story_texts(&self) -> Result<Vec<StoryText>> {
        let offsets = self.story_item_offsets()?;
        let offsets_of = |story: &StoryId| {
            offsets
                .get(&(story.kind, story.part_name.clone(), story.owner_index))
                .ok_or_else(|| Error::Other("story owner was not inventoried".to_owned()))
        };
        // The text boxes of each part: the cells of a table in a body text box
        // are read, those of a body table are not.
        let text_boxes: Vec<(&str, &Range<usize>)> = offsets
            .iter()
            .filter(|((kind, _, _), _)| *kind == StoryKind::TextBox)
            .map(|((_, part, _), (span, _))| (part.as_str(), span))
            .collect();
        let mut body_cells: HashMap<StoryId, bool> = HashMap::new();
        let mut main_part = None;
        // Each part in order, with its first story and its items by offset.
        type PartItems = Vec<(usize, StoryId, StoryTextItem)>;
        let mut parts: Vec<(StoryId, PartItems)> = Vec::new();
        for item in self.story_item_snapshots()? {
            let story = item.location().story();
            if story.kind() == StoryKind::Body {
                main_part = Some(story.part_name().to_owned());
                continue;
            }
            if story.kind() == StoryKind::TableCell
                && main_part.as_deref() == Some(story.part_name())
            {
                // A body table's cell, unless the table sits in a text box.
                let body_cell = match body_cells.get(story) {
                    Some(body_cell) => *body_cell,
                    None => {
                        let start = offsets_of(story)?.0.start;
                        let body_cell = !text_boxes.iter().any(|(part, span)| {
                            *part == story.part_name() && span.contains(&start)
                        });
                        body_cells.insert(story.clone(), body_cell);
                        body_cell
                    }
                };
                if body_cell {
                    continue;
                }
            }
            if parts
                .last()
                .is_none_or(|(first, _)| first.part_name() != story.part_name())
            {
                parts.push((story.clone(), Vec::new()));
            }
            let kind = item.location().item_kind();
            if !matches!(
                kind,
                StoryItemKind::Paragraph | StoryItemKind::ContentControl
            ) || !item.is_direct_child()
            {
                continue;
            }
            let index_path = item.location().index_path().to_vec();
            let offset = index_path
                .first()
                .and_then(|index| offsets_of(story).ok()?.1.get(*index).copied())
                .ok_or_else(|| Error::Other("story item was not inventoried".to_owned()))?;
            if let Some((_, items)) = parts.last_mut() {
                items.push((
                    offset,
                    story.clone(),
                    StoryTextItem {
                        index_path,
                        kind,
                        text: item.text().unwrap_or_default().to_owned(),
                    },
                ));
            }
        }
        let mut stories: Vec<StoryText> = Vec::new();
        for (first, mut items) in parts {
            stories.push(StoryText {
                story: first,
                items: Vec::new(),
            });
            items.sort_by_key(|(offset, _, _)| *offset);
            for (_, story, item) in items {
                match stories.last_mut() {
                    Some(last) if last.story == story => last.items.push(item),
                    _ => stories.push(StoryText {
                        story,
                        items: vec![item],
                    }),
                }
            }
        }
        Ok(stories)
    }

    /// Where each story owner spans in its part, and where each of its items
    /// starts, by the index the item snapshots give it.
    #[allow(clippy::type_complexity)]
    fn story_item_offsets(
        &self,
    ) -> Result<HashMap<(StoryKind, String, usize), (Range<usize>, Vec<usize>)>> {
        let mut offsets = HashMap::new();
        for source in self.story_sources()? {
            let xml = source.xml.as_ref();
            let owners = scan_story_owners(xml, source.root_kind)?;
            let scopes =
                story_namespace_scopes_at(xml, owners.iter().map(|owner| owner.full.start))?;
            for owner in owners {
                let key = (owner.kind, source.part_name.clone(), owner.owner_index);
                if offsets.contains_key(&key) {
                    continue;
                }
                let scope = scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                let items = scan_story_items_with_scope(xml, &owner, scope, None)?;
                let starts = items.iter().map(|item| item.full.start).collect();
                offsets.insert(key, (owner.full.clone(), starts));
            }
        }
        Ok(offsets)
    }

    /// Name the first story part of the main document that is not
    /// well-formed XML, with the reason.
    fn malformed_story_part(&self) -> Option<String> {
        let package = &self.package;
        let doc_part = package.main_document_part()?;
        let rels = package.get_part_rels(&doc_part)?;
        rels.items
            .iter()
            .filter(|rel| {
                rel.target_mode.as_deref() != Some("External")
                    && STORY_TEXT_RELATIONSHIPS.contains(&rel.rel_type.as_str())
            })
            .find_map(|rel| {
                let part_name = OpcPackage::resolve_rel_target(&doc_part, &rel.target);
                let bytes = package.get_part(&part_name)?;
                // Only XML that is not well-formed, as a part cut short is:
                // markup the reader handles is not refused here.
                if xml_is_complete(bytes) {
                    return None;
                }
                let detail = document_style_references(bytes).err().map_or_else(
                    || "it ends or nests wrongly".to_owned(),
                    |error| error.to_string(),
                );
                Some(format!("part {part_name} is not well-formed XML: {detail}"))
            })
    }

    /// Present accepted body text followed by readable other stories.
    /// If any other story is malformed, return main-only text and the CLI warning.
    /// A table in a header, a footer, a note or a comment is reported where
    /// it sits among the story's paragraphs, as the body's tables are.
    pub fn text_report(&self) -> TextReport {
        let mut text = self.text();
        let mut diagnostics = Vec::new();
        let (stories, scope) = match self.other_story_texts() {
            Ok(stories) => (stories, "all-supported-stories"),
            Err(reason) => {
                diagnostics.push(TextDiagnostic {
                    message: format!("other stories left out: {reason}"),
                });
                (Vec::new(), "main")
            }
        };
        for (kind, part_name, texts) in document_story_parts(&stories) {
            text.push_str(&format!(
                "--- {} ({part_name}) ---\n",
                document_story_kind_name(kind)
            ));
            for line in texts {
                text.push_str(line);
                text.push('\n');
            }
        }
        TextReport {
            text,
            scope,
            revision_view: "accepted",
            diagnostics,
        }
    }

    /// Count Unicode scalars in the same accepted body paragraphs as word_count.
    /// No presentation separators or story labels contribute characters.
    pub fn character_count(&self, include_spaces: bool) -> usize {
        let mut count = 0;
        visit_body_paragraphs(&self.document.body.content, true, &mut |paragraph| {
            count += paragraph
                .accepted_text()
                .chars()
                .filter(|character| include_spaces || !character.is_whitespace())
                .count();
        });
        count
    }
}
/// Group the texts of the stories by package part, each part labelled with
/// the kind of its first story, such as a header part holding table cells.
/// A part without any text, such as an empty header variant, is left out.
fn document_story_parts(stories: &[StoryText]) -> Vec<(StoryKind, &str, Vec<&str>)> {
    let mut parts: Vec<(StoryKind, &str, Vec<&str>)> = Vec::new();
    for story in stories {
        let part_name = story.story.part_name();
        if parts.last().is_none_or(|(_, last, _)| *last != part_name) {
            parts.push((story.story.kind(), part_name, Vec::new()));
        }
        if let Some((_, _, texts)) = parts.last_mut() {
            texts.extend(story.items.iter().map(|item| item.text.as_str()));
        }
    }
    parts.retain(|(_, _, texts)| texts.iter().any(|text| !text.trim().is_empty()));
    parts
}

fn document_story_kind_name(kind: StoryKind) -> &'static str {
    match kind {
        StoryKind::Body => "body",
        StoryKind::TableCell => "table_cell",
        StoryKind::Header => "header",
        StoryKind::Footer => "footer",
        StoryKind::Footnote => "footnote",
        StoryKind::Endnote => "endnote",
        StoryKind::Comment => "comment",
        StoryKind::TextBox => "text_box",
    }
}

/// Whether `xml` parses and closes every element it opens: a part cut short
/// is not.
fn xml_is_complete(xml: &[u8]) -> bool {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut depth = 0usize;
    let mut elements = 0usize;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(_)) => {
                depth += 1;
                elements += 1;
            }
            Ok(quick_xml::events::Event::Empty(_)) => elements += 1,
            Ok(quick_xml::events::Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(quick_xml::events::Event::Eof) => return depth == 0 && elements > 0,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}
