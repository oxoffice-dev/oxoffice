//! Comments and comment authors: reading, adding, anchoring, replying, moving, resolving and removing them.

use super::*;

const DEFAULT_POWERPOINT_AUTHORS_PART: &str = "/ppt/authors.xml";

impl Presentation {
    /// Returns modern PowerPoint comment authors in producer order.
    pub fn comment_authors(&self) -> &[CommentAuthor] {
        &self.comment_authors.authors
    }

    /// Relates the default authors part to the presentation when it has none.
    pub(super) fn ensure_comment_authors_part(&mut self) -> Result<()> {
        if self.comment_authors_part.is_some() {
            return Ok(());
        }
        if self.package.contains_part(DEFAULT_POWERPOINT_AUTHORS_PART) {
            return Err(Error::CollaborationPartCollision {
                part_name: DEFAULT_POWERPOINT_AUTHORS_PART.to_owned(),
            });
        }
        self.comment_authors_part = Some(DEFAULT_POWERPOINT_AUTHORS_PART.to_owned());
        let target = relative_part_target(&self.presentation_part, DEFAULT_POWERPOINT_AUTHORS_PART);
        self.package
            .get_or_create_part_rels(&self.presentation_part)
            .add(rel_types::POWERPOINT_AUTHORS, &target);
        Ok(())
    }

    /// Adds a caller-identified modern comment author atomically.
    pub fn add_comment_author(&mut self, author: CommentAuthor) -> Result<()> {
        if self
            .comment_authors
            .authors
            .iter()
            .any(|existing| existing.id == author.id)
        {
            return Err(invalid_presentation_mutation(
                "add comment author",
                format!("duplicate author id {}", author.id),
            ));
        }
        let mut staged = self.clone();
        staged.ensure_comment_authors_part()?;
        staged.comment_authors.authors.push(author);
        staged.comment_authors_dirty = true;
        self.commit_candidate(staged)
    }

    /// Returns modern comments on one slide in producer order.
    pub fn comments(&self, slide_index: usize) -> Option<&[Comment]> {
        self.slides
            .get(slide_index)
            .and_then(|slide| slide.comments.as_ref())
            .map(|comments| comments.comments.comments.as_slice())
    }

    /// Adds one caller-identified modern comment atomically.
    pub fn add_comment(&mut self, slide_index: usize, comment: Comment) -> Result<()> {
        self.require_slide_index(slide_index)?;
        self.validate_comment_identity(&comment.id, &comment.author_id)?;
        let mut staged = self.clone();
        if staged.slides[slide_index].comments.is_none() {
            // A free name, so legacy comment parts (PowerPoint 2007 to 2019
            // wrote them as `/ppt/comments/commentN.xml`) stay as they are.
            let comment_part = MediaNamer::scan(
                "/ppt/comments",
                "comment",
                staged.package.parts.keys().map(String::as_str),
            )
            .next_part_name("xml");
            let slide_part = staged.slides[slide_index].part_name.clone();
            let relationship_id = staged.package.get_or_create_part_rels(&slide_part).add(
                rel_types::POWERPOINT_COMMENTS,
                &relative_part_target(&slide_part, &comment_part),
            );
            Arc::make_mut(&mut staged.slides[slide_index])
                .slide
                .ensure_modern_comment_relationship(&relationship_id)
                .map_err(|error| Error::MalformedPart {
                    part_name: slide_part,
                    message: error.to_string(),
                })?;
            Arc::make_mut(&mut staged.slides[slide_index]).comments = Some(CommentRecord {
                part_name: comment_part,
                comments: CommentList::new(),
                dirty: true,
            });
        }
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .expect("comment record was materialized");
        comments.dirty = true;
        comments.comments.comments.push(comment);
        self.commit_candidate(staged)
    }

