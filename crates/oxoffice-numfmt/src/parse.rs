//! A format code read into sections of tokens.

use crate::names::Language;
use crate::{Color, Error};

/// A digit placeholder: `0` shows a zero, `?` a space, `#` nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placeholder {
    Zero,
    Question,
    Hash,
}

impl Placeholder {
    /// What the placeholder shows where the value has no digit.
    pub(crate) fn pad(self) -> Option<char> {
        match self {
            Placeholder::Zero => Some('0'),
            Placeholder::Question => Some(' '),
            Placeholder::Hash => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Hour,
    Minute,
    Second,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token {
    Literal(String),
    /// `_x`: a space as wide as `x`.
    Space(char),
    /// `*x`: `x` repeated to fill the cell.
    Fill(char),
    Digit(Placeholder),
    Point,
    Comma,
    Percent,
    /// `E+` (`plus`) or `E-`.
    Exponent {
        plus: bool,
    },
    Slash,
    /// `@`.
    Text,
    General,
    Year(usize),
    /// `e`, `ee`: the year of the era under a Japanese tag, the Gregorian year under any other.
    EraYear(usize),
    /// `g`, `gg`, `ggg`: the era under a Japanese tag (its initial letter, its first character, its name),
    /// nothing under any other.
    Era(usize),
    Month(usize),
    Day(usize),
    Hour(usize),
    Minute(usize),
    Second(usize),
    /// `m` or `mm` before the rule says whether it is the month or the minute.
    MonthOrMinute(usize),
    /// `[h]`, `[mm]`, `[ss]`...: the whole time in one unit.
    Elapsed(Unit, usize),
    /// `.0`, `.00`, `.000` after the seconds.
    SubSecond(usize),
    /// `AM/PM` shows `AM` or `PM`; `A/P` shows the letter written.
    AmPm {
        short: Option<(char, char)>,
    },
}

/// A comparison of `[>=100]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Condition {
    operator: Operator,
    threshold: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Equal,
    NotEqual,
}

impl Condition {
    /// `[>0]`, the condition of a first section without one when the second
    /// has one.
    pub(crate) const POSITIVE: Condition = Condition {
        operator: Operator::Greater,
        threshold: 0.0,
    };

    /// Whether only negative numbers meet the condition: such a section
    /// shows them without their minus sign, as the second section does.
    pub(crate) fn only_negatives(&self) -> bool {
        match self.operator {
            Operator::Less => self.threshold <= 0.0,
            Operator::LessOrEqual | Operator::Equal => self.threshold < 0.0,
            _ => false,
        }
    }

    pub(crate) fn holds(&self, value: f64) -> bool {
        let threshold = self.threshold;
        match self.operator {
            Operator::Less => value < threshold,
            Operator::LessOrEqual => value <= threshold,
            Operator::Greater => value > threshold,
            Operator::GreaterOrEqual => value >= threshold,
            Operator::Equal => value == threshold,
            Operator::NotEqual => value != threshold,
        }
    }
}

/// The date or time a `[$-F800]` or `[$-F400]` tag replaces the section with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SystemFormat {
    LongDate,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Only literals: the value shows as no digit.
    Literal,
    General,
    Number,
    Exponent,
    Fraction,
    Date,
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Section {
    pub(crate) tokens: Vec<Token>,
    pub(crate) kind: Kind,
    pub(crate) color: Option<Color>,
    pub(crate) condition: Option<Condition>,
    pub(crate) system: Option<SystemFormat>,
    /// The language of the section's locale tag (`[$-40C]`), if it has one.
    pub(crate) language: Option<Language>,
}

impl Section {
    pub(crate) fn has(&self, wanted: impl Fn(&Token) -> bool) -> bool {
        self.tokens.iter().any(wanted)
    }
}

const MAX_CODE: usize = 255;

/// Reads a code as stored in a file (ECMA-376 Part 1, 18.8.31: the
/// invariant en-US form) into its sections.
pub(crate) fn sections(code: &str) -> Result<Vec<Section>, Error> {
    let invalid = |reason: &'static str| Error::Invalid {
        code: code.to_owned(),
        reason,
    };
    if code.is_empty() {
        return Err(invalid("the code is empty"));
    }
    // Excel takes 255 characters at most, and opens a file with a longer
    // code by repairing it to General. The cap also bounds every width a
    // code asks for (`[hhh...]`).
    if code.chars().count() > MAX_CODE {
        return Err(invalid("a code has at most 255 characters"));
    }
    let mut sections = Vec::new();
    let mut section = RawSection::default();
    let chars: Vec<char> = code.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        at += 1;
        let token = match c {
            ';' => {
                sections.push(std::mem::take(&mut section));
                continue;
            }
            '"' => {
                let end = chars[at..]
                    .iter()
                    .position(|c| *c == '"')
                    .ok_or_else(|| invalid("a quoted text is not closed"))?;
                let text: String = chars[at..at + end].iter().collect();
                at += end + 1;
                Token::Literal(text)
            }
            '\\' | '_' | '*' => {
                let next = *chars
                    .get(at)
                    .ok_or_else(|| invalid("the code ends after \\, _ or *"))?;
                at += 1;
                match c {
                    '\\' => Token::Literal(next.to_string()),
                    '_' => Token::Space(next),
                    _ => Token::Fill(next),
                }
            }
            '[' => {
                let end = chars[at..]
                    .iter()
                    .position(|c| *c == ']')
                    .ok_or_else(|| invalid("a [ is not closed"))?;
                let content: String = chars[at..at + end].iter().collect();
                at += end + 1;
                match bracket(&content).ok_or_else(|| invalid("a condition is not a number"))? {
                    Bracket::Token(token) => token,
                    Bracket::Color(color) => {
                        section.color = Some(color);
                        continue;
                    }
                    Bracket::Condition(condition) => {
                        section.condition = Some(condition);
                        continue;
                    }
                    Bracket::System(system) => {
                        section.system = Some(system);
                        continue;
                    }
                    Bracket::Locale { symbol, language } => {
                        // The first tag of a section names its language; a
                        // date section with a second one is no code for
                        // Excel (see `RawSection::finish`).
                        section.second_language |= language.is_some() && section.language.is_some();
                        section.language = section.language.or(language);
                        match symbol {
                            Some(symbol) => Token::Literal(symbol),
                            None => continue,
                        }
                    }
                    Bracket::Ignored => continue,
                }
            }
            '0' => Token::Digit(Placeholder::Zero),
            '?' => Token::Digit(Placeholder::Question),
            '#' => Token::Digit(Placeholder::Hash),
            '.' => Token::Point,
            ',' => Token::Comma,
            '%' => Token::Percent,
            '/' => Token::Slash,
            '@' => Token::Text,
            'E' | 'e' if matches!(chars.get(at), Some('+' | '-')) => {
                if c == 'e' {
                    // Excel reads only `E+` and `E-` and shows such a code as
                    // General.
                    return Err(invalid("an exponent is written with a lowercase e"));
                }
                at += 1;
                Token::Exponent {
                    plus: chars[at - 1] == '+',
                }
            }
            'G' | 'g' if starts_with_ignore_case(&chars[at - 1..], "general") => {
                at += "general".len() - 1;
                Token::General
            }
            'A' | 'a' if starts_with_ignore_case(&chars[at - 1..], "am/pm") => {
                at += "am/pm".len() - 1;
                Token::AmPm { short: None }
            }
            'A' | 'a' if starts_with_ignore_case(&chars[at - 1..], "a/p") => {
                let short = (c, chars[at + 1]);
                at += "a/p".len() - 1;
                Token::AmPm { short: Some(short) }
            }
            // `g`, `gg` and `ggg` are the era, in either case; Excel cuts a longer run to `ggg` (`gggge` is
            // `ggge`).
            'G' | 'g' => {
                let count = chars[at - 1..]
                    .iter()
                    .take_while(|next| next.eq_ignore_ascii_case(&'g'))
                    .count();
                at += count - 1;
                Token::Era(count.min(3))
            }
            // `aaa` and `aaaa` are the weekday of `ddd` and `dddd`, East Asian
            // Excel's own spelling (`[$-804]aaaa`); Excel showed them so under
            // every tag and under none. A longer run, and `a` or `aa` (not
            // asked), are literals.
            'a' if run_of(&chars[at - 1..], 'a') >= 3 => {
                let count = run_of(&chars[at - 1..], 'a');
                at += count - 1;
                if count <= 4 {
                    Token::Day(count)
                } else {
                    Token::Literal("a".repeat(count))
                }
            }
            'y' | 'Y' | 'e' | 'E' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                let lower = c.to_ascii_lowercase();
                let mut count = 1;
                while chars
                    .get(at)
                    .is_some_and(|next| next.to_ascii_lowercase() == lower)
                {
                    count += 1;
                    at += 1;
                }
                // Excel reads no code with an upper-case `E` that is no exponent: it shows it as General. A
                // lower-case run is cut to `ee`.
                if lower == 'e' && chars[at - count..at].contains(&'E') {
                    return Err(invalid("an era year is written with a lowercase e"));
                }
                match lower {
                    'y' => Token::Year(count),
                    'e' => Token::EraYear(count.min(2)),
                    'm' if count <= 2 => Token::MonthOrMinute(count),
                    'm' => Token::Month(count),
                    'd' => Token::Day(count),
                    'h' => Token::Hour(count),
                    _ => Token::Second(count),
                }
            }
            c => Token::Literal(c.to_string()),
        };
        section.tokens.push(token);
    }
    sections.push(section);
    if sections.len() > 4 {
        return Err(invalid("a code has at most four sections"));
    }
    // The fourth section is the text section, whatever it holds.
    let mut sections = sections
        .into_iter()
        .enumerate()
        .map(|(index, raw)| raw.finish(index == 3).map_err(invalid))
        .collect::<Result<Vec<_>, _>>()?;
    // The locale tag of the first section names the language of every
    // section, and a tag of another section counts for nothing: Excel shows
    // `[$-407]mmmm;[$-40C]mmmm` with German names in both, and
    // `mmmm;[$-40C]mmmm` with English ones in both.
    let language = sections[0].language;
    for section in &mut sections {
        section.language = language;
    }
    Ok(sections)
}

