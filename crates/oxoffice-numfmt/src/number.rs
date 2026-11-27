//! Numbers: digit placeholders, General, exponents and fractions.

use crate::decimal::Decimal;
use crate::parse::{Kind, Placeholder, Section, Token};
use crate::{Color, Formatted, Out};

pub(crate) fn format(section: &Section, value: f64, sign: bool) -> Formatted {
    match section.kind {
        Kind::General => general_section(value, sign, &section.tokens, section.color),
        Kind::Number => plain(section, value, sign),
        Kind::Exponent => exponent(section, value, sign),
        Kind::Fraction => fraction(section, value, sign),
        _ => literal(section, value, sign),
    }
}

/// Literals only: the value shows as its minus sign when it has one.
fn literal(section: &Section, value: f64, sign: bool) -> Formatted {
    let mut out = Out::default();
    if sign && value < 0.0 {
        out.push('-');
    }
    push_tokens(&mut out, &section.tokens, |_, _| {});
    out.finish(section.color)
}

fn push_tokens(out: &mut Out, tokens: &[Token], mut other: impl FnMut(&mut Out, usize)) {
    for (at, token) in tokens.iter().enumerate() {
        if out.push_plain(token).is_none() {
            other(out, at);
        }
    }
}

/// General, alone or among literals (`"Total: "General`).
pub(crate) fn general_section(
    value: f64,
    sign: bool,
    tokens: &[Token],
    color: Option<Color>,
) -> Formatted {
    let magnitude = Decimal::from_f64(value);
    let text = general(&magnitude);
    let mut out = Out::default();
    if sign && value < 0.0 && !magnitude.is_zero() {
        out.push('-');
    }
    if tokens.is_empty() {
        out.push_str(&text);
    }
    push_tokens(&mut out, tokens, |out, at| {
        if tokens[at] == Token::General {
            out.push_str(&text);
        }
    });
    out.finish(color)
}

/// Excel's General: at most 11 characters besides the minus sign, no
/// trailing zero, scientific notation with 6 significant digits from 1E+11
/// on, and below 1E-5 when the fixed form would need more than 9 decimals.
pub(crate) fn general(magnitude: &Decimal) -> String {
    if magnitude.is_zero() {
        return "0".to_owned();
    }
    let power = magnitude.magnitude();
    if power >= 11 {
        return scientific(magnitude);
    }
    if power >= 0 {
        let rounded = magnitude.rounded((9 - power).max(0));
        if rounded.magnitude() >= 11 {
            return scientific(magnitude);
        }
        return fixed(&rounded);
    }
    if power >= -4 {
        return fixed(&magnitude.rounded(9));
    }
    let rounded = magnitude.rounded(12);
    if !rounded.is_zero() && rounded.decimals() <= 9 {
        fixed(&rounded)
    } else {
        scientific(magnitude)
    }
}

fn fixed(value: &Decimal) -> String {
    let integer = value.integer_digits();
    let mut text = if integer.is_empty() {
        "0".to_owned()
    } else {
        integer
    };
    let decimals = value.decimals();
    if decimals > 0 {
        text.push('.');
        text.push_str(&value.fraction_digits(decimals));
    }
    text
}

fn scientific(value: &Decimal) -> String {
    let rounded = value.significant(6);
    let digits = rounded.digit_string();
    let mut text = digits[..1].to_owned();
    if digits.len() > 1 {
        text.push('.');
        text.push_str(&digits[1..]);
    }
    let power = rounded.magnitude();
    let sign = if power < 0 { '-' } else { '+' };
    format!("{text}E{sign}{:02}", power.unsigned_abs())
}

/// What the placeholders and commas of a number section do.
struct Layout {
    /// Token indexes of the placeholders before the decimal point (or the
    /// exponent), then after it.
    integer: Vec<usize>,
    fraction: Vec<usize>,
    group: bool,
    /// Commas that stand for themselves, by token index.
    literal_commas: Vec<usize>,
    /// The power of ten the value is multiplied by: 2 per `%`, −3 per
    /// scaling comma.
    shift: i32,
}

