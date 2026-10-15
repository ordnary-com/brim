//! The random number generator of `std.random`: xoshiro256**, seeded with
//! splitmix64. The C runtime has the same generator, so a program that
//! calls `random.seed(n)` gets the same numbers in the interpreter and in
//! native code. Without a seed, the state comes from the operating system's
//! randomness.

use std::cell::Cell;

thread_local! {
    static STATE: Cell<Option<[u64; 4]>> = const { Cell::new(None) };
}

fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn state_from(mut seed: u64) -> [u64; 4] {
    [
        splitmix64(&mut seed),
        splitmix64(&mut seed),
        splitmix64(&mut seed),
        splitmix64(&mut seed),
    ]
}

/// A seed from the operating system: `/dev/urandom`, or the clock and the
/// process id where that is missing.
fn os_seed() -> u64 {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok()
    {
        return u64::from_le_bytes(bytes);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    nanos ^ (u64::from(std::process::id()) << 32)
}

/// `random.seed(n)`: the numbers that follow are the same on every run.
pub fn seed(n: i64) {
    STATE.with(|s| s.set(Some(state_from(n as u64))));
}

/// 64 random bits.
pub fn next() -> u64 {
    STATE.with(|cell| {
        let mut s = cell.get().unwrap_or_else(|| state_from(os_seed()));
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        cell.set(Some(s));
        result
    })
}

/// A number from `low` up to, but not including, `high`, each as likely;
/// `low` when `high <= low`.
pub fn range(low: i64, high: i64) -> i64 {
    if high <= low {
        return low;
    }
    let span = (high as u64).wrapping_sub(low as u64);
    // Numbers below `threshold` would make some results more likely.
    let threshold = span.wrapping_neg() % span;
    loop {
        let r = next();
        if r >= threshold {
            return (low as u64).wrapping_add(r % span) as i64;
        }
    }
}

/// A number from 0 up to, but not including, 1.
pub fn float() -> f64 {
    (next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_gives_the_same_numbers() {
        seed(42);
        let first: Vec<u64> = (0..3).map(|_| next()).collect();
        seed(42);
        let again: Vec<u64> = (0..3).map(|_| next()).collect();
        assert_eq!(first, again);
        seed(43);
        assert_ne!(next(), first[0]);
    }

    #[test]
    fn ranges_stay_inside() {
        seed(1);
        for _ in 0..1000 {
            let n = range(-3, 4);
            assert!((-3..4).contains(&n));
            let x = float();
            assert!((0.0..1.0).contains(&x));
        }
        assert_eq!(range(5, 5), 5);
        let wide = range(i64::MIN, i64::MAX);
        assert!(wide < i64::MAX);
    }
}