/// How many times `c` repeats from the start of `chars`.
fn run_of(chars: &[char], c: char) -> usize {
    chars.iter().take_while(|next| **next == c).count()
}

fn starts_with_ignore_case(chars: &[char], word: &str) -> bool {
    chars.len() >= word.len()
        && chars
            .iter()
            .zip(word.chars())
            .all(|(c, w)| c.to_ascii_lowercase() == w)
}

enum Bracket {
    Token(Token),
    Color(Color),
    Condition(Condition),
    System(SystemFormat),
    /// `[$€-40C]`: the currency symbol, and the language of the tag.
    Locale {
        symbol: Option<String>,
        language: Option<Language>,
    },
    /// `[DBNum1]`, `[NatNum1]`, a locale with no symbol...: no effect on the
    /// en-US text.
    Ignored,
}

/// What a `[...]` says; `None` for a condition whose threshold is no number.
fn bracket(content: &str) -> Option<Bracket> {
    if let Some(currency) = content.strip_prefix('$') {
        let (symbol, tag) = currency.split_once('-').unwrap_or((currency, ""));
        let tag = tag.to_ascii_lowercase();
        // A language code of two letters (`de`, `ca`, `fa`) is no LCID, though
        // its letters are hexadecimal digits: a producer writes an LCID with
        // three digits or more.
        let lcid = if tag.len() == 2 && tag.bytes().all(|b| b.is_ascii_lowercase()) {
            None
        } else {
            u32::from_str_radix(&tag, 16).ok()
        };
        let system = match (tag.as_str(), lcid.map(|lcid| lcid & 0xFFFF)) {
            ("x-sysdate", _) | (_, Some(0xF800)) => Some(SystemFormat::LongDate),
            ("x-systime", _) | (_, Some(0xF400)) => Some(SystemFormat::Time),
            _ => None,
        };
        if let Some(system) = system {
            return Some(Bracket::System(system));
        }
        let language = match lcid {
            Some(lcid) => Language::of_lcid(lcid),
            None => Language::of_tag(&tag),
        };
        return Some(Bracket::Locale {
            symbol: (!symbol.is_empty()).then(|| symbol.to_owned()),
            language,
        });
    }
    let operators = [
        ("<=", Operator::LessOrEqual),
        (">=", Operator::GreaterOrEqual),
        ("<>", Operator::NotEqual),
        ("<", Operator::Less),
        (">", Operator::Greater),
        ("=", Operator::Equal),
    ];
    for (text, operator) in operators {
        if let Some(threshold) = content.strip_prefix(text) {
            let threshold = threshold.trim().parse::<f64>().ok()?;
            return Some(Bracket::Condition(Condition {
                operator,
                threshold,
            }));
        }
    }
    let lower = content.to_ascii_lowercase();
    if let Some(color) = Color::named(&lower) {
        return Some(Bracket::Color(color));
    }
    if let Some(number) = lower.strip_prefix("color") {
        return Some(
            number
                .trim()
                .parse::<u8>()
                .ok()
                .and_then(Color::numbered)
                .map_or(Bracket::Ignored, Bracket::Color),
        );
    }
    let mut letters = lower.chars();
    if let Some(first @ ('h' | 'm' | 's')) = letters.next()
        && letters.all(|c| c == first)
    {
        let unit = match first {
            'h' => Unit::Hour,
            'm' => Unit::Minute,
            _ => Unit::Second,
        };
        return Some(Bracket::Token(Token::Elapsed(unit, lower.len())));
    }
    Some(Bracket::Ignored)
}

