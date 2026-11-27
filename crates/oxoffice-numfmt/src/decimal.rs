//! A non-negative value as the decimal digits Excel shows: at most 15
//! significant digits, rounded half away from zero from there on.

/// `digits` × 10^(`point` − `digits.len()`): the digits run from the most
/// significant one and the decimal point sits `point` digits after the first
/// (before it when negative). No leading or trailing zero; zero has no
/// digits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decimal {
    digits: Vec<u8>,
    point: i32,
}

impl Decimal {
    pub(crate) const ZERO: Decimal = Decimal {
        digits: Vec::new(),
        point: 0,
    };

    /// The magnitude of `value` to 15 significant digits, as Excel holds a
    /// cell value. Subnormal doubles read as zero, as in Excel.
    pub(crate) fn from_f64(value: f64) -> Decimal {
        let value = value.abs();
        if !value.is_finite() || value < f64::MIN_POSITIVE {
            return Decimal::ZERO;
        }
        let text = format!("{value:.14e}");
        let (mantissa, exponent) = text.split_once('e').expect("exponent form");
        let exponent: i32 = exponent.parse().expect("exponent digits");
        let digits = mantissa.bytes().filter(u8::is_ascii_digit).collect();
        Decimal::normalised(digits, exponent + 1)
    }

    fn normalised(mut digits: Vec<u8>, mut point: i32) -> Decimal {
        while digits.last() == Some(&b'0') {
            digits.pop();
        }
        let leading = digits.iter().take_while(|digit| **digit == b'0').count();
        digits.drain(..leading);
        point -= leading as i32;
        if digits.is_empty() {
            return Decimal::ZERO;
        }
        Decimal { digits, point }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// A whole number.
    pub(crate) fn from_integer(value: u128) -> Decimal {
        let digits = value.to_string().into_bytes();
        let point = digits.len() as i32;
        Decimal::normalised(digits, point)
    }

    /// The value × `factor`, exactly.
    pub(crate) fn times(&self, factor: u64) -> Decimal {
        if self.is_zero() || factor == 0 {
            return Decimal::ZERO;
        }
        // Schoolbook multiplication on the digits, least significant first.
        let mut product = vec![0u128; self.digits.len() + 20];
        for (at, digit) in self.digits.iter().rev().enumerate() {
            product[at] += u128::from(digit - b'0') * u128::from(factor);
        }
        for at in 0..product.len() - 1 {
            product[at + 1] += product[at] / 10;
            product[at] %= 10;
        }
        let extra = product.len() as i32 - self.digits.len() as i32;
        let digits = product
            .iter()
            .rev()
            .map(|digit| b'0' + *digit as u8)
            .collect();
        Decimal::normalised(digits, self.point.saturating_add(extra))
    }

    pub(crate) fn to_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        format!("0.{}e{}", self.digit_string(), self.point)
            .parse()
            .expect("decimal digits read as a number")
    }

    /// The power of ten of the first digit (`2` for `123.4`, `-3` for
    /// `0.0012`).
    pub(crate) fn magnitude(&self) -> i32 {
        self.point - 1
    }

    /// The value × 10^`places`.
    pub(crate) fn shifted(&self, places: i32) -> Decimal {
        if self.is_zero() {
            return Decimal::ZERO;
        }
        Decimal {
            digits: self.digits.clone(),
            point: self.point.saturating_add(places),
        }
    }

    /// Rounded half away from zero to `decimals` places after the point
    /// (a negative count rounds to tens, hundreds...).
    pub(crate) fn rounded(&self, decimals: i32) -> Decimal {
        let keep = self.point.saturating_add(decimals);
        if keep < 0 || self.is_zero() {
            return Decimal::ZERO;
        }
        let keep = keep as usize;
        if keep >= self.digits.len() {
            return self.clone();
        }
        let mut digits = self.digits[..keep].to_vec();
        let mut point = self.point;
        if self.digits[keep] >= b'5' {
            let mut at = keep;
            loop {
                if at == 0 {
                    digits.insert(0, b'1');
                    point += 1;
                    break;
                }
                at -= 1;
                if digits[at] == b'9' {
                    digits[at] = b'0';
                } else {
                    digits[at] += 1;
                    break;
                }
            }
        }
        Decimal::normalised(digits, point)
    }

