//! A formula as stored and as displayed.
//!
//! Each conversion edits tokens of the formula read into its structure and
//! leaves the others as written, whitespace included. A stored formula gives
//! back its exact text after a trip through its displayed form:
//! `to_stored(&to_displayed(s)) == s`. That is what [`check_stored`] asks of
//! a stored formula, and what keeps a cell read then written unchanged.

use crate::SyntaxError;
use crate::functions::{Prefix, function, stored_function};
use crate::lexer::TokenKind;
use crate::parser::{Node, Parsed, parse};
use crate::reference::{Target, target};

/// The prefixes of LET and LAMBDA parameters: `_xlpm.` on declarations and
/// uses, `_xlop.` on an optional parameter's declaration (MS-XLSX
/// `lambda-parameter-name`), kept as written.
const PARAMETER: &str = "_xlpm.";
const OPTIONAL_PARAMETER: &str = "_xlop.";

/// The displayed form of a stored formula, as Excel shows it after its `=`:
///
/// - a future function loses its prefix (`_xlfn.XLOOKUP` is `XLOOKUP`,
///   `_xlfn._xlws.SORT` is `SORT`); a prefixed name the function table lacks
///   keeps it, so that it comes back unchanged;
/// - LET and LAMBDA parameters lose `_xlpm.`, where they are declared and
///   where they are used in their scope;
/// - `_xlfn.SINGLE(x)` is `@x` (`@(x)` when `x` is not a reference or a
///   call) and `_xlfn.ANCHORARRAY(x)` is `x#` when `x` is a cell or a name.
///
/// Table references stay as stored (`Expenses[[#This Row],[Quantity]]`):
/// Excel shows `[@Quantity]` inside the table, which depends on where the
/// cell is. A formula this crate cannot read comes back as it is.
pub fn to_displayed(stored: &str) -> String {
    let Ok(parsed) = parse(stored, false) else {
        return stored.to_owned();
    };
    let mut edits = Edits::new(parsed.tokens.len());
    each(&parsed.root, &mut |node| {
        displayed(&parsed, node, &mut edits)
    });
    parameters(
        &parsed,
        &parsed.root,
        &mut Vec::new(),
        Form::Displayed,
        &mut edits,
    );
    edits.apply(&parsed)
}

/// The stored form of a displayed formula, after the syntax check: the
/// conversions of [`to_displayed`] the other way, and `Table[@Column]`
/// (Excel's displayed form of the current row) written
/// `Table[[#This Row],[Column]]`. A leading `=` is dropped.
///
/// Refused, with a plain message: what cannot be read; `;` outside an array
/// constant (Excel's separator in some locales: write `,`); an operator
/// without its operands; unbalanced brackets; a built-in function given too
/// few or too many arguments; more than 255 arguments, 64 nested functions
/// or 8,192 characters; an array constant holding more than constants, or
/// rows of different lengths; a LET or LAMBDA declaring what is not a name.
pub fn to_stored(displayed: &str) -> Result<String, SyntaxError> {
    let (text, shift) = match displayed.strip_prefix('=') {
        Some(rest) => (rest, 1),
        None => (displayed, 0),
    };
    let parsed = parse(text, true).map_err(|error| error.shifted(displayed, shift))?;
    let mut edits = Edits::new(parsed.tokens.len());
    each(&parsed.root, &mut |node| stored(&parsed, node, &mut edits));
    parameters(
        &parsed,
        &parsed.root,
        &mut Vec::new(),
        Form::Stored,
        &mut edits,
    );
    Ok(edits.apply(&parsed))
}

/// Checks a formula given in its stored form: the syntax check of
/// [`to_stored`], and the stored names: a future function with its prefix,
/// LET and LAMBDA parameters with theirs, `_xlfn.SINGLE` for `@`. The
/// message of a formula not in its stored form gives the stored form.
pub fn check_stored(stored: &str) -> Result<(), SyntaxError> {
    parse(stored, true)?;
    let canonical = to_stored(&to_displayed(stored))?;
    if canonical == stored {
        return Ok(());
    }
    let differs = stored
        .char_indices()
        .zip(canonical.chars())
        .find(|((_, left), right)| left != right)
        .map_or(stored.len().min(canonical.len()), |((offset, _), _)| offset);
    Err(SyntaxError::new(
        stored,
        differs,
        format!("Excel stores this formula as `{canonical}`"),
    ))
}

