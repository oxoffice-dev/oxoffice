//! The structure of a formula, read from its tokens, and the syntax check.
//!
//! One reader serves both forms: the stored form has `_xlfn.` names, the
//! displayed form `@` and `#`. Without the check, the reader builds the
//! structure the forms are converted on, refusing only what no Excel formula
//! is (more than 64 nested functions included); with it, it also refuses
//! what Excel would repair: a wrong argument count, more than 255 arguments
//! or 8,192 characters, an unknown error value, array rows of different
//! lengths, a LET or LAMBDA declaring what is not a name.

use crate::SyntaxError;
use crate::functions::function;
use crate::lexer::{ERROR_VALUES, Token, TokenKind, tokenize};
use crate::reference::{Target, target};

/// At most this many arguments in one call (Excel's limit).
const MAX_ARGUMENTS: usize = 255;
/// At most this many functions nested in one another (Excel's limit).
const MAX_NESTED_CALLS: usize = 64;
/// At most this many characters in a formula, counted as Excel counts them
/// (UTF-16 units).
const MAX_LENGTH: usize = 8_192;
/// How deep the reader goes before refusing: far beyond any real formula,
/// well within a thread's stack.
const MAX_DEPTH: usize = 256;

/// A part of a formula's structure. Token indexes point into the token list
/// of the formula.
#[derive(Debug)]
pub(crate) enum Node {
    /// A constant, a reference, a name or a table reference.
    Leaf(usize),
    /// `-x`, `+x`, `@x`.
    Prefix {
        op: usize,
        operand: Box<Node>,
    },
    /// `x%`, `x#`.
    Postfix {
        operand: Box<Node>,
        op: usize,
    },
    /// `x+y-z`: operands joined by operators that bind alike, applied from
    /// the left. `x:y`, `x,y` (a union) and `x y` (an intersection, its
    /// operator the whitespace token) too. A flat list, so that a long sum
    /// does not make a deep tree.
    Operation {
        first: Box<Node>,
        rest: Vec<(usize, Node)>,
    },
    Paren {
        open: usize,
        inner: Box<Node>,
        close: usize,
    },
    Array {
        open: usize,
        close: usize,
    },
    /// `name(args)`; an empty argument is `None`.
    Call {
        name: usize,
        open: usize,
        args: Vec<Option<Node>>,
        close: usize,
    },
    /// `LAMBDA(x,x+1)(2)`: a call's result called.
    Invoke {
        callee: Box<Node>,
        args: Vec<Option<Node>>,
        close: usize,
    },
}

impl Node {
    pub(crate) fn first(&self) -> usize {
        match self {
            Node::Leaf(index) => *index,
            Node::Prefix { op, .. } => *op,
            Node::Postfix { operand, .. } => operand.first(),
            Node::Operation { first, .. } => first.first(),
            Node::Paren { open, .. } | Node::Array { open, .. } => *open,
            Node::Call { name, .. } => *name,
            Node::Invoke { callee, .. } => callee.first(),
        }
    }

    pub(crate) fn last(&self) -> usize {
        match self {
            Node::Leaf(index) => *index,
            Node::Prefix { operand, .. } => operand.last(),
            Node::Postfix { op, .. } => *op,
            Node::Operation { first, rest } => rest
                .last()
                .map_or_else(|| first.last(), |(_, node)| node.last()),
            Node::Paren { close, .. }
            | Node::Array { close, .. }
            | Node::Call { close, .. }
            | Node::Invoke { close, .. } => *close,
        }
    }
}

/// A formula read into its structure.
pub(crate) struct Parsed<'a> {
    pub text: &'a str,
    pub tokens: Vec<Token>,
    pub root: Node,
}

impl Parsed<'_> {
    pub(crate) fn token(&self, index: usize) -> &str {
        &self.text[self.tokens[index].span.clone()]
    }

    pub(crate) fn kind(&self, index: usize) -> TokenKind {
        self.tokens[index].kind
    }

    /// The index of the first token after `index` that is not whitespace.
    pub(crate) fn next_significant(&self, index: usize) -> Option<usize> {
        (index + 1..self.tokens.len()).find(|&next| self.kind(next) != TokenKind::Whitespace)
    }
}

