//! A shared formula's member: the master's formula with its relative
//! references moved by the member's offset.

use crate::SyntaxError;
use crate::lexer::{TokenKind, tokenize};
use crate::reference::{MAX_COLUMNS, MAX_ROWS, Part, Target, column_letters, target, target_start};

/// The formula `formula` becomes in a cell `rows` below and `columns` to the
/// right of the cell it is written for (negative: above, to the left), as
/// the members of a shared formula read their master's.
///
/// Relative rows and columns move; `$` ones stay; whole columns keep their
/// rows and whole rows their columns. A reference a part of which leaves the
/// grid becomes `#REF!` (after its sheet prefix, which stays). Texts, names,
/// table references and error values are not references and stay as they
/// are, as does every reference this move leaves where it was, spelling
/// included.
///
/// ```
/// use oxoffice_xlsx_formula::move_references;
///
/// assert_eq!(move_references("SUM(B2:B5)*$F$1+Rates!C$3", 2, 1)?, "SUM(C4:C7)*$F$1+Rates!D$3");
/// assert_eq!(move_references("A1+1", -1, 0)?, "#REF!+1");
/// # Ok::<(), oxoffice_xlsx_formula::SyntaxError>(())
/// ```
pub fn move_references(formula: &str, rows: i64, columns: i64) -> Result<String, SyntaxError> {
    let tokens = tokenize(formula)?;
    let mut moved = String::with_capacity(formula.len());
    for token in &tokens {
        let text = &formula[token.span.clone()];
        if token.kind == TokenKind::Reference
            && let Some(target) = target(text)
        {
            moved.push_str(&move_reference(text, &target, rows, columns));
        } else {
            moved.push_str(text);
        }
    }
    Ok(moved)
}

/// A part moved by `by`, or `None` when it leaves the grid; absolute parts
/// stay.
fn moved(part: &Part, by: i64, limit: u32) -> Option<u32> {
    if part.absolute || by == 0 {
        return Some(part.index);
    }
    let index = i64::from(part.index) + by;
    (0..i64::from(limit))
        .contains(&index)
        .then_some(index as u32)
}

fn move_reference(text: &str, target: &Target, rows: i64, columns: i64) -> String {
    // Each moved part: its span in `text` and its new spelling.
    let mut changes: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let column = |part: &Part, changes: &mut Vec<_>| -> Option<()> {
        let index = moved(part, columns, MAX_COLUMNS)?;
        if index != part.index {
            changes.push((part.span.clone(), column_letters(index)));
        }
        Some(())
    };
    let row = |part: &Part, changes: &mut Vec<_>| -> Option<()> {
        let index = moved(part, rows, MAX_ROWS)?;
        if index != part.index {
            changes.push((part.span.clone(), (index + 1).to_string()));
        }
        Some(())
    };
    let inside = match target {
        Target::Cell { column: c, row: r } => {
            column(c, &mut changes).and_then(|()| row(r, &mut changes))
        }
        Target::Area { first, last } => column(&first.0, &mut changes)
            .and_then(|()| row(&first.1, &mut changes))
            .and_then(|()| column(&last.0, &mut changes))
            .and_then(|()| row(&last.1, &mut changes)),
        Target::Columns(first, last) => {
            column(first, &mut changes).and_then(|()| column(last, &mut changes))
        }
        Target::Rows(first, last) => {
            row(first, &mut changes).and_then(|()| row(last, &mut changes))
        }
        Target::Deleted => Some(()),
    };
    if inside.is_none() {
        return format!("{}#REF!", &text[..target_start(text)]);
    }
    let mut result = text.to_owned();
    // From the end, so the earlier spans stay where they are.
    changes.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for (span, spelling) in changes {
        result.replace_range(span, &spelling);
    }
    result
}
