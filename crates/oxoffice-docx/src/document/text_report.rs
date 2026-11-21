//! The text of every story as one report, with what could not be read.

use super::*;

/// The accepted-view text of one story that the body view leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryText {
    /// The story the items belong to.
    pub story: StoryId,
    /// Each direct paragraph and block content control of the story.
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
    /// view already covers, in [`Document::stories`] order.
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
        self.read_other_story_texts().map_err(|error| {
            self.malformed_story_part()
                .unwrap_or_else(|| error.to_string())
        })
    }

    fn read_other_story_texts(&self) -> Result<Vec<StoryText>> {
        Ok(self
            .read_other_stories()?
            .into_iter()
            .map(|story| story.text)
            .collect())
    }

    /// [`Self::read_other_story_texts`], with where each direct table of a
    /// story sits among its items.
    fn read_other_stories(&self) -> Result<Vec<OtherStory>> {
        let mut main_part = None;
        let mut stories: Vec<OtherStory> = Vec::new();
        for item in self.story_item_snapshots()? {
            let story = item.location().story();
            if story.kind() == StoryKind::Body {
                main_part = Some(story.part_name().to_owned());
                continue;
            }
            if story.kind() == StoryKind::TableCell
                && main_part.as_deref() == Some(story.part_name())
            {
                continue;
            }
            if stories.last().is_none_or(|last| &last.text.story != story) {
                stories.push(OtherStory {
                    text: StoryText {
                        story: story.clone(),
                        items: Vec::new(),
                    },
                    tables: Vec::new(),
                });
            }
            let kind = item.location().item_kind();
            if kind == StoryItemKind::Table
                && item.is_direct_child()
                && let Some(last) = stories.last_mut()
            {
                let position = last.text.items.len();
                last.tables.push((position, table_cell_count(item.xml())));
                continue;
            }
            if !matches!(
                kind,
                StoryItemKind::Paragraph | StoryItemKind::ContentControl
            ) {
                continue;
            }
            if item.is_direct_child()
                && let Some(last) = stories.last_mut()
            {
                last.text.items.push(StoryTextItem {
                    index_path: item.location().index_path().to_vec(),
                    kind,
                    text: item.text().unwrap_or_default().to_owned(),
                });
            }
        }
        Ok(stories)
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
                let detail = document_style_references(package.get_part(&part_name)?).err()?;
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
        let read = self.read_other_stories().map_err(|error| {
            self.malformed_story_part()
                .unwrap_or_else(|| error.to_string())
        });
        let (stories, scope) = match read {
            Ok(stories) => (in_reading_order(stories), "all-supported-stories"),
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
/// A story read for the text report, and each of its direct tables: the
/// count of its items before the table, and the table cells it holds.
struct OtherStory {
    text: StoryText,
    tables: Vec<(usize, usize)>,
}

/// Put the cells of each table where the table sits in its story. The cell
/// stories of a part follow the story that holds their table, in source
/// order, so the next table-cell stories of the same part, as many as the
/// table has cells, and the stories they hold, go before the items that
/// follow the table. A story is then split into the parts around its tables.
fn in_reading_order(stories: Vec<OtherStory>) -> Vec<StoryText> {
    let mut queue: std::collections::VecDeque<OtherStory> = stories.into();
    let mut ordered = Vec::new();
    while let Some(story) = queue.pop_front() {
        place_tables(story, &mut queue, &mut ordered);
    }
    ordered
}

/// Push `story` with the stories of its tables' cells in place, and return
/// how many table-cell stories that took from `queue`, nested ones included.
fn place_tables(
    story: OtherStory,
    queue: &mut std::collections::VecDeque<OtherStory>,
    ordered: &mut Vec<StoryText>,
) -> usize {
    let OtherStory {
        text: StoryText { story: id, items },
        tables,
    } = story;
    let mut items = items.into_iter();
    let mut placed = 0;
    let mut taken = 0;
    for (position, cells) in tables {
        ordered.push(StoryText {
            story: id.clone(),
            items: items.by_ref().take(position - placed).collect(),
        });
        placed = position;
        let mut counted = 0;
        while counted < cells {
            let Some(next) = queue.pop_front() else {
                break;
            };
            if next.text.story.part_name() != id.part_name() {
                queue.push_front(next);
                break;
            }
            if next.text.story.kind() == StoryKind::TableCell {
                counted += 1;
            }
            counted += place_tables(next, queue, ordered);
        }
        taken += counted;
    }
    ordered.push(StoryText {
        story: id,
        items: items.collect(),
    });
    taken
}

/// The table cells in a table's XML, those of nested tables included: the
/// story scanner makes each a table-cell story. The fallback copy of
/// alternate content is not counted, as its stories are not read twice.
fn table_cell_count(xml: &[u8]) -> usize {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut count = 0;
    let mut fallback_depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                let name = element.local_name();
                if fallback_depth > 0 || name.as_ref() == b"Fallback" {
                    fallback_depth += 1;
                } else if name.as_ref() == b"tc" {
                    count += 1;
                }
            }
            Ok(Event::Empty(element))
                if fallback_depth == 0 && element.local_name().as_ref() == b"tc" =>
            {
                count += 1;
            }
            Ok(Event::End(_)) if fallback_depth > 0 => fallback_depth -= 1,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buffer.clear();
    }
    count
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