/// Reads `text` into its structure; with `check`, also refuses what Excel
/// would repair.
pub(crate) fn parse(text: &str, check: bool) -> Result<Parsed<'_>, SyntaxError> {
    if check && text.encode_utf16().count() > MAX_LENGTH {
        return Err(SyntaxError::new(
            text,
            0,
            "a formula holds at most 8,192 characters",
        ));
    }
    let tokens = tokenize(text)?;
    let significant: Vec<usize> = (0..tokens.len())
        .filter(|&index| tokens[index].kind != TokenKind::Whitespace)
        .collect();
    let mut parser = Parser {
        text,
        tokens: &tokens,
        significant,
        at: 0,
        check,
        depth: 0,
        calls: 0,
    };
    let root = parser.formula()?;
    Ok(Parsed { text, tokens, root })
}

struct Parser<'a> {
    text: &'a str,
    tokens: &'a [Token],
    /// The indexes of the tokens that are not whitespace.
    significant: Vec<usize>,
    /// The position in `significant`.
    at: usize,
    check: bool,
    depth: usize,
    calls: usize,
}

/// How tightly an infix operator binds (Excel's precedence, loosest first).
fn binding(op: &str) -> Option<u8> {
    Some(match op {
        "=" | "<>" | "<" | "<=" | ">" | ">=" => 1,
        "&" => 2,
        "+" | "-" => 3,
        "*" | "/" => 4,
        "^" => 5,
        ":" => RANGE,
        _ => return None,
    })
}
const PERCENT: u8 = 6;
const NEGATION: u8 = 7;
const UNION: u8 = 8;
const INTERSECTION: u8 = 9;
const RANGE: u8 = 10;

