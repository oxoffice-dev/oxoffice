//! The comment parts of a sheet, read only: the notes (`comments`, related to
//! the sheet, `CT_Comments`), Excel 365's threaded comments
//! (`threadedComments`) and the people who wrote them (`persons/person.xml`,
//! related to the workbook part). Elements are read by their local names,
//! whatever prefix a producer gives them (Google writes `x18tc:`).

use crate::shared_strings::CT_Rst;
use crate::xml::{Element, Result};
use crate::xstring;

/// A note: the cell it is on as written (`B3`), its author's name (empty
/// when the part names none) and its text, runs joined. `uid` is
/// `xr:uid`, which Excel sets to the thread's id on the copy it writes of a
/// threaded comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub reference: String,
    pub author: String,
    pub text: String,
    pub uid: Option<String>,
}

/// The notes of a `comments` part, in the order written.
pub fn read_notes(xml: &[u8]) -> Result<Vec<Note>> {
    let root = Element::parse(xml)?;
    let authors: Vec<String> = root
        .child("authors")
        .map(|authors| {
            authors
                .elements_named("author")
                .map(|author| xstring::decode(&author.text()).into_owned())
                .collect()
        })
        .unwrap_or_default();
    let Some(list) = root.child("commentList") else {
        return Ok(Vec::new());
    };
    Ok(list
        .elements_named("comment")
        .map(|comment| Note {
            reference: comment.tag.get("ref").unwrap_or_default(),
            author: comment
                .tag
                .u32("authorId")
                .and_then(|id| authors.get(id as usize))
                .cloned()
                .unwrap_or_default(),
            text: comment
                .child("text")
                .map(|text| CT_Rst(text.clone()).plain_text())
                .unwrap_or_default(),
            uid: comment.tag.get_prefixed("uid"),
        })
        .collect())
}

/// A threaded comment (`threadedComment`), as written: a thread's first
/// comment has no `parent_id`, a reply names the first comment's `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadedComment {
    pub reference: String,
    pub id: String,
    pub parent_id: Option<String>,
    pub person_id: Option<String>,
    /// `dT`, ISO 8601 as stored.
    pub date: Option<String>,
    /// `done`: the thread is resolved.
    pub done: bool,
    pub text: String,
}

/// The threaded comments of a `threadedComments` part, in the order
/// written.
pub fn read_threaded_comments(xml: &[u8]) -> Result<Vec<ThreadedComment>> {
    let root = Element::parse(xml)?;
    Ok(root
        .elements_named("threadedComment")
        .map(|comment| ThreadedComment {
            reference: comment.tag.get("ref").unwrap_or_default(),
            id: comment.tag.get("id").unwrap_or_default(),
            parent_id: comment.tag.get("parentId"),
            person_id: comment.tag.get("personId"),
            date: comment.tag.get("dT"),
            done: comment.tag.bool("done").unwrap_or(false),
            text: comment
                .child("text")
                .map(|text| xstring::decode(&text.text()).into_owned())
                .unwrap_or_default(),
        })
        .collect())
}

/// A person of `persons/person.xml`: the id threaded comments name, and the
/// name shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub display_name: String,
}

/// The people of a `personList` part, in the order written.
pub fn read_persons(xml: &[u8]) -> Result<Vec<Person>> {
    let root = Element::parse(xml)?;
    Ok(root
        .elements_named("person")
        .map(|person| Person {
            id: person.tag.get("id").unwrap_or_default(),
            display_name: person.tag.get("displayName").unwrap_or_default(),
        })
        .collect())
}
