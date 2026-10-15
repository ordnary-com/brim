//! `f64` as text: `to_string`, `print` and `parse_float`. The C runtime
//! formats and parses the same way, so programs print the same numbers
//! natively and in the interpreter.
//!
//! A number is written with the fewest digits that read back as exactly
//! the same number. Numbers from 1e-5 up to (not including) 1e16 are
//! written as decimals, with `.0` when they are whole (`2.0`, `0.001`);
//! others in scientific notation (`1e16`, `1.5e-7`). The special values are
//! `NaN`, `inf` and `-inf`; negative zero is `-0.0`.

/// The text of a number.
pub fn format_f64(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x == 0.0 {
        return format!("{sign}0.0");
    }
    // The fewest digits that read back exactly, correctly rounded: the
    // first precision that round-trips. (`{:e}` alone is also shortest,
    // but may pick the farther of two equally short candidates; the C
    // runtime does it this way, so both agree.)
    let abs = x.abs();
    let sci = (0..=17)
        .map(|p| format!("{abs:.p$e}"))
        .find(|t| t.parse::<f64>().ok() == Some(abs))
        .expect("17 significant digits always round-trip");
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exp: i32 = exp.parse().expect("`{:e}` has a numeric exponent");
    format!("{sign}{}", layout(&digits, exp))
}

/// Lays out significant `digits` (the first is non-zero) whose first digit
/// stands for 10^`exp`.
pub fn layout(digits: &str, exp: i32) -> String {
    if (-5..16).contains(&exp) {
        if exp >= 0 {
            let int_len = exp as usize + 1;
            if digits.len() <= int_len {
                let zeros = "0".repeat(int_len - digits.len());
                format!("{digits}{zeros}.0")
            } else {
                format!("{}.{}", &digits[..int_len], &digits[int_len..])
            }
        } else {
            let zeros = "0".repeat((-exp - 1) as usize);
            format!("0.{zeros}{digits}")
        }
    } else if digits.len() == 1 {
        format!("{digits}e{exp}")
    } else {
        format!("{}.{}e{exp}", &digits[..1], &digits[1..])
    }
}

/// Whether `text` is a decimal number as `parse_float` accepts it: an
/// optional sign, digits, optionally a fraction and an exponent
/// (`-1.5e3`).
pub fn is_float_text(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if !digits(&mut i) {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// `s.parse_float()`: the nearest `f64`, `inf` for numbers too large.
pub fn parse_f64(text: &str) -> Option<f64> {
    if !is_float_text(text) {
        return None;
    }
    text.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (2.5, "2.5"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (100.0, "100.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e16"),
            (1.5e16, "1.5e16"),
            (0.00001, "0.00001"),
            (0.000001, "1e-6"),
            (1.5e-7, "1.5e-7"),
            (-123.456, "-123.456"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (5e-324, "5e-324"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
        ];
        for (x, text) in cases {
            assert_eq!(format_f64(x), text, "{x:?}");
        }
    }

    #[test]
    fn parses() {
        assert_eq!(parse_f64("1.5"), Some(1.5));
        assert_eq!(parse_f64("-2e3"), Some(-2000.0));
        assert_eq!(parse_f64("+0.25E-1"), Some(0.025));
        assert_eq!(parse_f64("1e999"), Some(f64::INFINITY));
        for bad in [
            "", "1.", ".5", "1e", "inf", "NaN", "1.5x", " 1", "1_0", "--1", "0x10",
        ] {
            assert_eq!(parse_f64(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn round_trips() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let f = f64::from_bits(x);
            if f.is_finite() {
                let text = format_f64(f);
                let back: f64 = text.parse().unwrap();
                assert_eq!(back.to_bits(), f.to_bits(), "{text}");
            }
        }
    }
}