    /// Rounded to `count` significant digits.
    pub(crate) fn significant(&self, count: i32) -> Decimal {
        self.rounded(count - self.point)
    }

    /// The digits before the point, empty for a value below one.
    pub(crate) fn integer_digits(&self) -> String {
        if self.point <= 0 {
            return String::new();
        }
        let point = self.point as usize;
        let mut text: String = self.digits[..point.min(self.digits.len())]
            .iter()
            .map(|digit| char::from(*digit))
            .collect();
        text.extend(std::iter::repeat_n(
            '0',
            point.saturating_sub(self.digits.len()),
        ));
        text
    }

    /// The first `count` digits after the point, padded with zeros.
    pub(crate) fn fraction_digits(&self, count: usize) -> String {
        (0..count)
            .map(|at| {
                let index = self.point + at as i32;
                if index < 0 {
                    '0'
                } else {
                    self.digits
                        .get(index as usize)
                        .map_or('0', |digit| char::from(*digit))
                }
            })
            .collect()
    }

    /// How many digits the value has after the point.
    pub(crate) fn decimals(&self) -> usize {
        (self.digits.len() as i32 - self.point).max(0) as usize
    }

    /// The significant digits, without the point.
    pub(crate) fn digit_string(&self) -> String {
        self.digits.iter().map(|digit| char::from(*digit)).collect()
    }

    /// The value as the fraction `numerator / 10^decimals`, when it fits.
    pub(crate) fn as_ratio(&self) -> Option<(u128, u128)> {
        let decimals = u32::try_from(self.decimals()).ok()?;
        let denominator = 10u128.checked_pow(decimals)?;
        let mut numerator: u128 = 0;
        for digit in &self.digits {
            numerator = numerator
                .checked_mul(10)?
                .checked_add(u128::from(digit - b'0'))?;
        }
        let trailing = u32::try_from((self.point - self.digits.len() as i32).max(0)).ok()?;
        numerator = numerator.checked_mul(10u128.checked_pow(trailing)?)?;
        Some((numerator, denominator))
    }

    /// The part after the point.
    pub(crate) fn fraction_part(&self) -> Decimal {
        if self.point <= 0 {
            return self.clone();
        }
        let point = self.point as usize;
        if point >= self.digits.len() {
            return Decimal::ZERO;
        }
        Decimal::normalised(self.digits[point..].to_vec(), 0)
    }

    /// The part before the point.
    pub(crate) fn integer_part(&self) -> Decimal {
        if self.point <= 0 {
            return Decimal::ZERO;
        }
        let point = self.point as usize;
        Decimal::normalised(
            self.digits[..point.min(self.digits.len())].to_vec(),
            self.point,
        )
    }

    pub(crate) fn plus_one(&self) -> Decimal {
        let mut integer: Vec<u8> = self.integer_digits().into_bytes();
        let mut at = integer.len();
        loop {
            if at == 0 {
                integer.insert(0, b'1');
                break;
            }
            at -= 1;
            if integer[at] == b'9' {
                integer[at] = b'0';
            } else {
                integer[at] += 1;
                break;
            }
        }
        let point = integer.len() as i32;
        Decimal::normalised(integer, point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_keeps_fifteen_digits_and_rounds_half_away_from_zero() {
        // 1.005 is 1.00499999999999989... in binary: Excel shows 1.01.
        let value = Decimal::from_f64(1.005);
        assert_eq!(value.rounded(2).fraction_digits(2), "01");
        let carried = Decimal::from_f64(9.995).significant(3);
        assert_eq!(
            (carried.digit_string(), carried.magnitude()),
            ("1".into(), 1)
        );
        assert_eq!(Decimal::from_f64(1.0e20).integer_digits().len(), 21);
        assert_eq!(Decimal::from_f64(0.000123).fraction_digits(6), "000123");
        assert!(Decimal::from_f64(2.0e-308).is_zero());
        assert_eq!(Decimal::from_f64(0.375).as_ratio(), Some((375, 1000)));
        assert_eq!(Decimal::from_f64(999.0).plus_one().integer_digits(), "1000");
        let product = Decimal::from_f64(1.0e37).times(100);
        assert_eq!(
            (product.digit_string(), product.magnitude()),
            ("1".into(), 39)
        );
        assert_eq!(
            Decimal::from_f64(0.3).times(16).rounded(0),
            Decimal::from_integer(5)
        );
    }
}