    /// Adds a modern comment anchored to a shape or its UTF-16 text range.
    pub fn add_comment_at_shape(
        &mut self,
        slide_index: usize,
        mut comment: Comment,
        shape_id: u32,
        text_range: Option<(usize, usize)>,
    ) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let record = &self.slides[slide_index];
        if shape_id_count(
            &record.slide.common_slide_data.shape_tree.children,
            shape_id,
        ) != 1
        {
            return Err(invalid_shape_mutation(
                "comment anchor",
                format!("shape id {shape_id} is missing or ambiguous"),
            ));
        }
        let mut monikers = Vec::new();
        let target = find_comment_anchor_shape(
            &record.slide.common_slide_data.shape_tree.children,
            shape_id,
            &mut monikers,
        )
        .expect("unique shape id was checked");
        let text_context = if let Some((start, length)) = text_range {
            let ShapeTreeChild::Shape(shape) = target else {
                return Err(invalid_shape_mutation(
                    "comment anchor",
                    "text range requires an ordinary text shape",
                ));
            };
            let text = shape
                .text_body
                .as_ref()
                .map_or_else(String::new, office_art_comment_text);
            let text_length = text.encode_utf16().count();
            if start > text_length || length > text_length - start {
                return Err(invalid_shape_mutation(
                    "comment anchor",
                    "text range exceeds shape text",
                ));
            }
            if comment_text_context_count(
                &record.slide.common_slide_data.shape_tree.children,
                &text,
            ) != 1
            {
                return Err(invalid_shape_mutation(
                    "comment anchor",
                    "text context is ambiguous on the slide",
                ));
            }
            Some(text)
        } else {
            None
        };
        comment
            .set_drawing_anchor(record.id, &monikers, text_range, text_context.as_deref())
            .map_err(|error| invalid_shape_mutation("comment anchor", error.to_string()))?;
        self.add_comment(slide_index, comment)
    }

    /// Appends one caller-identified reply to an existing comment atomically.
    pub fn reply_to_comment(
        &mut self,
        slide_index: usize,
        comment_id: &str,
        reply: CommentReply,
    ) -> Result<()> {
        self.require_slide_index(slide_index)?;
        self.validate_comment_identity(&reply.id, &reply.author_id)?;
        let mut staged = self.clone();
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    "reply to comment",
                    format!("unknown comment id {comment_id}"),
                )
            })?;
        let comment = comments
            .comments
            .comments
            .iter_mut()
            .find(|comment| comment.id == comment_id)
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    "reply to comment",
                    format!("unknown comment id {comment_id}"),
                )
            })?;
        comment.add_reply(reply).map_err(|error| {
            invalid_presentation_mutation("reply to comment", error.to_string())
        })?;
        comments.dirty = true;
        self.commit_candidate(staged)
    }

    /// Moves one modern comment to a final zero-based position.
    pub fn move_comment(&mut self, slide_index: usize, from: usize, to: usize) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let mut staged = self.clone();
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    "move comment",
                    "slide has no modern comments".to_owned(),
                )
            })?;
        comments
            .comments
            .move_comment(from, to)
            .map_err(|error| invalid_presentation_mutation("move comment", error.to_string()))?;
        comments.dirty = true;
        self.commit_candidate(staged)
    }

    /// Moves one reply within an existing comment to a final zero-based position.
    pub fn move_reply(
        &mut self,
        slide_index: usize,
        comment_id: &str,
        from: usize,
        to: usize,
    ) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let mut staged = self.clone();
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    "move comment reply",
                    format!("unknown comment id {comment_id}"),
                )
            })?;
        let comment = comments
            .comments
            .comments
            .iter_mut()
            .find(|comment| comment.id == comment_id)
            .ok_or_else(|| {
                invalid_presentation_mutation(
                    "move comment reply",
                    format!("unknown comment id {comment_id}"),
                )
            })?;
        comment.move_reply(from, to).map_err(|error| {
            invalid_presentation_mutation("move comment reply", error.to_string())
        })?;
        comments.dirty = true;
        self.commit_candidate(staged)
    }

    /// Marks one modern comment thread resolved atomically.
    ///
    /// The status is stored on the thread's top-level comment, so a reply id
    /// is rejected as unknown.
    pub fn resolve_comment(&mut self, slide_index: usize, comment_id: &str) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let mut staged = self.clone();
        let unknown = || {
            invalid_presentation_mutation(
                "resolve comment",
                format!("unknown comment id {comment_id}"),
            )
        };
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .ok_or_else(unknown)?;
        let comment = comments
            .comments
            .comments
            .iter_mut()
            .find(|comment| comment.id == comment_id)
            .ok_or_else(unknown)?;
        comment.status = Some("resolved".to_owned());
        comments.dirty = true;
        self.commit_candidate(staged)
    }

    /// Removes one modern comment with its replies, or one reply, atomically.
    ///
    /// The slide keeps its comment part when its last comment is removed.
    pub fn remove_comment(&mut self, slide_index: usize, comment_id: &str) -> Result<()> {
        self.require_slide_index(slide_index)?;
        let mut staged = self.clone();
        let unknown = || {
            invalid_presentation_mutation(
                "remove comment",
                format!("unknown comment id {comment_id}"),
            )
        };
        let comments = Arc::make_mut(&mut staged.slides[slide_index])
            .comments
            .as_mut()
            .ok_or_else(unknown)?;
        let list = &mut comments.comments.comments;
        if let Some(index) = list.iter().position(|comment| comment.id == comment_id) {
            list.remove(index);
        } else if !list
            .iter_mut()
            .any(|comment| comment.remove_reply(comment_id))
        {
            return Err(unknown());
        }
        comments.dirty = true;
        self.commit_candidate(staged)
    }
}