impl Parser<'_> {
    fn peek(&self) -> Option<usize> {
        self.significant.get(self.at).copied()
    }

    fn kind(&self, index: usize) -> TokenKind {
        self.tokens[index].kind
    }

    fn token(&self, index: usize) -> &str {
        &self.text[self.tokens[index].span.clone()]
    }

    fn advance(&mut self) -> usize {
        let index = self.significant[self.at];
        self.at += 1;
        index
    }

    /// Whether whitespace comes just before the next token.
    fn spaced(&self) -> bool {
        self.peek()
            .is_some_and(|index| index > 0 && self.kind(index - 1) == TokenKind::Whitespace)
    }

    fn error_at(&self, index: usize, message: impl Into<String>) -> SyntaxError {
        SyntaxError::new(self.text, self.tokens[index].span.start, message)
    }

    fn error_at_end(&self, message: impl Into<String>) -> SyntaxError {
        SyntaxError::new(self.text, self.text.len(), message)
    }

    fn semicolon(&self, index: usize) -> SyntaxError {
        self.error_at(
            index,
            "`;` separates the rows of an array constant only: write `,` between arguments",
        )
    }

    /// The error for a value missing where the next token stands.
    fn missing_value(&self) -> SyntaxError {
        let previous = self
            .at
            .checked_sub(1)
            .map(|at| self.token(self.significant[at]));
        match (previous, self.peek()) {
            (_, Some(index)) if self.kind(index) == TokenKind::Semicolon => self.semicolon(index),
            (Some(previous), None) => {
                self.error_at_end(format!("a value is missing after `{previous}`"))
            }
            (None, None) => self.error_at_end("the formula is empty"),
            (None, Some(index)) => self.error_at(
                index,
                format!("a value is missing before `{}`", self.token(index)),
            ),
            (Some(previous), Some(index)) => self.error_at(
                index,
                format!(
                    "a value is missing between `{previous}` and `{}`",
                    self.token(index)
                ),
            ),
        }
    }

    fn formula(&mut self) -> Result<Node, SyntaxError> {
        let root = self.expression(0, true)?;
        match self.peek() {
            None => Ok(root),
            Some(index) => Err(match self.kind(index) {
                TokenKind::CloseParen => self.error_at(index, "`)` has no `(` to close"),
                TokenKind::CloseBrace => self.error_at(index, "`}` has no `{` to close"),
                TokenKind::Semicolon => self.semicolon(index),
                _ => self.error_at(
                    index,
                    format!("an operator is missing before `{}`", self.token(index)),
                ),
            }),
        }
    }

    /// An expression whose operators bind at least as tightly as `minimum`;
    /// `union` says whether `,` joins references here (it separates
    /// arguments in a call).
    fn expression(&mut self, minimum: u8, union: bool) -> Result<Node, SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let at = self
                .peek()
                .map_or(self.text.len(), |index| self.tokens[index].span.start);
            return Err(SyntaxError::new(
                self.text,
                at,
                "the formula nests too deeply",
            ));
        }
        let mut left = self.prefix(union)?;
        // The strength of the operators `left` chains, if this loop built it.
        let mut chained = 0;
        let mut postfixes = 0;
        while let Some(index) = self.peek() {
            let kind = self.kind(index);
            let op = self.token(index);
            if kind == TokenKind::Operator && op == "%" {
                if PERCENT < minimum {
                    break;
                }
                self.advance();
                postfixes += 1;
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(self.error_at(index, "the formula nests too deeply"));
                }
                left = Node::Postfix {
                    operand: Box::new(left),
                    op: index,
                };
                chained = 0;
                continue;
            }
            if kind == TokenKind::Operator && op == "#" {
                return Err(self.error_at(index, "`#` follows a cell or a name"));
            }
            let (strength, op_index) = if kind == TokenKind::Operator
                && let Some(strength) = binding(op)
            {
                (strength, index)
            } else if kind == TokenKind::Comma && union {
                (UNION, index)
            } else if self.spaced() && self.starts_reference(index) && self.is_reference(&left) {
                (INTERSECTION, index - 1)
            } else {
                break;
            };
            if strength < minimum {
                break;
            }
            if op_index == index {
                self.advance();
            }
            let right = self.expression(strength + 1, union)?;
            if strength >= UNION && !(self.is_reference(&left) && self.is_reference(&right)) {
                let what = match strength {
                    UNION => "a `,` outside a function joins references only",
                    INTERSECTION => "a space between values is an intersection of references",
                    _ => "`:` makes a range of two references",
                };
                return Err(self.error_at(op_index, what));
            }
            left = match left {
                Node::Operation { first, mut rest } if strength == chained => {
                    rest.push((op_index, right));
                    Node::Operation { first, rest }
                }
                left => Node::Operation {
                    first: Box::new(left),
                    rest: vec![(op_index, right)],
                },
            };
            chained = strength;
        }
        self.depth -= 1 + postfixes;
        Ok(left)
    }

    /// Whether the token at `index` can start a reference: what makes a
    /// space before it an intersection.
    fn starts_reference(&self, index: usize) -> bool {
        match self.kind(index) {
            TokenKind::Reference
            | TokenKind::Name
            | TokenKind::Function
            | TokenKind::StructuredReference
            | TokenKind::OpenParen => true,
            TokenKind::Error => self.token(index).eq_ignore_ascii_case("#REF!"),
            TokenKind::Operator => self.token(index) == "@",
            _ => false,
        }
    }

    /// Whether `node` can give a reference, as the operands of `:`, of an
    /// intersection and of a union must.
    fn is_reference(&self, node: &Node) -> bool {
        match node {
            Node::Leaf(index) => match self.kind(*index) {
                TokenKind::Reference | TokenKind::Name | TokenKind::StructuredReference => true,
                TokenKind::Error => self.token(*index).eq_ignore_ascii_case("#REF!"),
                _ => false,
            },
            Node::Call { .. } | Node::Invoke { .. } => true,
            Node::Paren { inner, .. } => self.is_reference(inner),
            Node::Operation { rest, .. } => rest.first().is_some_and(|(op, _)| {
                matches!(self.kind(*op), TokenKind::Whitespace | TokenKind::Comma)
                    || self.token(*op) == ":"
            }),
            Node::Postfix { op, .. } => self.token(*op) == "#",
            Node::Prefix { op, .. } => self.token(*op) == "@",
            Node::Array { .. } => false,
        }
    }

    fn prefix(&mut self, union: bool) -> Result<Node, SyntaxError> {
        let Some(index) = self.peek() else {
            return Err(self.missing_value());
        };
        if self.kind(index) == TokenKind::Operator {
            let op = self.token(index);
            let strength = match op {
                "+" | "-" => NEGATION,
                // `@` takes a range: `@A1:A10` is `@(A1:A10)`.
                "@" => RANGE,
                _ => return Err(self.missing_value()),
            };
            self.advance();
            let operand = self.expression(strength, union)?;
            return Ok(Node::Prefix {
                op: index,
                operand: Box::new(operand),
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, SyntaxError> {
        let Some(index) = self.peek() else {
            return Err(self.missing_value());
        };
        match self.kind(index) {
            TokenKind::Number | TokenKind::Text | TokenKind::Boolean => {
                self.advance();
                Ok(Node::Leaf(index))
            }
            TokenKind::Error => {
                self.advance();
                self.check_error_value(index)?;
                Ok(Node::Leaf(index))
            }
            TokenKind::StructuredReference => {
                self.advance();
                Ok(Node::Leaf(index))
            }
            TokenKind::Reference | TokenKind::Name => {
                self.advance();
                let leaf = Node::Leaf(index);
                match self.peek() {
                    Some(next)
                        if self.kind(next) == TokenKind::Operator && self.token(next) == "#" =>
                    {
                        if !self.spills(index) {
                            return Err(self.error_at(next, "`#` follows a cell or a name"));
                        }
                        self.advance();
                        Ok(Node::Postfix {
                            operand: Box::new(leaf),
                            op: next,
                        })
                    }
                    _ => Ok(leaf),
                }
            }
            TokenKind::Function => self.call(),
            TokenKind::OpenParen => {
                self.advance();
                let inner = self.expression(0, true)?;
                match self.peek() {
                    Some(close) if self.kind(close) == TokenKind::CloseParen => {
                        self.advance();
                        Ok(Node::Paren {
                            open: index,
                            inner: Box::new(inner),
                            close,
                        })
                    }
                    Some(other) if self.kind(other) == TokenKind::Semicolon => {
                        Err(self.semicolon(other))
                    }
                    Some(other) => Err(self.error_at(
                        other,
                        format!("an operator is missing before `{}`", self.token(other)),
                    )),
                    None => Err(self.error_at(index, "`(` is not closed")),
                }
            }
            TokenKind::OpenBrace => self.array(),
            _ => Err(self.missing_value()),
        }
    }

    /// Whether a spill `#` may follow the reference or name at `index`: a
    /// cell or a name, not a range.
    fn spills(&self, index: usize) -> bool {
        self.kind(index) == TokenKind::Name
            || matches!(target(self.token(index)), Some(Target::Cell { .. }))
    }

    fn check_error_value(&self, index: usize) -> Result<(), SyntaxError> {
        let value = self.token(index);
        if self.check
            && !ERROR_VALUES
                .iter()
                .any(|known| known.eq_ignore_ascii_case(value))
        {
            return Err(self.error_at(index, format!("`{value}` is not an error value")));
        }
        Ok(())
    }

    /// An array constant: rows of constants, `,` between columns and `;`
    /// between rows.
    fn array(&mut self) -> Result<Node, SyntaxError> {
        let open = self.advance();
        let mut rows = vec![0usize];
        loop {
            // An element: a number (signed or not), a text, a boolean, an error.
            let Some(mut index) = self.peek() else {
                return Err(self.error_at(open, "`{` is not closed"));
            };
            if self.kind(index) == TokenKind::Operator && self.token(index) == "-" {
                self.advance();
                index = self
                    .peek()
                    .ok_or_else(|| self.error_at(open, "`{` is not closed"))?;
                if self.kind(index) != TokenKind::Number {
                    return Err(self.not_a_constant(index));
                }
            }
            match self.kind(index) {
                TokenKind::Number | TokenKind::Text | TokenKind::Boolean => {}
                TokenKind::Error => self.check_error_value(index)?,
                _ => return Err(self.not_a_constant(index)),
            }
            self.advance();
            *rows.last_mut().expect("a row") += 1;
            let Some(separator) = self.peek() else {
                return Err(self.error_at(open, "`{` is not closed"));
            };
            match self.kind(separator) {
                TokenKind::Comma => {}
                TokenKind::Semicolon => rows.push(0),
                TokenKind::CloseBrace => {
                    self.advance();
                    if self.check && rows.iter().any(|&length| length != rows[0]) {
                        return Err(self.error_at(
                            open,
                            "the rows of an array constant must have the same length",
                        ));
                    }
                    return Ok(Node::Array {
                        open,
                        close: separator,
                    });
                }
                _ => return Err(self.not_a_constant(separator)),
            }
            self.advance();
        }
    }

    fn not_a_constant(&self, index: usize) -> SyntaxError {
        self.error_at(
            index,
            format!(
                "an array constant holds numbers, texts, TRUE, FALSE and error values only, not `{}`",
                self.token(index)
            ),
        )
    }

    /// A function call, then the calls of its result (`LAMBDA(x,x)(1)`).
    fn call(&mut self) -> Result<Node, SyntaxError> {
        let name = self.advance();
        let open = self.advance();
        debug_assert_eq!(
            self.kind(open),
            TokenKind::OpenParen,
            "the lexer puts `(` after a function"
        );
        let (args, close) = self.arguments(name, open)?;
        let mut node = Node::Call {
            name,
            open,
            args,
            close,
        };
        if self.check {
            self.check_call(&node)?;
        }
        while let Some(open) = self.peek() {
            if self.kind(open) != TokenKind::OpenParen || self.spaced() {
                break;
            }
            self.advance();
            let (args, close) = self.arguments(name, open)?;
            if self.check && args.len() > MAX_ARGUMENTS {
                let count = args.len();
                return Err(self.error_at(
                    open,
                    format!("a function takes at most 255 arguments, not {count}"),
                ));
            }
            node = Node::Invoke {
                callee: Box::new(node),
                args,
                close,
            };
        }
        Ok(node)
    }

    /// The arguments of a call after its `(`, up to its `)`.
    fn arguments(
        &mut self,
        name: usize,
        open: usize,
    ) -> Result<(Vec<Option<Node>>, usize), SyntaxError> {
        self.calls += 1;
        // Read or checked: deeper nesting is no Excel formula.
        if self.calls > MAX_NESTED_CALLS {
            return Err(self.error_at(name, "functions nest at most 64 levels deep"));
        }
        let mut args = Vec::new();
        if let Some(close) = self.peek()
            && self.kind(close) == TokenKind::CloseParen
        {
            self.advance();
            self.calls -= 1;
            return Ok((args, close));
        }
        loop {
            match self.peek().map(|index| self.kind(index)) {
                Some(TokenKind::Comma | TokenKind::CloseParen) => args.push(None),
                Some(TokenKind::Semicolon) => {
                    return Err(self.semicolon(self.peek().expect("a token")));
                }
                None => return Err(self.error_at(open, "`(` is not closed")),
                _ => args.push(Some(self.expression(0, false)?)),
            }
            let Some(index) = self.peek() else {
                return Err(self.error_at(open, "`(` is not closed"));
            };
            match self.kind(index) {
                TokenKind::Comma => {
                    self.advance();
                }
                TokenKind::CloseParen => {
                    self.advance();
                    self.calls -= 1;
                    return Ok((args, index));
                }
                TokenKind::Semicolon => return Err(self.semicolon(index)),
                _ => {
                    return Err(self.error_at(
                        index,
                        format!("an operator is missing before `{}`", self.token(index)),
                    ));
                }
            }
        }
    }

    /// The argument count of a built-in function, the limit of every call,
    /// and the names LET and LAMBDA declare.
    fn check_call(&self, node: &Node) -> Result<(), SyntaxError> {
        let Node::Call { name, args, .. } = node else {
            return Ok(());
        };
        let count = args.len();
        if count > MAX_ARGUMENTS {
            return Err(self.error_at(
                *name,
                format!("a function takes at most 255 arguments, not {count}"),
            ));
        }
        let Some(function) = function(self.token(*name)) else {
            return Ok(());
        };
        let (min, max) = (
            usize::from(function.min_arguments),
            usize::from(function.max_arguments),
        );
        let plural = |n: usize| match n {
            0 => "no arguments".to_owned(),
            1 => "1 argument".to_owned(),
            n => format!("{n} arguments"),
        };
        let wrong = if min == max && count != min {
            Some(format!("takes {}", plural(min)))
        } else if count < min {
            Some(format!("takes at least {}", plural(min)))
        } else if count > max {
            Some(format!("takes at most {}", plural(max)))
        } else {
            None
        };
        if let Some(wrong) = wrong {
            return Err(self.error_at(*name, format!("`{}` {wrong}, not {count}", function.name)));
        }
        let declares = |index: usize| match &args[index] {
            Some(Node::Leaf(leaf)) => {
                self.kind(*leaf) == TokenKind::Name && !self.token(*leaf).contains('!')
            }
            _ => false,
        };
        let not_a_name = |index: usize| match &args[index] {
            Some(node) => format!("`{}` is not a name", self.span_text(node)),
            None => "a name is missing".to_owned(),
        };
        match function.name {
            "LET" if count % 2 == 0 => Err(self.error_at(
                *name,
                "`LET` takes a name and its value for each name, then a calculation",
            )),
            "LET" => match (0..count - 1).step_by(2).find(|&index| !declares(index)) {
                Some(index) => Err(self.error_at(
                    *name,
                    format!("`LET` declares names: {}", not_a_name(index)),
                )),
                None => Ok(()),
            },
            "LAMBDA" => match (0..count - 1).find(|&index| !declares(index)) {
                Some(index) => Err(self.error_at(
                    *name,
                    format!(
                        "`LAMBDA` takes its parameter names, then a calculation: {}",
                        not_a_name(index)
                    ),
                )),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn span_text(&self, node: &Node) -> &str {
        &self.text[self.tokens[node.first()].span.start..self.tokens[node.last()].span.end]
    }
}
