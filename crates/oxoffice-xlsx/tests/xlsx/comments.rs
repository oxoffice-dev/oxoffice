//! Comments read as notes and threads, by cell: a thread with its replies,
//! their authors from the persons part, and whether it is resolved; the
//! note Excel writes for each thread is not listed again.

use oxoffice_xlsx::{CommentKind, Workbook};

use crate::support::{corpus_bytes, edited};

/// A comment in one line: `cell kind author resolved: text`, each reply
/// after ` / `.
fn lines(path: &str) -> Vec<String> {
    let workbook = Workbook::from_bytes(&corpus_bytes(path)).unwrap();
    let comments = workbook.sheet("Orders").unwrap().comments().unwrap();
    comments
        .iter()
        .map(|comment| {
            let mut line = format!(
                "{} {} {}{}: {}",
                comment.address,
                comment.kind.as_str(),
                comment.author.as_deref().unwrap_or("-"),
                if comment.resolved { " resolved" } else { "" },
                comment.text
            );
            for reply in &comment.replies {
                line.push_str(&format!(
                    " / {}: {}",
                    reply.author.as_deref().unwrap_or("-"),
                    reply.text
                ));
            }
            assert_eq!(comment.date.is_some(), comment.kind == CommentKind::Thread);
            line
        })
        .collect()
}

#[test]
fn threads_and_notes_read_by_cell_with_replies_and_authors() {
    let threads = |note_author: &str| {
        vec![
            "C3 thread Corpus Author: The dates should follow the delivery calendar. / Corpus Author: Agreed, I will align them with the supplier. / Corpus Author: Done, see the updated list.".to_owned(),
            format!("E5 note {note_author}: The price differs from the quote of 12 October. Please check."),
            "B6 thread Corpus Author: Is this supplier still approved?".to_owned(),
            format!("G9 note {note_author}: Status to confirm with the supplier."),
            "F10 thread Corpus Author resolved: The total looks too high. / Corpus Author: It includes the freight.".to_owned(),
            format!("A14 note {note_author}: Is this the 140 cm model?"),
        ]
    };
    assert_eq!(lines("excel/threads.xlsx"), threads("Reviewer"));
    // Google's notes name no author.
    assert_eq!(lines("google/threads.xlsx"), threads("-"));
    let notes = threads("Reviewer")
        .into_iter()
        .filter(|line| line.contains(" note "))
        .collect::<Vec<_>>();
    for path in [
        "authored/review.xlsx",
        "libreoffice/review.xlsx",
        "excel/strict.xlsx",
    ] {
        assert_eq!(lines(path), notes, "{path}");
    }
}

#[test]
fn replies_read_by_date_whatever_their_order() {
    // The first reply of C3 dated after the second.
    let workbook = edited(
        "excel/threads.xlsx",
        "xl/threadedComments/threadedComment1.xml",
        &[(
            r#"dT="2026-11-23T09:25:00.00""#,
            r#"dT="2026-11-23T12:00:00.00""#,
        )],
    );
    let comments = workbook.sheet("Orders").unwrap().comments().unwrap();
    let replies: Vec<&str> = comments[0]
        .replies
        .iter()
        .map(|reply| reply.text.as_str())
        .collect();
    assert_eq!(
        replies,
        [
            "Done, see the updated list.",
            "Agreed, I will align them with the supplier."
        ]
    );
}