impl Presentation {
    fn validate_comment_identity(&self, id: &str, author_id: &str) -> Result<()> {
        if !self
            .comment_authors
            .authors
            .iter()
            .any(|author| author.id == author_id)
        {
            return Err(invalid_presentation_mutation(
                "mutate comments",
                format!("unknown comment author id {author_id}"),
            ));
        }
        if self.slides.iter().any(|slide| {
            slide.comments.as_ref().is_some_and(|comments| {
                comments.comments.comments.iter().any(|comment| {
                    comment.id == id || comment.replies().iter().any(|reply| reply.id == id)
                })
            })
        }) {
            return Err(invalid_presentation_mutation(
                "mutate comments",
                format!("duplicate modern comment id {id}"),
            ));
        }
        Ok(())
    }
}

fn find_comment_anchor_shape<'a>(
    children: &'a [ShapeTreeChild],
    shape_id: u32,
    monikers: &mut Vec<(&'static str, u32)>,
) -> Option<&'a ShapeTreeChild> {
    for child in children {
        let id = child.non_visual_id();
        let kind = match child {
            ShapeTreeChild::Shape(_) => Some("spMk"),
            ShapeTreeChild::Picture(_) => Some("picMk"),
            ShapeTreeChild::GraphicFrame(_) => Some("graphicFrameMk"),
            ShapeTreeChild::GroupShape(_) => Some("grpSpMk"),
            ShapeTreeChild::Connector(_) => Some("cxnSpMk"),
            ShapeTreeChild::AlternateContent(_) => None,
        };
        if id == Some(shape_id) {
            monikers.push((kind?, shape_id));
            return Some(child);
        }
        if let ShapeTreeChild::GroupShape(group) = child
            && let Some(group_id) = id
        {
            monikers.push(("grpSpMk", group_id));
            if let Some(found) = find_comment_anchor_shape(&group.children, shape_id, monikers) {
                return Some(found);
            }
            monikers.pop();
        }
    }
    None
}

fn office_art_comment_text(body: &CT_TextBody) -> String {
    body.paragraphs()
        .iter()
        .map(|paragraph| {
            paragraph
                .runs
                .iter()
                .map(|run| match run {
                    TextRun::Run(run) => run.text.value.as_str(),
                    TextRun::Break(_) => "\u{000b}",
                    TextRun::Field(field) => {
                        field.text.as_ref().map_or("", |text| text.value.as_str())
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\r")
}

fn comment_text_context_count(children: &[ShapeTreeChild], text: &str) -> usize {
    children
        .iter()
        .map(|child| match child {
            ShapeTreeChild::Shape(shape) => usize::from(
                shape
                    .text_body
                    .as_ref()
                    .is_some_and(|body| office_art_comment_text(body) == text),
            ),
            ShapeTreeChild::GroupShape(group) => comment_text_context_count(&group.children, text),
            _ => 0,
        })
        .sum()
}