/// The text each token is written as: replaced, and with text before and
/// after it.
struct Edits {
    replace: Vec<Option<String>>,
    before: Vec<String>,
    after: Vec<String>,
}

impl Edits {
    fn new(tokens: usize) -> Self {
        Edits {
            replace: vec![None; tokens],
            before: vec![String::new(); tokens],
            after: vec![String::new(); tokens],
        }
    }

    fn apply(&self, parsed: &Parsed<'_>) -> String {
        let mut text = String::with_capacity(parsed.text.len() + 16);
        for index in 0..parsed.tokens.len() {
            text.push_str(&self.before[index]);
            text.push_str(
                self.replace[index]
                    .as_deref()
                    .unwrap_or_else(|| parsed.token(index)),
            );
            text.push_str(&self.after[index]);
        }
        text
    }
}

/// Calls `visit` on `node` and every node under it.
fn each(node: &Node, visit: &mut impl FnMut(&Node)) {
    visit(node);
    match node {
        Node::Leaf(_) | Node::Array { .. } => {}
        Node::Prefix { operand, .. } | Node::Postfix { operand, .. } => each(operand, visit),
        Node::Operation { first, rest } => {
            each(first, visit);
            for (_, node) in rest {
                each(node, visit);
            }
        }
        Node::Paren { inner, .. } => each(inner, visit),
        Node::Call { args, .. } => {
            for arg in args.iter().flatten() {
                each(arg, visit);
            }
        }
        Node::Invoke { callee, args, .. } => {
            each(callee, visit);
            for arg in args.iter().flatten() {
                each(arg, visit);
            }
        }
    }
}

/// Whether `node` is what `@` takes in the displayed form without
/// parentheses: an operand or a range (`@A1:A10`, `@INDEX(...)`). Not a
/// parenthesized one: `@(x)` gives its parentheses to `_xlfn.SINGLE`.
fn is_range_operand(parsed: &Parsed<'_>, node: &Node) -> bool {
    match node {
        Node::Leaf(_) | Node::Call { .. } | Node::Invoke { .. } | Node::Array { .. } => true,
        Node::Postfix { op, .. } => parsed.token(*op) == "#",
        Node::Prefix { op, .. } => parsed.token(*op) == "@",
        Node::Operation { rest, .. } => rest.iter().all(|(op, _)| parsed.token(*op) == ":"),
        Node::Paren { .. } => false,
    }
}

/// Whether a spill `#` can follow the token at `index`: a cell or a name.
fn spills(parsed: &Parsed<'_>, index: usize) -> bool {
    match parsed.kind(index) {
        TokenKind::Name => true,
        TokenKind::Reference => matches!(target(parsed.token(index)), Some(Target::Cell { .. })),
        _ => false,
    }
}

