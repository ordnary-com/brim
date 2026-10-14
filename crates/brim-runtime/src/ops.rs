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

#[cfg(test)]
mod tests {
    use super::*;

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