#[derive(Default)]
struct RawSection {
    tokens: Vec<Token>,
    color: Option<Color>,
    condition: Option<Condition>,
    system: Option<SystemFormat>,
    language: Option<Language>,
    /// The section holds a second tag that names a language.
    second_language: bool,
}

impl RawSection {
    fn finish(self, text: bool) -> Result<Section, &'static str> {
        let RawSection {
            tokens,
            color,
            condition,
            system,
            language,
            second_language,
        } = self;
        let is_date = |token: &Token| {
            matches!(
                token,
                Token::Year(_)
                    | Token::EraYear(_)
                    | Token::Era(_)
                    | Token::Month(_)
                    | Token::Day(_)
                    | Token::Hour(_)
                    | Token::Minute(_)
                    | Token::Second(_)
                    | Token::MonthOrMinute(_)
                    | Token::Elapsed(..)
                    | Token::AmPm { .. }
            )
        };
        let (kind, tokens) = if text || tokens.contains(&Token::Text) {
            (Kind::Text, text_tokens(tokens)?)
        } else if system.is_some() || tokens.iter().any(is_date) {
            (Kind::Date, date_tokens(tokens)?)
        } else if tokens.contains(&Token::General) {
            if tokens.iter().any(|token| matches!(token, Token::Digit(_))) {
                return Err("General is mixed with digit placeholders");
            }
            (Kind::General, punctuation_as_literals(tokens))
        } else if tokens.iter().any(|token| matches!(token, Token::Digit(_))) {
            number_tokens(tokens)?
        } else {
            (Kind::Literal, punctuation_as_literals(tokens))
        };
        // Excel reads no date section with two language tags (`[$-40C][$-407]mmmm`
        // shows as General); only that shape was asked. Any other section takes
        // its first tag, which is our choice.
        if second_language && kind == Kind::Date {
            return Err("a date section has two locale tags");
        }
        Ok(Section {
            tokens,
            kind,
            color,
            condition,
            system,
            language,
        })
    }
}