fn displayed(parsed: &Parsed<'_>, node: &Node, edits: &mut Edits) {
    let Node::Call {
        name,
        open,
        args,
        close,
    } = node
    else {
        return;
    };
    let (name, open, close) = (*name, *open, *close);
    let text = parsed.token(name);
    let Some(function) = stored_function(text) else {
        return;
    };
    let one = match args.as_slice() {
        [Some(arg)] => Some(arg),
        _ => None,
    };
    // What would join the operand of `@` or `#` once the call's brackets go:
    // a range (`@A1:B2` takes `B2` too) or a call of the result.
    let joins = parsed.next_significant(close).is_some_and(|next| {
        parsed.token(next) == ":"
            || (next == close + 1 && parsed.kind(next) == TokenKind::OpenParen)
    });
    match (function.name, one) {
        ("SINGLE", Some(arg)) if !joins => {
            edits.replace[name] = Some("@".to_owned());
            let tight = arg.first() == open + 1 && arg.last() + 1 == close;
            if tight && is_range_operand(parsed, arg) {
                edits.replace[open] = Some(String::new());
                edits.replace[close] = Some(String::new());
            }
        }
        // Right after `:`, `Data:Sheet2!B2#` would read as a 3D reference.
        ("ANCHORARRAY", Some(Node::Leaf(leaf)))
            if !joins
                && *leaf == open + 1
                && leaf + 1 == close
                && spills(parsed, *leaf)
                && !(name > 0
                    && parsed.token(name - 1) == ":"
                    && parsed.token(*leaf).contains('!')) =>
        {
            edits.replace[name] = Some(String::new());
            edits.replace[open] = Some(String::new());
            edits.replace[close] = Some("#".to_owned());
        }
        _ if function.prefix != Prefix::None => {
            edits.replace[name] = Some(text[function.prefix.as_str().len()..].to_owned());
        }
        _ => {}
    }
}

fn stored(parsed: &Parsed<'_>, node: &Node, edits: &mut Edits) {
    match node {
        Node::Prefix { op, operand } if parsed.token(*op) == "@" => match operand.as_ref() {
            Node::Paren { open, .. } if *open == op + 1 => {
                edits.replace[*op] = Some("_xlfn.SINGLE".to_owned());
            }
            _ => {
                edits.replace[*op] = Some("_xlfn.SINGLE(".to_owned());
                edits.after[operand.last()].push(')');
            }
        },
        Node::Postfix { operand, op } if parsed.token(*op) == "#" => {
            edits.before[operand.first()].insert_str(0, "_xlfn.ANCHORARRAY(");
            edits.replace[*op] = Some(")".to_owned());
        }
        Node::Call { name, .. } => {
            let text = parsed.token(*name);
            if stored_function(text).is_none()
                && let Some(function) = function(text)
                && function.prefix != Prefix::None
            {
                edits.replace[*name] = Some(format!("{}{text}", function.prefix.as_str()));
            }
        }
        Node::Leaf(index) if parsed.kind(*index) == TokenKind::StructuredReference => {
            if let Some(written) = this_row(parsed.token(*index)) {
                edits.replace[*index] = Some(written);
            }
        }
        _ => {}
    }
}