impl Layout {
    /// The layout of `tokens[..end]`.
    fn of(tokens: &[Token], end: usize) -> Layout {
        let tokens = &tokens[..end];
        let point = tokens
            .iter()
            .position(|token| *token == Token::Point)
            .unwrap_or(tokens.len());
        let digits = |range: std::ops::Range<usize>| -> Vec<usize> {
            range
                .filter(|at| matches!(tokens[*at], Token::Digit(_)))
                .collect()
        };
        let integer = digits(0..point);
        let fraction = digits(point.min(tokens.len())..tokens.len());
        let mut layout = Layout {
            group: false,
            literal_commas: Vec::new(),
            shift: 0,
            integer,
            fraction,
        };
        for (at, token) in tokens.iter().enumerate() {
            match token {
                Token::Percent => layout.shift += 2,
                Token::Comma if at < point => {
                    let before = layout.integer.iter().any(|digit| *digit < at);
                    let after = layout.integer.iter().any(|digit| *digit > at);
                    match (before, after) {
                        (true, true) => layout.group = true,
                        (true, false) => layout.shift -= 3,
                        (false, _) => layout.literal_commas.push(at),
                    }
                }
                // A comma between decimals is ignored; after the last one
                // it scales.
                Token::Comma if !layout.fraction.iter().any(|digit| *digit > at) => {
                    layout.shift -= 3;
                }
                _ => {}
            }
        }
        layout
    }
}

/// What each placeholder of a right-aligned run shows: `digits` from the
/// right, the extra ones on the first placeholder, the pad of the others,
/// and a comma after every third digit from the right when `group`.
fn right_aligned(placeholders: &[Placeholder], digits: &str, group: bool) -> Vec<String> {
    let digits: Vec<char> = digits.chars().collect();
    let count = placeholders.len();
    let comma = |position: usize, share: &mut String| {
        if group && position > 0 && position.is_multiple_of(3) {
            share.push(',');
        }
    };
    placeholders
        .iter()
        .enumerate()
        .map(|(index, placeholder)| {
            let position = count - 1 - index;
            let mut share = String::new();
            if index == 0 {
                for extra in (count..digits.len()).rev() {
                    share.push(digits[digits.len() - 1 - extra]);
                    comma(extra, &mut share);
                }
            }
            if position < digits.len() {
                share.push(digits[digits.len() - 1 - position]);
                comma(position, &mut share);
            } else if let Some(pad) = placeholder.pad() {
                share.push(pad);
                if pad == '0' {
                    comma(position, &mut share);
                }
            }
            share
        })
        .collect()
}

/// What each decimal placeholder shows: the digit up to the last nonzero
/// one or the last `0` placeholder, the pad of the placeholder after.
fn decimals(placeholders: &[Placeholder], digits: &str) -> Vec<String> {
    let digits: Vec<char> = digits.chars().collect();
    let significant = digits.iter().rposition(|digit| *digit != '0');
    let required = placeholders
        .iter()
        .rposition(|placeholder| *placeholder == Placeholder::Zero);
    let keep = significant.max(required);
    placeholders
        .iter()
        .enumerate()
        .map(|(index, placeholder)| {
            if keep.is_some_and(|keep| index <= keep) {
                digits[index].to_string()
            } else {
                placeholder.pad().map(String::from).unwrap_or_default()
            }
        })
        .collect()
}

fn placeholders(tokens: &[Token], indexes: &[usize]) -> Vec<Placeholder> {
    indexes
        .iter()
        .map(|at| match tokens[*at] {
            Token::Digit(placeholder) => placeholder,
            _ => unreachable!("placeholder indexes point at digits"),
        })
        .collect()
}

