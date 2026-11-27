//! Spreadsheet number format codes to the text Excel displays.
//!
//! A format code is read as stored in a file (ECMA-376 Part 1, 18.8.30 and
//! 18.8.31: the invariant en-US form) into up to four sections, and a cell
//! value goes through the section Excel picks for it. The text is what an
//! en-US Excel shows in a cell wide enough for it, as its CSV export writes
//! it; [`Formatted`] also carries what a render needs on top of the text:
//! the colour of the section, the fill character and where it goes, and the
//! spaces as wide as a given character.
//!
//! ```
//! use oxoffice_numfmt::{FormatOptions, NumberFormat};
//!
//! let format = NumberFormat::parse("#,##0.00;[Red](#,##0.00)")?;
//! let shown = format.format_number(-1234.567, &FormatOptions::default());
//! assert_eq!(shown.text, "(1,234.57)");
//! assert_eq!(shown.color.map(|color| color.number()), Some(3));
//! # Ok::<(), oxoffice_numfmt::Error>(())
//! ```
//!
//! Booleans and error values are not formatted: Excel shows `TRUE`, `FALSE`
//! and the error code (`#DIV/0!`) whatever the format, so the caller writes
//! them as they are.

mod builtin;
mod date;
mod decimal;
mod number;
mod parse;

use parse::{Condition, Kind, Section, Token};

pub use builtin::builtin_code;

/// A format code Excel cannot read. Excel shows such a code as General
/// ([`NumberFormat::general`]), and so should a caller.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("number format `{code}` cannot be read: {reason}")]
    Invalid { code: String, reason: &'static str },
}

/// Which day serial 0 is. Workbooks say it with `workbookPr/@date1904`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DateSystem {
    /// Serial 1 is 1900-01-01, with Excel's 1900-02-29 (serial 60). Serial 0
    /// shows as 1900-01-00, and a negative serial cannot be shown.
    #[default]
    Date1900,
    /// Serial 0 is 1904-01-01. A negative serial shows as the date or time
    /// of its magnitude after a minus sign.
    Date1904,
}

/// The language and conventions of the displayed text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Locale {
    /// A decimal point, a comma between thousands, English month and day
    /// names, `m/d/yy` for the short date, `dddd, mmmm d, yyyy` for the long
    /// date and `h:mm:ss AM/PM` for the time.
    #[default]
    EnUs,
}

/// How a value is displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FormatOptions {
    pub date_system: DateSystem,
    pub locale: Locale,
}

impl FormatOptions {
    pub fn with_date_system(mut self, date_system: DateSystem) -> Self {
        self.date_system = date_system;
        self
    }
}

/// A colour of `[Red]` or `[Color10]`: one of the 56 colours of the
/// workbook's palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(u8);

impl Color {
    /// The colour of `[ColorN]`, N from 1 to 56.
    pub fn numbered(number: u8) -> Option<Color> {
        (1..=56).contains(&number).then_some(Color(number))
    }

    /// The colour a name stands for: `[Black]` is `[Color1]`, then White,
    /// Red, Green, Blue, Yellow, Magenta and Cyan.
    pub(crate) fn named(name: &str) -> Option<Color> {
        let names = [
            "black", "white", "red", "green", "blue", "yellow", "magenta", "cyan",
        ];
        let at = names.iter().position(|known| *known == name)?;
        Some(Color(at as u8 + 1))
    }

    /// N of `[ColorN]`.
    pub fn number(self) -> u8 {
        self.0
    }

    /// The index of the colour in the workbook's indexed colours
    /// (`styles.xml` `indexedColors`, or the default palette): N + 7.
    pub fn indexed(self) -> u8 {
        self.0 + 7
    }
}

/// `*x`: the character repeated to fill the cell's width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub character: char,
    /// The byte offset in [`Formatted::text`] where the repetitions go.
    pub at: usize,
}

/// `_x`: a space as wide as a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    /// The byte offset of the space in [`Formatted::text`].
    pub at: usize,
    /// The character whose width the space takes.
    pub width_of: char,
}