/// `Table[@Column]` written as stored, `Table[[#This Row],[Column]]`
/// (also `[@[Unit price]]`, `[@[A]:[B]]` and `[@]`); `None` for any other
/// table reference.
fn this_row(reference: &str) -> Option<String> {
    // The bracket part starts after a workbook prefix (`[1]!`) and the
    // table name.
    let start = match reference.strip_prefix('[') {
        Some(_) if reference.contains("]!") => {
            let after = reference.find("]!")? + 2;
            after + reference[after..].find('[')?
        }
        Some(_) => 0,
        None => reference.find('[')?,
    };
    let inner = reference[start..].strip_prefix("[@")?.strip_suffix(']')?;
    let table = &reference[..start];
    Some(if inner.trim().is_empty() {
        format!("{table}[#This Row]")
    } else if inner.starts_with('[') {
        format!("{table}[[#This Row],{inner}]")
    } else {
        format!("{table}[[#This Row],[{inner}]]")
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Displayed,
    Stored,
}

/// A parameter name split from its prefix, and the key it is compared by
/// (names compare in any case).
fn parameter(text: &str) -> (&str, &str, String) {
    for prefix in [PARAMETER, OPTIONAL_PARAMETER] {
        if let Some(bare) = text.strip_prefix(prefix) {
            return (prefix, bare, bare.to_lowercase());
        }
    }
    ("", text, text.to_lowercase())
}

/// Whether the token at `index` is a name without a sheet or workbook
/// prefix: what LET and LAMBDA declare and their scope uses.
fn plain_name(parsed: &Parsed<'_>, index: usize) -> bool {
    parsed.kind(index) == TokenKind::Name && !parsed.token(index).contains('!')
}

/// A declared parameter: `_xlpm.` shown in the displayed form and taken in
/// the stored one.
fn declare(parsed: &Parsed<'_>, index: usize, form: Form, edits: &mut Edits) -> String {
    let (prefix, bare, key) = parameter(parsed.token(index));
    match form {
        Form::Displayed if prefix == PARAMETER => edits.replace[index] = Some(bare.to_owned()),
        Form::Stored if prefix.is_empty() => {
            edits.replace[index] = Some(format!("{PARAMETER}{bare}"))
        }
        _ => {}
    }
    key
}

/// A name used where `scope` holds the parameters in force.
fn use_name(parsed: &Parsed<'_>, index: usize, scope: &[String], form: Form, edits: &mut Edits) {
    let (prefix, bare, key) = parameter(parsed.token(index));
    if !scope.contains(&key) {
        return;
    }
    match form {
        Form::Displayed if prefix == PARAMETER => edits.replace[index] = Some(bare.to_owned()),
        Form::Stored if prefix.is_empty() => {
            edits.replace[index] = Some(format!("{PARAMETER}{bare}"))
        }
        _ => {}
    }
}

/// The LET and LAMBDA parameters of `node` and their uses. A LET name is in
/// scope from the value after its own to the calculation; a LAMBDA parameter
/// in the calculation; a call of the result (`LAMBDA(x,x)(1)`) is outside.
fn parameters(
    parsed: &Parsed<'_>,
    node: &Node,
    scope: &mut Vec<String>,
    form: Form,
    edits: &mut Edits,
) {
    match node {
        Node::Leaf(index) if plain_name(parsed, *index) => {
            use_name(parsed, *index, scope, form, edits)
        }
        Node::Leaf(_) | Node::Array { .. } => {}
        Node::Call { name, args, .. } => {
            let callee = function(parsed.token(*name)).map(|function| function.name);
            let base = scope.len();
            let count = args.len();
            match callee {
                Some("LET") => {
                    let mut pending = None;
                    for (index, arg) in args.iter().enumerate() {
                        if index + 1 < count
                            && index % 2 == 0
                            && let Some(Node::Leaf(leaf)) = arg
                            && plain_name(parsed, *leaf)
                        {
                            pending = Some(declare(parsed, *leaf, form, edits));
                            continue;
                        }
                        if let Some(arg) = arg {
                            parameters(parsed, arg, scope, form, edits);
                        }
                        if let Some(key) = pending.take() {
                            scope.push(key);
                        }
                    }
                }
                Some("LAMBDA") => {
                    for (index, arg) in args.iter().enumerate() {
                        if index + 1 < count
                            && let Some(Node::Leaf(leaf)) = arg
                            && plain_name(parsed, *leaf)
                        {
                            let key = declare(parsed, *leaf, form, edits);
                            scope.push(key);
                            continue;
                        }
                        if let Some(arg) = arg {
                            parameters(parsed, arg, scope, form, edits);
                        }
                    }
                }
                callee => {
                    // A parameter holding a LAMBDA is called by its name.
                    if callee.is_none() {
                        use_name(parsed, *name, scope, form, edits);
                    }
                    for arg in args.iter().flatten() {
                        parameters(parsed, arg, scope, form, edits);
                    }
                }
            }
            scope.truncate(base);
        }
        Node::Invoke { callee, args, .. } => {
            parameters(parsed, callee, scope, form, edits);
            for arg in args.iter().flatten() {
                parameters(parsed, arg, scope, form, edits);
            }
        }
        Node::Prefix { operand, .. } | Node::Postfix { operand, .. } => {
            parameters(parsed, operand, scope, form, edits)
        }
        Node::Operation { first, rest } => {
            parameters(parsed, first, scope, form, edits);
            for (_, node) in rest {
                parameters(parsed, node, scope, form, edits);
            }
        }
        Node::Paren { inner, .. } => parameters(parsed, inner, scope, form, edits),
    }
}