/// Writes `tokens[range]` of a number: placeholders from the shares, the
/// integer digits at the point when no placeholder is before it.
#[allow(clippy::too_many_arguments)]
fn push_number(
    out: &mut Out,
    tokens: &[Token],
    range: std::ops::Range<usize>,
    layout: &Layout,
    integer_shares: &[String],
    integer_digits: &str,
    fraction_shares: &[String],
) {
    for at in range {
        match &tokens[at] {
            Token::Digit(_) => {
                if let Some(index) = layout.integer.iter().position(|digit| *digit == at) {
                    out.push_str(&integer_shares[index]);
                } else if let Some(index) = layout.fraction.iter().position(|digit| *digit == at) {
                    out.push_str(&fraction_shares[index]);
                }
            }
            Token::Point => {
                if layout.integer.is_empty() {
                    out.push_str(integer_digits);
                }
                out.push('.');
            }
            Token::Comma if layout.literal_commas.contains(&at) => out.push(','),
            Token::Comma => {}
            Token::Percent => out.push('%'),
            token => {
                out.push_plain(token);
            }
        }
    }
}

/// `0`, `#`, `?`, the decimal point, commas and `%`.
fn plain(section: &Section, value: f64, sign: bool) -> Formatted {
    let tokens = &section.tokens;
    let layout = Layout::of(tokens, tokens.len());
    let magnitude = Decimal::from_f64(value)
        .shifted(layout.shift)
        .rounded(layout.fraction.len() as i32);
    let integer = magnitude.integer_digits();
    let integer_shares = right_aligned(
        &placeholders(tokens, &layout.integer),
        &integer,
        layout.group,
    );
    let fraction_shares = decimals(
        &placeholders(tokens, &layout.fraction),
        &magnitude.fraction_digits(layout.fraction.len()),
    );
    let mut out = Out::default();
    // A number that rounds to zero shows no minus sign.
    if sign && value < 0.0 && !magnitude.is_zero() {
        out.push('-');
    }
    push_number(
        &mut out,
        tokens,
        0..tokens.len(),
        &layout,
        &integer_shares,
        &integer,
        &fraction_shares,
    );
    out.finish(section.color)
}

/// `0.00E+00`; with more than one placeholder before the point the
/// exponent is a multiple of their count (`##0.0E+0`, engineering).
fn exponent(section: &Section, value: f64, sign: bool) -> Formatted {
    let tokens = &section.tokens;
    let at_exponent = tokens
        .iter()
        .position(|token| matches!(token, Token::Exponent { .. }))
        .expect("an exponent section has an exponent");
    let layout = Layout::of(tokens, at_exponent);
    let percent = 2 * tokens
        .iter()
        .filter(|token| **token == Token::Percent)
        .count() as i32;
    let base = Decimal::from_f64(value).shifted(percent);
    let step = layout.integer.len().max(1) as i32;
    let decimals = layout.fraction.len() as i32;
    let (mantissa, power) = if base.is_zero() {
        (Decimal::ZERO, 0)
    } else {
        let mut leading = base.magnitude();
        loop {
            let power = leading.div_euclid(step) * step;
            let mantissa = base.shifted(-power).rounded(decimals);
            if mantissa.magnitude() < step {
                break (mantissa, power);
            }
            leading = mantissa.magnitude() + power;
        }
    };
    let integer = mantissa.integer_digits();
    let integer_shares = right_aligned(&placeholders(tokens, &layout.integer), &integer, false);
    let fraction_shares = decimals_of(&layout, tokens, &mantissa);
    let mut out = Out::default();
    if sign && value < 0.0 && !mantissa.is_zero() {
        out.push('-');
    }
    let exponent_layout = Layout {
        integer: (at_exponent + 1..tokens.len())
            .filter(|at| matches!(tokens[*at], Token::Digit(_)))
            .collect(),
        fraction: Vec::new(),
        group: false,
        literal_commas: Vec::new(),
        shift: 0,
    };
    let exponent_digits = if power == 0 {
        String::new()
    } else {
        power.unsigned_abs().to_string()
    };
    let exponent_shares = right_aligned(
        &placeholders(tokens, &exponent_layout.integer),
        &exponent_digits,
        false,
    );
    push_number(
        &mut out,
        tokens,
        0..at_exponent,
        &layout,
        &integer_shares,
        &integer,
        &fraction_shares,
    );
    let Token::Exponent { plus } = tokens[at_exponent] else {
        unreachable!("the exponent index points at the exponent");
    };
    out.push('E');
    if power < 0 {
        out.push('-');
    } else if plus {
        out.push('+');
    }
    push_number(
        &mut out,
        tokens,
        at_exponent + 1..tokens.len(),
        &exponent_layout,
        &exponent_shares,
        "",
        &[],
    );
    out.finish(section.color)
}

