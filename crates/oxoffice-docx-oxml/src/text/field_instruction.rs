//! Field instruction syntax: the lexer and the classification of its tokens.

use super::*;

/// Parse a field instruction string into the shared recursive grammar.
pub(super) fn parse_field_instruction(instr: &str) -> FieldInstruction {
    classify_instruction_tokens(instr.to_owned(), lex_field_text(instr))
        .materialize(Vec::new())
        .0
}

/// Whether the shared field lexer and parser admit a nonempty first token.
#[doc(hidden)]
pub fn story_field_instruction_has_name(instruction: &str) -> bool {
    !parse_field_instruction(instruction).name.is_empty()
}

#[derive(Debug)]
pub(super) enum InstructionPart {
    Text(String),
    Nested(Field, ParsedFieldSummary),
}

/// The grammar carries occurrence identity until actual model publication.
#[derive(Clone, Debug)]
pub(super) enum InstructionOperand {
    Text(String),
    Nested(usize),
}

impl InstructionOperand {
    fn text_bytes(&self) -> Option<usize> {
        match self {
            Self::Text(text) => Some(text.len()),
            Self::Nested(_) => None,
        }
    }

    pub(super) fn materialize(self, fields: &mut [Option<Field>]) -> FieldArgument {
        match self {
            Self::Text(text) => FieldArgument::Text(text),
            Self::Nested(index) => FieldArgument::Nested(Box::new(
                fields[index]
                    .take()
                    .expect("one field per instruction occurrence"),
            )),
        }
    }
}

impl InstructionSyntax {
    pub(super) fn initial_work_bytes(
        &self,
        cached_result: &str,
        scope_bytes: usize,
    ) -> Option<usize> {
        self.instruction_work?
            .checked_add(cached_result.len())?
            .checked_add(scope_bytes)
    }

    pub(super) fn materialize(
        self,
        mut fields: Vec<Option<Field>>,
    ) -> (FieldInstruction, Vec<NestedFieldPosition>) {
        let arguments = self
            .arguments
            .into_iter()
            .map(|argument| argument.materialize(&mut fields))
            .collect();
        let switches = self
            .switches
            .into_iter()
            .map(|(name, argument)| FieldSwitch {
                name,
                argument: argument.map(|argument| argument.materialize(&mut fields)),
            })
            .collect();
        (
            FieldInstruction {
                raw: self.raw,
                name: self.name,
                arguments,
                switches,
            },
            self.nested_order,
        )
    }
}

pub(super) fn parse_instruction_syntax(
    parts: Vec<InstructionPart>,
) -> (
    InstructionSyntax,
    Vec<Option<Field>>,
    Vec<ParsedFieldSummary>,
) {
    let mut raw = String::new();
    let mut lexer = FieldLexer::new();
    let mut text = String::new();
    let mut fields = Vec::new();
    let mut summaries = Vec::new();
    for part in parts {
        match part {
            InstructionPart::Text(value) => {
                raw.push_str(&value);
                text.push_str(&value);
            }
            InstructionPart::Nested(field, summary) => {
                lexer.text(&text);
                text.clear();
                lexer.nested(fields.len());
                fields.push(Some(field));
                summaries.push(summary);
            }
        }
    }
    lexer.text(&text);

    (
        classify_instruction_tokens(raw, lexer.finish()),
        fields,
        summaries,
    )
}