/// The source character of a token that stands for itself in a section
/// with no digit.
fn as_literal(token: &Token) -> Option<char> {
    Some(match token {
        Token::Point => '.',
        Token::Comma => ',',
        Token::Slash => '/',
        Token::Percent => '%',
        Token::Digit(Placeholder::Zero) => '0',
        Token::Digit(Placeholder::Question) => '?',
        Token::Digit(Placeholder::Hash) => '#',
        _ => return None,
    })
}

fn punctuation_as_literals(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| match as_literal(&token) {
            Some(c) => Token::Literal(c.to_string()),
            None => token,
        })
        .collect()
}

fn text_tokens(tokens: Vec<Token>) -> Result<Vec<Token>, &'static str> {
    tokens
        .into_iter()
        .map(|token| match token {
            Token::Literal(_) | Token::Space(_) | Token::Fill(_) | Token::Text => Ok(token),
            token => as_literal(&token)
                .map(|c| Token::Literal(c.to_string()))
                .ok_or("a text section holds a number or date part"),
        })
        .collect()
}

fn date_tokens(tokens: Vec<Token>) -> Result<Vec<Token>, &'static str> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            Token::Point
                if matches!(
                    out.last(),
                    Some(Token::Second(_) | Token::Elapsed(Unit::Second, _))
                ) && tokens.peek() == Some(&Token::Digit(Placeholder::Zero)) =>
            {
                let mut count = 0;
                while tokens
                    .next_if_eq(&Token::Digit(Placeholder::Zero))
                    .is_some()
                {
                    count += 1;
                }
                out.push(Token::SubSecond(count.min(3)));
            }
            Token::Digit(_) | Token::Exponent { .. } | Token::General | Token::Text => {
                return Err("a date or time section holds a number part");
            }
            token => out.push(match as_literal(&token) {
                Some(c) => Token::Literal(c.to_string()),
                None => token,
            }),
        }
    }
    // `m` and `mm` are minutes right after the hours or right before the
    // seconds, literals between them aside; months otherwise.
    let is_time_or_date = |token: &Token| {
        !matches!(
            token,
            Token::Literal(_) | Token::Space(_) | Token::Fill(_) | Token::SubSecond(_)
        )
    };
    for at in 0..out.len() {
        let Token::MonthOrMinute(count) = out[at] else {
            continue;
        };
        let before = out[..at].iter().rev().find(|token| is_time_or_date(token));
        let after = out[at + 1..].iter().find(|token| is_time_or_date(token));
        let minute = matches!(before, Some(Token::Hour(_) | Token::Elapsed(Unit::Hour, _)))
            || matches!(
                after,
                Some(Token::Second(_) | Token::Elapsed(Unit::Second, _))
            );
        out[at] = if minute {
            Token::Minute(count)
        } else {
            Token::Month(count)
        };
    }
    Ok(out)
}