fn decimals_of(layout: &Layout, tokens: &[Token], value: &Decimal) -> Vec<String> {
    decimals(
        &placeholders(tokens, &layout.fraction),
        &value.fraction_digits(layout.fraction.len()),
    )
}

/// `# ?/?`, `# ??/16`, `?/?`: a whole part when the code has placeholders
/// before the numerator, the closest fraction Excel's continued fraction
/// finds, or the fixed denominator.
fn fraction(section: &Section, value: f64, sign: bool) -> Formatted {
    let tokens = &section.tokens;
    let slash = tokens
        .iter()
        .enumerate()
        .position(|(at, token)| {
            *token == Token::Slash && at > 0 && matches!(tokens[at - 1], Token::Digit(_))
        })
        .expect("a fraction section has a slash after a placeholder");
    let numerator_start = (0..slash)
        .rev()
        .take_while(|at| matches!(tokens[*at], Token::Digit(_)))
        .last()
        .expect("a placeholder is before the slash");
    let numerator: Vec<usize> = (numerator_start..slash).collect();
    let fixed: String = tokens[slash + 1..]
        .iter()
        .map_while(|token| match token {
            Token::Literal(text) if text.len() == 1 && text.as_bytes()[0].is_ascii_digit() => {
                text.chars().next()
            }
            Token::Digit(Placeholder::Zero) => Some('0'),
            _ => None,
        })
        .collect();
    // A denominator written as a number (`# ??/16`) starts with a digit
    // other than 0, which reads as a placeholder; one too large for any
    // fraction is an overflow.
    let fixed = if fixed.starts_with(|c: char| c.is_ascii_digit() && c != '0') {
        match fixed.parse::<u64>() {
            Ok(denominator) => Some(denominator),
            Err(_) => return Formatted::overflow(section.color),
        }
    } else {
        None
    };
    let denominator_end = slash
        + 1
        + match fixed {
            Some(_) => tokens[slash + 1..]
                .iter()
                .take_while(|token| {
                    matches!(token, Token::Literal(text) if text.len() == 1 && text.as_bytes()[0].is_ascii_digit())
                        || **token == Token::Digit(Placeholder::Zero)
                })
                .count(),
            None => tokens[slash + 1..]
                .iter()
                .take_while(|token| matches!(token, Token::Digit(_)))
                .count(),
        };
    let denominator: Vec<usize> = (slash + 1..denominator_end).collect();
    let before = Layout::of(tokens, numerator_start);
    let mixed = !before.integer.is_empty();
    let percents = |tokens: &[Token]| {
        tokens
            .iter()
            .filter(|token| **token == Token::Percent)
            .count() as i32
    };
    let scale = before.shift - 2 * percents(&tokens[..numerator_start]);
    let magnitude = Decimal::from_f64(value).shifted(scale + 2 * percents(tokens));
    let (mut whole, part) = if mixed {
        (magnitude.integer_part(), magnitude.fraction_part())
    } else {
        (Decimal::ZERO, magnitude.clone())
    };
    // The numerator in decimal: `?/100` takes 1E+37 as 1E+39 hundredths.
    let (mut top, bottom) = match fixed {
        Some(denominator) => (part.times(denominator).rounded(0), u128::from(denominator)),
        // A whole number of more than 15 digits has no fraction: itself
        // over one.
        None if !mixed && part.magnitude() >= 15 => (part.integer_part(), 1),
        None => {
            let digits = u32::try_from(denominator.len().min(7)).expect("seven at most");
            match continued_fraction(&part, 10u128.pow(digits) - 1) {
                Some((numerator, denominator)) => (Decimal::from_integer(numerator), denominator),
                None => return Formatted::overflow(section.color),
            }
        }
    };
    // Excel shows `?/100` with 1E+37 as `#####`: a numerator beyond its 15
    // digits is an overflow (the limit is ours).
    if top.magnitude() >= 15 {
        return Formatted::overflow(section.color);
    }
    if mixed && top == Decimal::from_integer(bottom) {
        whole = whole.plus_one();
        top = Decimal::ZERO;
    }
    let mut out = Out::default();
    if sign && value < 0.0 {
        out.push('-');
    }
    let blank = mixed && top.is_zero();
    let whole_digits = match whole.integer_digits() {
        digits if digits.is_empty() && blank => "0".to_owned(),
        digits => digits,
    };
    let whole_shares = right_aligned(
        &placeholders(tokens, &before.integer),
        &whole_digits,
        before.group,
    );
    push_number(
        &mut out,
        tokens,
        0..numerator_start,
        &before,
        &whole_shares,
        &whole_digits,
        &[],
    );
    if blank {
        out.push_str(&" ".repeat(denominator_end - numerator_start));
    } else {
        let top = match top.integer_digits() {
            digits if digits.is_empty() => "0".to_owned(),
            digits => digits,
        };
        for share in right_aligned(&placeholders(tokens, &numerator), &top, false) {
            out.push_str(&share);
        }
        out.push('/');
        match fixed {
            Some(denominator) => out.push_str(&denominator.to_string()),
            None => {
                let digits: Vec<char> = bottom.to_string().chars().collect();
                for (index, placeholder) in placeholders(tokens, &denominator).iter().enumerate() {
                    match digits.get(index) {
                        Some(digit) => out.push(*digit),
                        None => {
                            if let Some(pad) = placeholder.pad() {
                                out.push(pad);
                            }
                        }
                    }
                }
            }
        }
    }
    let after = Layout {
        integer: Vec::new(),
        fraction: Vec::new(),
        group: false,
        literal_commas: Vec::new(),
        shift: 0,
    };
    push_number(
        &mut out,
        tokens,
        denominator_end..tokens.len(),
        &after,
        &[],
        "",
        &[],
    );
    out.finish(section.color)
}