pub(super) fn classify_instruction_tokens(
    raw: String,
    tokens: Vec<InstructionToken>,
) -> InstructionSyntax {
    let mut tokens = tokens.into_iter();
    let name = match tokens.next().map(|token| token.argument) {
        Some(InstructionOperand::Text(value)) => value.to_uppercase(),
        Some(InstructionOperand::Nested(_)) | None => String::new(),
    };
    let remaining = tokens.collect::<Vec<_>>();
    let mut arguments = Vec::new();
    let mut switches = Vec::new();
    let mut nested_order = Vec::new();
    let mut index = 0usize;
    while index < remaining.len() {
        let InstructionToken {
            argument: InstructionOperand::Text(value),
            quoted,
            ..
        } = &remaining[index]
        else {
            if matches!(&remaining[index].argument, InstructionOperand::Nested(_)) {
                nested_order.push(NestedFieldPosition::Argument(arguments.len()));
            }
            arguments.push(remaining[index].argument.clone());
            index += 1;
            continue;
        };
        let Some(switch_name) = (!quoted)
            .then(|| value.strip_prefix('\\'))
            .flatten()
            .filter(|name| !name.is_empty())
        else {
            arguments.push(remaining[index].argument.clone());
            index += 1;
            continue;
        };
        let takes_argument = switch_takes_argument(&name, switch_name)
            || !switch_is_known_flag(&name, switch_name)
                && remaining.get(index + 1).is_some_and(|token| {
                    token.quoted || matches!(token.argument, InstructionOperand::Nested(_))
                });
        let argument = if takes_argument && remaining.get(index + 1).is_some_and(|next| {
            next.quoted
                || !matches!(&next.argument, InstructionOperand::Text(text) if text.starts_with('\\'))
        }) {
            index += 1;
            Some(remaining[index].argument.clone())
        } else {
            None
        };
        if matches!(&argument, Some(InstructionOperand::Nested(_))) {
            nested_order.push(NestedFieldPosition::Switch(switches.len()));
        }
        switches.push((switch_name.to_ascii_lowercase(), argument));
        index += 1;
    }

    let raw = raw.trim().to_owned();
    let mut instruction_work = initial_field_work_base(raw.len(), name.len(), 0, 0);
    for argument in &arguments {
        instruction_work = field_argument_work(instruction_work, argument.text_bytes());
    }
    for (name, argument) in &switches {
        instruction_work = field_switch_work(
            instruction_work,
            name.len(),
            argument.as_ref().and_then(InstructionOperand::text_bytes),
        );
    }
    InstructionSyntax {
        raw,
        name,
        arguments,
        switches,
        nested_order,
        instruction_work,
    }
}

pub(super) fn switch_is_known_flag(field_name: &str, switch_name: &str) -> bool {
    let flags: &[&str] = match field_name {
        "REF" => &["h", "n", "t", "w", "p"],
        "PAGEREF" => &["h", "p"],
        "SEQ" => &["n", "c", "h"],
        "STYLEREF" => &["l", "n", "t", "w", "p"],
        "TOC" => &["h", "z", "u", "w", "x"],
        "TC" => &["n"],
        "TOA" => &["b", "h", "p"],
        "INDEX" => &["r"],
        "XE" | "TA" => &["b", "i"],
        "CITATION" => &["n", "y", "t"],
        "INCLUDETEXT" => &["!"],
        "FILENAME" => &["p"],
        "MERGEFIELD" => &["m", "v"],
        _ => &[],
    };
    flags.contains(&switch_name.to_ascii_lowercase().as_str())
}

pub(super) fn switch_takes_argument(field_name: &str, switch_name: &str) -> bool {
    let switch_name = switch_name.to_ascii_lowercase();
    if field_name == "INDEX" && switch_name == "r" {
        return false;
    }
    matches!(
        switch_name.as_str(),
        "*" | "#" | "@" | "r" | "s" | "d" | "b" | "f"
    ) || field_name == "INCLUDETEXT" && switch_name == "c"
        || field_name == "TOC" && matches!(switch_name.as_str(), "o" | "n" | "t" | "p")
        || field_name == "TC" && switch_name == "l"
        || field_name == "CITATION"
            && matches!(switch_name.as_str(), "l" | "m" | "p" | "v" | "f" | "s")
        || field_name == "BIBLIOGRAPHY" && matches!(switch_name.as_str(), "l" | "m" | "f")
        || field_name == "INDEX"
            && matches!(
                switch_name.as_str(),
                "z" | "f" | "h" | "e" | "l" | "g" | "k" | "s"
            )
        || field_name == "XE" && matches!(switch_name.as_str(), "f" | "r" | "t")
        || field_name == "TA" && matches!(switch_name.as_str(), "l" | "s" | "c" | "r")
        || field_name == "TOA" && matches!(switch_name.as_str(), "c" | "e" | "l" | "g")
        || field_name == "TOC" && matches!(switch_name.as_str(), "c" | "a")
        || matches!(field_name, "DISPLAYBARCODE" | "MERGEBARCODE")
            && matches!(switch_name.as_str(), "h" | "q" | "p" | "c")
}

