//! The comments of a sheet, read only: notes, and Excel 365's threaded
//! comments with their replies and authors.
//!
//! Notes come from the `comments` part related to the sheet (its VML
//! shapes are kept, not read); threads from `threadedComments`, named by
//! `persons/person.xml`. Excel writes a copy of each thread among the notes
//! for older versions, its author `tc={thread id}`: that copy is not listed
//! a second time. Comments are ordered by cell, row by row; replies by
//! date.

use oxoffice_xlsx_oxml::CellRef;
use oxoffice_xlsx_oxml::comments::{Note, Person, ThreadedComment};

use crate::{Error, Result};

/// A note or a comment thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentKind {
    /// A legacy comment: one text, no replies.
    Note,
    /// A threaded comment, with its replies.
    Thread,
}

impl CommentKind {
    /// `note` or `thread`.
    pub fn as_str(self) -> &'static str {
        match self {
            CommentKind::Note => "note",
            CommentKind::Thread => "thread",
        }
    }
}

/// A note or a thread on a cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub address: CellRef,
    pub kind: CommentKind,
    /// The name of whoever wrote it; `None` when the file names nobody
    /// (Google's notes).
    pub author: Option<String>,
    /// When it was written, ISO 8601 as stored: threads only.
    pub date: Option<String>,
    pub text: String,
    /// The thread is resolved; a note never is.
    pub resolved: bool,
    /// The replies of a thread, by date.
    pub replies: Vec<Reply>,
}

/// A reply in a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub author: Option<String>,
    pub date: Option<String>,
    pub text: String,
}

/// The cell a comment of `part` is on.
fn address(part: &str, reference: &str) -> Result<CellRef> {
    CellRef::parse(reference).ok_or_else(|| Error::MalformedPart {
        part: part.to_owned(),
        message: format!("a comment is on `{reference}`, not a cell"),
    })
}

fn author(name: &str) -> Option<String> {
    (!name.is_empty()).then(|| name.to_owned())
}

/// The comments of a sheet from its notes and threads (each with the part
/// it was read from), threads named by `persons`.
pub(crate) fn merge(
    notes: Option<(String, Vec<Note>)>,
    threads: Option<(String, Vec<ThreadedComment>)>,
    persons: &[Person],
) -> Result<Vec<Comment>> {
    let mut comments = Vec::new();
    let mut thread_ids = Vec::new();
    if let Some((part, threads)) = threads {
        let person = |id: &Option<String>| {
            let id = id.as_deref()?;
            persons
                .iter()
                .find(|person| person.id.eq_ignore_ascii_case(id))
                .and_then(|person| author(&person.display_name))
        };
        for root in threads.iter().filter(|comment| comment.parent_id.is_none()) {
            let mut replies: Vec<&ThreadedComment> = threads
                .iter()
                .filter(|reply| {
                    reply
                        .parent_id
                        .as_deref()
                        .is_some_and(|parent| parent.eq_ignore_ascii_case(&root.id))
                })
                .collect();
            replies.sort_by(|a, b| a.date.cmp(&b.date));
            thread_ids.push(root.id.to_ascii_lowercase());
            comments.push(Comment {
                address: address(&part, &root.reference)?,
                kind: CommentKind::Thread,
                author: person(&root.person_id),
                date: root.date.clone(),
                text: root.text.clone(),
                resolved: root.done,
                replies: replies
                    .into_iter()
                    .map(|reply| Reply {
                        author: person(&reply.person_id),
                        date: reply.date.clone(),
                        text: reply.text.clone(),
                    })
                    .collect(),
            });
        }
    }
    if let Some((part, notes)) = notes {
        for note in notes {
            let copy_of = note
                .author
                .strip_prefix("tc=")
                .map(|id| id.to_ascii_lowercase());
            if copy_of.is_some_and(|id| thread_ids.contains(&id)) {
                continue;
            }
            comments.push(Comment {
                address: address(&part, &note.reference)?,
                kind: CommentKind::Note,
                author: author(&note.author),
                date: None,
                text: note.text,
                resolved: false,
                replies: Vec::new(),
            });
        }
    }
    comments.sort_by_key(|comment| comment.address);
    Ok(comments)
}
