//! Checked `i64` arithmetic with Brim's semantics.
//!
//! Brim integers never wrap silently: overflow and division by zero are
//! run-time errors. Division truncates toward zero, and the remainder has the
//! sign of the dividend (the same as C, Go, Java and Rust).

use crate::RuntimeError;

pub fn add(a: i64, b: i64) -> Result<i64, RuntimeError> {
    a.checked_add(b)
        .ok_or(RuntimeError::IntegerOverflow { op: "+" })
}

pub fn sub(a: i64, b: i64) -> Result<i64, RuntimeError> {
    a.checked_sub(b)
        .ok_or(RuntimeError::IntegerOverflow { op: "-" })
}

pub fn mul(a: i64, b: i64) -> Result<i64, RuntimeError> {
    a.checked_mul(b)
        .ok_or(RuntimeError::IntegerOverflow { op: "*" })
}

pub fn div(a: i64, b: i64) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError::DivisionByZero { op: "/" });
    }
    // Only i64::MIN / -1 can overflow.
    a.checked_div(b)
        .ok_or(RuntimeError::IntegerOverflow { op: "/" })
}

pub fn rem(a: i64, b: i64) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError::DivisionByZero { op: "%" });
    }
    a.checked_rem(b)
        .ok_or(RuntimeError::IntegerOverflow { op: "%" })
}

pub fn neg(a: i64) -> Result<i64, RuntimeError> {
    a.checked_neg()
        .ok_or(RuntimeError::IntegerOverflow { op: "-" })
}

/// `abs(a)`: only `i64::MIN` has no absolute value in `i64`.
pub fn abs(a: i64) -> Result<i64, RuntimeError> {
    a.checked_abs()
        .ok_or(RuntimeError::IntegerOverflow { op: "abs" })
}

/// `pow(base, exp)` on integers. A negative exponent divides:
/// `base^-n` is `1 / base^n`, rounded toward zero, so only 1 and -1 give a
/// non-zero result and 0 has none.
pub fn pow(base: i64, exp: i64) -> Result<i64, RuntimeError> {
    let overflow = RuntimeError::IntegerOverflow { op: "pow" };
    if exp < 0 {
        return match base {
            0 => Err(overflow),
            1 => Ok(1),
            -1 => Ok(if exp % 2 == 0 { 1 } else { -1 }),
            _ => Ok(0),
        };
    }
    let exp = u32::try_from(exp).map_err(|_| overflow.clone())?;
    base.checked_pow(exp).ok_or(overflow)
}

/// `a` if `min <= a <= max`: keeps `i32` and `u8` results in range.
pub fn fit(a: i64, min: i64, max: i64, op: &'static str) -> Result<i64, RuntimeError> {
    if (min..=max).contains(&a) {
        Ok(a)
    } else {
        Err(RuntimeError::IntegerOverflow { op })
    }
}

// `u64` arithmetic on the bits of an `i64`.

fn unsigned(
    a: i64,
    b: i64,
    op: &'static str,
    f: fn(u64, u64) -> Option<u64>,
) -> Result<i64, RuntimeError> {
    f(a as u64, b as u64)
        .map(|r| r as i64)
        .ok_or(RuntimeError::IntegerOverflow { op })
}

pub fn uadd(a: i64, b: i64) -> Result<i64, RuntimeError> {
    unsigned(a, b, "+", u64::checked_add)
}

pub fn usub(a: i64, b: i64) -> Result<i64, RuntimeError> {
    unsigned(a, b, "-", u64::checked_sub)
}

pub fn umul(a: i64, b: i64) -> Result<i64, RuntimeError> {
    unsigned(a, b, "*", u64::checked_mul)
}

pub fn udiv(a: i64, b: i64) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError::DivisionByZero { op: "/" });
    }
    Ok(((a as u64) / (b as u64)) as i64)
}

pub fn urem(a: i64, b: i64) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError::DivisionByZero { op: "%" });
    }
    Ok(((a as u64) % (b as u64)) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_and_pow() {
        assert_eq!(abs(-5), Ok(5));
        assert_eq!(
            abs(i64::MIN),
            Err(RuntimeError::IntegerOverflow { op: "abs" })
        );
        assert_eq!(pow(2, 10), Ok(1024));
        assert_eq!(pow(-3, 3), Ok(-27));
        assert_eq!(pow(2, 63), Err(RuntimeError::IntegerOverflow { op: "pow" }));
        assert_eq!(pow(2, -1), Ok(0));
        assert_eq!(pow(-1, -3), Ok(-1));
        assert_eq!(pow(0, -1), Err(RuntimeError::IntegerOverflow { op: "pow" }));
        assert_eq!(pow(7, 0), Ok(1));
    }

    #[test]
    fn unsigned_and_narrow_arithmetic() {
        let max = u64::MAX as i64;
        assert_eq!(uadd(max - 1, 1), Ok(max));
        assert_eq!(uadd(max, 1), Err(RuntimeError::IntegerOverflow { op: "+" }));
        assert_eq!(usub(0, 1), Err(RuntimeError::IntegerOverflow { op: "-" }));
        assert_eq!(udiv(max, 2), Ok(i64::MAX));
        assert_eq!(urem(7, 0), Err(RuntimeError::DivisionByZero { op: "%" }));
        assert_eq!(fit(255, 0, 255, "+"), Ok(255));
        assert_eq!(
            fit(256, 0, 255, "+"),
            Err(RuntimeError::IntegerOverflow { op: "+" })
        );
    }

    #[test]
    fn ordinary_arithmetic() {
        assert_eq!(add(40, 2), Ok(42));
        assert_eq!(sub(2, 44), Ok(-42));
        assert_eq!(mul(-6, 7), Ok(-42));
        assert_eq!(div(7, 2), Ok(3));
        assert_eq!(div(-7, 2), Ok(-3));
        assert_eq!(rem(-7, 2), Ok(-1));
        assert_eq!(rem(7, -2), Ok(1));
        assert_eq!(neg(5), Ok(-5));
    }

    #[test]
    fn overflow_is_an_error() {
        let overflow = |op| Err(RuntimeError::IntegerOverflow { op });
        assert_eq!(add(i64::MAX, 1), overflow("+"));
        assert_eq!(sub(i64::MIN, 1), overflow("-"));
        assert_eq!(mul(i64::MAX, 2), overflow("*"));
        assert_eq!(div(i64::MIN, -1), overflow("/"));
        assert_eq!(rem(i64::MIN, -1), overflow("%"));
        assert_eq!(neg(i64::MIN), overflow("-"));
    }

    #[test]
    fn division_by_zero_is_an_error() {
        assert_eq!(div(1, 0), Err(RuntimeError::DivisionByZero { op: "/" }));
        assert_eq!(rem(1, 0), Err(RuntimeError::DivisionByZero { op: "%" }));
    }
}
