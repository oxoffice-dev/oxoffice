//! A comment or a reply created without a date is dated now, UTC to the
//! second; an explicit date is written as given, and `CommentDate::Undated`
//! writes none, on the Word and Google Docs files of the corpus.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use oxoffice_docx::{CommentDate, Document, RunPosition, RunRange};

const PRODUCERS: [&str; 2] = ["word", "google"];
const STAMP: &str = "2026-01-02T03:04:05Z";

fn report(producer: &str) -> Document {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx");
    Document::open(path).expect("corpus report opens")
}

/// The first run of the summary paragraph, the second body item.
fn summary_run() -> RunRange {
    RunRange {
        start: RunPosition {
            body_index: 1,
            run_index: 0,
        },
        end: RunPosition {
            body_index: 1,
            run_index: 1,
        },
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The Unix time of an RFC 3339 timestamp in UTC to the second,
/// `YYYY-MM-DDTHH:MM:SSZ`, the only form a comment dated now takes.
fn unix_seconds(date: &str) -> u64 {
    let bytes = date.as_bytes();
    assert!(
        date.len() == 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[19] == b'Z',
        "{date:?} is not YYYY-MM-DDTHH:MM:SSZ"
    );
    let field = |range: std::ops::Range<usize>| date[range].parse::<i64>().unwrap();
    let (year, month, day) = (field(0..4), field(5..7), field(8..10));
    // Days from 1970-01-01 to the civil date (Howard Hinnant's algorithm).
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + field(11..13) * 3_600 + field(14..16) * 60 + field(17..19))
        .unwrap()
}

/// A copy of `doc` through its saved bytes.
fn copy(doc: &mut Document) -> Document {
    Document::from_bytes(&doc.to_bytes().unwrap()).unwrap()
}

/// The date of comment `id` once `doc` is saved and reopened.
fn date_of(doc: &mut Document, id: i32) -> Option<String> {
    copy(doc)
        .comments()
        .into_iter()
        .find(|comment| comment.id() == id)
        .unwrap_or_else(|| panic!("comment {id} is in the saved document"))
        .date()
        .map(str::to_owned)
}

fn assert_dated_now(date: Option<String>, before: u64, after: u64, what: &str) {
    let date = date.unwrap_or_else(|| panic!("{what} has no date"));
    let seconds = unix_seconds(&date);
    assert!(
        before <= seconds && seconds <= after,
        "{what}: {date} is not between {before} and {after}"
    );
}

#[test]
fn comments_and_replies_created_without_a_date_are_dated_now() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let before = now();
        let ranged = doc
            .add_comment(summary_run(), "Reviewer", None, "On a range")
            .unwrap();
        let on_text = doc
            .add_comment_on_text(
                "Summary",
                0,
                "Reviewer",
                None,
                "On a text",
                CommentDate::Now,
            )
            .unwrap();
        let reply = doc.reply_to(ranged, "Author", "A reply").unwrap();
        let after = now();
        for (id, what) in [(ranged, "range"), (on_text, "text"), (reply, "reply")] {
            assert_dated_now(
                date_of(&mut doc, id),
                before,
                after,
                &format!("{producer} {what}"),
            );
        }
    }
}

#[test]
fn an_explicit_date_is_written_as_given() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let ranged = doc
            .add_comment_with_date(summary_run(), "Reviewer", None, "c", CommentDate::At(STAMP))
            .unwrap();
        let on_text = doc
            .add_comment_on_text("Summary", 0, "Reviewer", None, "c", CommentDate::At(STAMP))
            .unwrap();
        let reply = doc
            .reply_to_with_date(ranged, "Author", "r", CommentDate::At(STAMP))
            .unwrap();
        for id in [ranged, on_text, reply] {
            assert_eq!(date_of(&mut doc, id).as_deref(), Some(STAMP), "{producer}");
        }
    }
}