/// A value as Excel displays it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Formatted {
    /// The text, with one ordinary space for each `_x` and nothing for the
    /// fill, as Excel's CSV export writes it.
    pub text: String,
    /// The colour of the section, when it names one.
    pub color: Option<Color>,
    /// The fill, for a render: the text export leaves it out.
    pub fill: Option<Fill>,
    /// The spaces of `_x`, for a render that gives them their width.
    pub spaces: Vec<Space>,
    /// Excel cannot show the value in this format (a negative date in the
    /// 1900 system, a date after 9999, a number that matches no section,
    /// a text longer than 255 characters): it fills the cell with `#`. The
    /// text is then `#####` and the fill `#`.
    pub overflow: bool,
}

impl Formatted {
    fn overflow(color: Option<Color>) -> Formatted {
        Formatted {
            text: "#####".to_owned(),
            color,
            fill: Some(Fill {
                character: '#',
                at: 0,
            }),
            spaces: Vec::new(),
            overflow: true,
        }
    }
}

/// The text of a formatted value, built left to right.
#[derive(Default)]
pub(crate) struct Out {
    text: String,
    fill: Option<Fill>,
    spaces: Vec<Space>,
}

impl Out {
    pub(crate) fn push(&mut self, c: char) {
        self.text.push(c);
    }

    pub(crate) fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Literals, spaces and fills; `None` for any other token.
    pub(crate) fn push_plain(&mut self, token: &Token) -> Option<()> {
        match token {
            Token::Literal(text) => self.push_str(text),
            Token::Space(width_of) => {
                self.spaces.push(Space {
                    at: self.text.len(),
                    width_of: *width_of,
                });
                self.text.push(' ');
            }
            Token::Fill(character) => {
                // The first fill of a section is the one kept.
                if self.fill.is_none() {
                    self.fill = Some(Fill {
                        character: *character,
                        at: self.text.len(),
                    });
                }
            }
            _ => return None,
        }
        Some(())
    }

    pub(crate) fn finish(self, color: Option<Color>) -> Formatted {
        Formatted {
            text: self.text,
            color,
            fill: self.fill,
            spaces: self.spaces,
            overflow: false,
        }
    }
}

/// What a date or time format shows, for a reader that types a cell value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateKind {
    /// Days, months or years only.
    Date,
    /// Hours, minutes or seconds only.
    Time,
    /// Both.
    DateTime,
    /// `[h]`, `[m]` or `[s]`: a duration, not a time of day.
    Duration,
}

/// A parsed format code.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberFormat {
    code: String,
    sections: Vec<Section>,
}

impl Default for NumberFormat {
    fn default() -> Self {
        NumberFormat::general()
    }
}

impl NumberFormat {
    /// Reads a code as stored in a file. A code Excel cannot read is an
    /// error, and Excel shows it as General.
    pub fn parse(code: &str) -> Result<NumberFormat, Error> {
        Ok(NumberFormat {
            code: code.to_owned(),
            sections: parse::sections(code)?,
        })
    }

    /// `General`.
    pub fn general() -> NumberFormat {
        NumberFormat::parse("General").expect("General is a code")
    }

    /// The code as given.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The sections numbers take: all but the text section, the fourth or
    /// a last one that holds `@`.
    fn number_sections(&self) -> &[Section] {
        match self.text_section() {
            Some(_) => &self.sections[..self.sections.len() - 1],
            None => &self.sections,
        }
    }

    fn text_section(&self) -> Option<&Section> {
        self.sections.last().filter(|last| last.kind == Kind::Text)
    }

    /// Whether a positive number shows as a date or a time, and which.
    pub fn date_kind(&self) -> Option<DateKind> {
        let section = self.number_sections().first()?;
        if section.kind != Kind::Date {
            return None;
        }
        if section.has(|token| matches!(token, Token::Elapsed(..))) {
            return Some(DateKind::Duration);
        }
        let date = section.system == Some(parse::SystemFormat::LongDate)
            || section
                .has(|token| matches!(token, Token::Year(_) | Token::Month(_) | Token::Day(_)));
        let time = section.system == Some(parse::SystemFormat::Time)
            || section.has(|token| {
                matches!(
                    token,
                    Token::Hour(_) | Token::Minute(_) | Token::Second(_) | Token::AmPm { .. }
                )
            });
        Some(match (date, time) {
            (true, true) => DateKind::DateTime,
            (false, true) => DateKind::Time,
            _ => DateKind::Date,
        })
    }