fn number_tokens(tokens: Vec<Token>) -> Result<(Kind, Vec<Token>), &'static str> {
    let exponent = tokens
        .iter()
        .position(|token| matches!(token, Token::Exponent { .. }));
    let slash = tokens.iter().enumerate().position(|(at, token)| {
        *token == Token::Slash && at > 0 && matches!(tokens[at - 1], Token::Digit(_))
    });
    if exponent.is_some() && slash.is_some() {
        return Err("a section holds both an exponent and a fraction");
    }
    if slash.is_some() && tokens.contains(&Token::Point) {
        // Excel shows `0.0 ?/?` as General.
        return Err("a fraction has a decimal point");
    }
    let mut seen_point = false;
    let tokens = tokens
        .into_iter()
        .map(|token| match token {
            Token::Point if seen_point || slash.is_some() => Token::Literal(".".into()),
            Token::Point => {
                seen_point = true;
                token
            }
            Token::Slash if slash.is_none() => Token::Literal("/".into()),
            token => token,
        })
        .collect();
    let kind = if exponent.is_some() {
        Kind::Exponent
    } else if slash.is_some() {
        Kind::Fraction
    } else {
        Kind::Number
    };
    Ok((kind, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_give_colours_conditions_symbols_and_elapsed_units() {
        let read = |code: &str| sections(code).expect(code).remove(0);
        assert_eq!(read("[Red]0").color, Color::numbered(3));
        assert_eq!(read("[color 45]0").color, Color::numbered(45));
        assert!(read("[>=1E3]0").condition.unwrap().holds(1000.0));
        assert_eq!(read("[$€-40C]0").tokens[0], Token::Literal("€".into()));
        assert_eq!(read("[$-x-sysdate]d").system, Some(SystemFormat::LongDate));
        assert_eq!(read("[hh]:mm").tokens[0], Token::Elapsed(Unit::Hour, 2));
        assert_eq!(
            read("[DBNum1]0").tokens,
            vec![Token::Digit(Placeholder::Zero)]
        );
    }

    #[test]
    fn m_is_the_minute_next_to_hours_or_seconds_and_the_month_elsewhere() {
        let read = |code: &str| sections(code).expect(code).remove(0).tokens;
        assert_eq!(read("h \"h\" m").last(), Some(&Token::Minute(1)));
        assert_eq!(read("mm ss")[0], Token::Minute(2));
        assert_eq!(read("yyyy mm")[2], Token::Month(2));
        assert_eq!(read("mmm")[0], Token::Month(3));
    }

    #[test]
    fn a_code_excel_cannot_read_is_refused() {
        let too_long = format!("0\"{}\"", "x".repeat(253));
        for code in [
            "",
            "0.0e+00",
            "0.0 ?/?",
            "\"open",
            "[Red",
            "0\\",
            "0;0;0;@;0",
            "yyyy 0",
            &too_long,
        ] {
            assert!(sections(code).is_err(), "{code}");
        }
        assert!(sections(&format!("0\"{}\"", "x".repeat(252))).is_ok());
    }

    #[test]
    fn two_language_tags_refuse_a_date_section_only() {
        // Excel showed `[$-40C][$-407]mmmm` as General, a date section.
        for code in [
            "[$-40C][$-407]mmmm",
            "[$-407]mmmm [$-40C]",
            "mmmm;[$-40C]yyyy[$-407]",
        ] {
            assert!(sections(code).is_err(), "{code}");
        }
        // Any other section takes its first tag (our choice), and a tag with
        // no language is no second tag.
        let first = Language::of_lcid(0x407);
        for code in [
            "[$€-407]#,##0.00 [$$-409]",
            "[$-407]0;[$-40C]0",
            "[$USD][$-407]0",
        ] {
            let read = sections(code).expect(code);
            assert_eq!(read[0].language, first, "{code}");
        }
        assert!(sections("[$USD][$-40C]mmmm").is_ok());
    }
}