#[test]
fn an_undated_comment_or_reply_carries_no_date() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let ranged = doc
            .add_comment_with_date(summary_run(), "Reviewer", None, "c", CommentDate::Undated)
            .unwrap();
        let on_text = doc
            .add_comment_on_text("Summary", 0, "Reviewer", None, "c", CommentDate::Undated)
            .unwrap();
        let reply = doc
            .reply_to_with_initials(ranged, "Author", Some("A"), "r", CommentDate::Undated)
            .unwrap();
        for id in [ranged, on_text, reply] {
            assert_eq!(date_of(&mut doc, id), None, "{producer}");
        }
    }
}

#[test]
fn an_invalid_date_is_refused_and_changes_nothing() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let before = doc.to_bytes().unwrap();
        for date in ["yesterday", "2026-13-01T00:00:00Z"] {
            assert!(
                doc.add_comment_with_date(summary_run(), "R", None, "c", CommentDate::At(date))
                    .is_err()
            );
            assert!(
                doc.add_comment_on_text("Summary", 0, "R", None, "c", CommentDate::At(date))
                    .is_err()
            );
        }
        assert_eq!(doc.to_bytes().unwrap(), before, "{producer}");
    }
}

#[test]
fn comparison_keeps_an_undated_reply_undated() {
    // One comparison only: each costs seconds in an unoptimised build. A
    // dated reply replays its own date, as before `CommentDate`.
    let mut original = report("word");
    let parent = original.comments()[0].id();
    let mut edited = copy(&mut original);
    let reply = edited
        .reply_to_with_date(parent, "Author", "A reply", CommentDate::Undated)
        .unwrap();
    let mut redline = copy(&mut original);
    redline
        .compare(&edited, "Comparer", "2026-02-01T00:00:00Z")
        .unwrap();
    assert_eq!(date_of(&mut redline, reply), None);
}

/// The first run of the default header's paragraph.
fn header_run(doc: &Document) -> oxoffice_docx::StoryRunRange {
    let header = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == oxoffice_docx::StoryKind::Header)
        .expect("a header story");
    let paragraph = doc.story_items(&header).unwrap()[0].location().clone();
    oxoffice_docx::StoryRunRange {
        start: oxoffice_docx::StoryRunPosition {
            location: paragraph.clone(),
            run_index: 0,
        },
        end: oxoffice_docx::StoryRunPosition {
            location: paragraph,
            run_index: 1,
        },
    }
}

#[test]
fn a_comment_in_a_header_is_dated_like_one_in_the_body() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let before = now();
        let dated_now = doc
            .add_story_comment(header_run(&doc), "Reviewer", None, "In the header")
            .unwrap();
        let after = now();
        assert_dated_now(
            date_of(&mut doc, dated_now),
            before,
            after,
            &format!("{producer} header"),
        );
        let range = header_run(&doc);
        let dated = doc
            .add_story_comment_with_date(range, "R", None, "c", CommentDate::At(STAMP))
            .unwrap();
        assert_eq!(
            date_of(&mut doc, dated).as_deref(),
            Some(STAMP),
            "{producer}"
        );
        let range = header_run(&doc);
        let undated = doc
            .add_story_comment_with_date(range, "R", None, "c", CommentDate::Undated)
            .unwrap();
        assert_eq!(date_of(&mut doc, undated), None, "{producer}");
    }
}

#[test]
fn an_invalid_date_on_a_story_comment_or_a_reply_is_refused_and_changes_nothing() {
    for producer in PRODUCERS {
        let mut doc = report(producer);
        let parent = doc.comments()[0].id();
        let before = doc.to_bytes().unwrap();
        let range = header_run(&doc);
        assert!(
            doc.add_story_comment_with_date(range, "R", None, "c", CommentDate::At("noon"))
                .is_err()
        );
        assert!(
            doc.reply_to_with_date(parent, "A", "r", CommentDate::At("noon"))
                .is_err()
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{producer}");
    }
}