    /// Whether a section tagged with a language other than English
    /// (`[$-40C]mmmm`) shows month or day names: Excel writes them in that
    /// language, this crate in English for now.
    pub fn shows_foreign_names(&self) -> bool {
        self.sections.iter().any(|section| {
            section.foreign
                && section.kind == Kind::Date
                && section.has(
                    |token| matches!(token, Token::Month(count) | Token::Day(count) if *count >= 3),
                )
        })
    }

    /// A number as Excel displays it. A value that is not finite (no cell
    /// holds one) shows as an overflow.
    pub fn format_number(&self, value: f64, options: &FormatOptions) -> Formatted {
        if !value.is_finite() {
            return Formatted::overflow(None);
        }
        let (section, sign) = match choose(self.number_sections(), value) {
            Choice::Section(section, sign) => (section, sign),
            Choice::General => return number::general_section(value, true, &[], None),
            Choice::Overflow => return Formatted::overflow(None),
        };
        let formatted = match section.kind {
            Kind::Date => date::format(section, value, sign, options),
            // `@` in a section numbers take (`@;0`): General, as a code
            // with only a text section.
            Kind::Text => number::general_section(value, sign, &[], section.color),
            _ => number::format(section, value, sign),
        };
        if !formatted.overflow && formatted.text.chars().count() > MAX_TEXT {
            return Formatted::overflow(section.color);
        }
        formatted
    }

    /// A text value: through the text section, as it is without one.
    pub fn format_text(&self, text: &str) -> Formatted {
        let Some(section) = self.text_section() else {
            return Formatted {
                text: text.to_owned(),
                ..Formatted::default()
            };
        };
        let mut out = Out::default();
        for token in &section.tokens {
            if *token == Token::Text {
                out.push_str(text);
            } else {
                out.push_plain(token);
            }
        }
        out.finish(section.color)
    }
}

/// A number whose text is longer than this is an overflow: Excel shows
/// 1E+300 in `0` as `#####`, and this limit is ours.
const MAX_TEXT: usize = 255;

enum Choice<'a> {
    /// The section, and whether it shows the minus sign.
    Section(&'a Section, bool),
    General,
    Overflow,
}

/// The section a number takes, and whether its minus sign shows.
///
/// With no condition on the first two sections: one section takes every
/// number with its sign; two give the second to negatives, without their
/// sign; three give the third to zero. With a condition on the second
/// section only, the first takes positives (`[>0]`, before the second's
/// condition: `0;[>100]"big"` shows 200 as `200`). Then the first section
/// whose condition holds takes the number, else the third when the first
/// two carry one (an overflow without a third), else the second, else
/// General for a code of one section. The minus sign is dropped only by a
/// section that takes negatives alone: the default second section, `[<0]`,
/// `[<-10]`, `[=-5]`, but not `[<=0]` or `[<100]`. A code with only a text
/// section shows numbers as General.
fn choose(sections: &[Section], value: f64) -> Choice<'_> {
    let Some(first) = sections.first() else {
        return Choice::General;
    };
    let second = sections.get(1);
    let second_condition = second.and_then(|second| second.condition);
    let first_condition = first
        .condition
        .or(second_condition.map(|_| Condition::POSITIVE));
    let Some(first_condition) = first_condition else {
        return match (sections.len(), second) {
            (_, Some(second)) if value < 0.0 => Choice::Section(second, false),
            (3, _) if value == 0.0 => Choice::Section(&sections[2], true),
            _ => Choice::Section(first, true),
        };
    };
    if first_condition.holds(value) {
        return Choice::Section(first, !first_condition.only_negatives());
    }
    match (second, second_condition) {
        (Some(second), Some(condition)) if condition.holds(value) => {
            Choice::Section(second, !condition.only_negatives())
        }
        (Some(_), Some(_)) => sections
            .get(2)
            .map_or(Choice::Overflow, |third| Choice::Section(third, true)),
        (Some(second), None) => Choice::Section(second, true),
        (None, _) => Choice::General,
    }
}

/// Whether a code shows a positive number as a date or a time. A code that
/// cannot be read is General, no date.
pub fn is_date_format(code: &str) -> bool {
    NumberFormat::parse(code).is_ok_and(|format| format.date_kind().is_some())
}