pub(super) fn lex_field_text(input: &str) -> Vec<InstructionToken> {
    let mut lexer = FieldLexer::new();
    lexer.text(input);
    lexer.finish()
}

/// Splits a field instruction into operands, one text segment or nested field
/// at a time. A nested field is one atom of its operand, so a quote stays open
/// across it: `IF "{ MERGEFIELD Country }" = "France"` reads the nested field
/// as the quoted operand, as Word does.
struct FieldLexer {
    tokens: Vec<InstructionToken>,
    /// The operand being read: text, nested fields, or a mix of both.
    pieces: Vec<InstructionOperand>,
    quoted: bool,
    token_was_quoted: bool,
    token_start: Option<usize>,
    token_end: usize,
    quote_closed: bool,
    well_formed: bool,
}

impl FieldLexer {
    pub(super) fn new() -> Self {
        Self {
            tokens: Vec::new(),
            pieces: Vec::new(),
            quoted: false,
            token_was_quoted: false,
            token_start: None,
            token_end: 0,
            quote_closed: false,
            well_formed: true,
        }
    }

    /// Read one text segment. Source spans are offsets in this segment, so
    /// they only mean something when the instruction is one segment.
    fn text(&mut self, input: &str) {
        let mut characters = input.char_indices().peekable();
        while let Some((offset, character)) = characters.next() {
            if !self.quoted && character.is_whitespace() {
                self.token_end = offset;
                self.end_token();
                continue;
            }
            self.token_start.get_or_insert(offset);
            if self.quoted {
                match character {
                    '"' => {
                        self.quoted = false;
                        self.quote_closed = true;
                    }
                    '\\' if characters
                        .peek()
                        .is_some_and(|(_, next)| matches!(next, '"' | '\\')) =>
                    {
                        self.push_char(characters.next().expect("peeked character exists").1);
                    }
                    _ => self.push_char(character),
                }
            } else {
                if self.quote_closed {
                    self.well_formed = false;
                }
                match character {
                    '"' => {
                        if !self.pieces.is_empty() {
                            self.well_formed = false;
                        }
                        self.quoted = true;
                        self.token_was_quoted = true;
                    }
                    _ => self.push_char(character),
                }
            }
        }
        self.token_end = input.len();
    }

    /// Read one nested field, the `index`th of the instruction.
    fn nested(&mut self, index: usize) {
        if !self.quoted && self.quote_closed {
            self.well_formed = false;
        }
        self.pieces.push(InstructionOperand::Nested(index));
    }

    fn push_char(&mut self, character: char) {
        if let Some(InstructionOperand::Text(text)) = self.pieces.last_mut() {
            text.push(character);
        } else {
            self.pieces
                .push(InstructionOperand::Text(character.to_string()));
        }
    }

    fn end_token(&mut self) {
        let pieces = std::mem::take(&mut self.pieces);
        let quoted = self.token_was_quoted;
        let well_formed = self.well_formed && !self.quoted;
        let span = self.token_start.map(|start| start..self.token_end);
        self.token_start = None;
        self.token_was_quoted = false;
        self.quote_closed = false;
        self.well_formed = true;
        let has_nested = pieces
            .iter()
            .any(|piece| matches!(piece, InstructionOperand::Nested(_)));
        match <[InstructionOperand; 1]>::try_from(pieces) {
            Ok([argument]) => self.tokens.push(InstructionToken {
                source_span: (!has_nested).then_some(span).flatten(),
                argument,
                quoted,
                well_formed,
            }),
            Err(pieces) if pieces.is_empty() => {
                if quoted {
                    self.tokens.push(InstructionToken {
                        argument: InstructionOperand::Text(String::new()),
                        quoted,
                        source_span: span,
                        well_formed,
                    });
                }
            }
            // Text joined to a nested field, or two nested fields in one
            // operand: the model holds no such operand, so each piece is one
            // and the token is marked ill-formed.
            Err(pieces) => {
                self.tokens
                    .extend(pieces.into_iter().map(|argument| InstructionToken {
                        argument,
                        quoted,
                        source_span: None,
                        well_formed: false,
                    }))
            }
        }
    }