/// Excel's fraction: the last convergent of the continued fraction of
/// `value` whose denominator is at most `limit` (no semiconvergent, so
/// `# ??/??` shows pi as 3 1/7), stopping when the remainder is below
/// 5E-8. `None` when the value is too large for exact arithmetic.
fn continued_fraction(value: &Decimal, limit: u128) -> Option<(u128, u128)> {
    if value.is_zero() || value.magnitude() < -9 {
        return Some((0, 1));
    }
    let (mut numerator, mut denominator) = value.as_ratio()?;
    let (mut p_before, mut p_last, mut q_before, mut q_last) = (0u128, 1u128, 1u128, 0u128);
    let (mut p, mut q) = (0u128, 1u128);
    while q_last < limit {
        let whole = numerator / denominator;
        p = whole.checked_mul(p_last)?.checked_add(p_before)?;
        q = whole.checked_mul(q_last)?.checked_add(q_before)?;
        let remainder = numerator - whole * denominator;
        if remainder.checked_mul(20_000_000)? < denominator {
            break;
        }
        (numerator, denominator) = (denominator, remainder);
        (p_before, p_last, q_before, q_last) = (p_last, p, q_last, q);
    }
    if q > limit {
        (p, q) = if q_last > limit {
            (p_before, q_before)
        } else {
            (p_last, q_last)
        };
    }
    Some((p, q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_takes_convergents_only() {
        let fraction = |value: f64, limit| continued_fraction(&Decimal::from_f64(value), limit);
        assert_eq!(fraction(0.14159265358979, 99), Some((1, 7)));
        assert_eq!(fraction(0.456, 99), Some((26, 57)));
        assert_eq!(fraction(0.333, 999), Some((1, 3)));
        assert_eq!(fraction(0.1, 9), Some((0, 1)));
        assert_eq!(fraction(0.999, 99), Some((1, 1)));
    }
}
