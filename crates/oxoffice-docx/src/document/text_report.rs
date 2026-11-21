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
        let mut main_part = None;
        let mut stories: Vec<StoryText> = Vec::new();
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
            if stories.last().is_none_or(|last| &last.story != story) {
                stories.push(StoryText {
                    story: story.clone(),
                    items: Vec::new(),
                });
            }
            let kind = item.location().item_kind();
            if !matches!(
                kind,
                StoryItemKind::Paragraph | StoryItemKind::ContentControl
            ) {
                continue;
            }
            if item.is_direct_child()
                && let Some(last) = stories.last_mut()
            {
                last.items.push(StoryTextItem {
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
                .text()
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