    fn finish(mut self) -> Vec<InstructionToken> {
        self.end_token();
        self.tokens
    }
}

/// Replace only bibliography l, f and m token spans, retaining other raw spelling.
///
/// This checked edit uses the shared field lexer. Ambiguous source boundaries
/// return an error before a caller can publish any instruction changes.
#[doc(hidden)]
pub fn bibliography_option_switch_edits(
    raw: &str,
    replacement: &[FieldSwitch],
) -> Result<Vec<(std::ops::Range<usize>, String)>> {
    let tokens = lex_field_text(raw);
    if tokens.iter().any(|token| !token.well_formed) {
        return Err(OxmlError::InvalidValue(
            "ambiguous bibliography token boundary".into(),
        ));
    }
    let instruction = parse_field_instruction(raw);
    if instruction.name != "BIBLIOGRAPHY" || !instruction.arguments.is_empty() {
        return Err(OxmlError::InvalidValue(
            "invalid bibliography instruction shape".into(),
        ));
    }
    if replacement.iter().any(|switch| {
        !matches!(switch.name.as_str(), "l" | "f" | "m")
            || !matches!(switch.argument, Some(FieldArgument::Text(_)))
    }) {
        return Err(OxmlError::InvalidValue(
            "invalid owned bibliography replacement switch".into(),
        ));
    }
    let authored = FieldInstruction::new("BIBLIOGRAPHY", Vec::new(), replacement.to_vec())?;
    let mut ranges = Vec::new();
    let mut index = 1;
    while index < tokens.len() {
        let InstructionOperand::Text(value) = &tokens[index].argument else {
            return Err(OxmlError::InvalidValue("nested bibliography token".into()));
        };
        let Some(name) = (!tokens[index].quoted)
            .then(|| value.strip_prefix('\\'))
            .flatten()
        else {
            return Err(OxmlError::InvalidValue(
                "unowned bibliography argument".into(),
            ));
        };
        let name = name.to_ascii_lowercase();
        let takes_argument = switch_takes_argument("BIBLIOGRAPHY", &name)
            || !switch_is_known_flag("BIBLIOGRAPHY", &name)
                && tokens.get(index + 1).is_some_and(|token| token.quoted);
        let has_argument = takes_argument && tokens.get(index + 1).is_some_and(|token| {
            token.quoted
                || !matches!(&token.argument, InstructionOperand::Text(value) if value.starts_with('\\'))
        });
        let start = tokens[index]
            .source_span
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("missing bibliography token span".into()))?
            .start;
        let end_index = index + usize::from(has_argument);
        if matches!(name.as_str(), "l" | "m") && !has_argument {
            return Err(OxmlError::InvalidValue(
                "missing bibliography option operand".into(),
            ));
        }
        if matches!(name.as_str(), "l" | "f" | "m") {
            let end = tokens[end_index]
                .source_span
                .as_ref()
                .ok_or_else(|| OxmlError::InvalidValue("missing bibliography operand span".into()))?
                .end;
            ranges.push(start..end);
        }
        index = end_index + 1;
    }
    let owned = instruction
        .switches
        .iter()
        .filter(|switch| matches!(switch.name.as_str(), "l" | "f" | "m"))
        .cloned()
        .collect::<Vec<_>>();
    if owned == replacement {
        return Ok(Vec::new());
    }
    let mut edits = ranges
        .into_iter()
        .map(|range| (range, String::new()))
        .collect::<Vec<_>>();
    let appended = &authored.raw["BIBLIOGRAPHY".len()..];
    if !appended.is_empty() {
        edits.push((raw.len()..raw.len(), appended.to_owned()));
    }
    Ok(edits)
}
