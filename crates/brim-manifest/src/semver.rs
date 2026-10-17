//! Semantic versions and version requirements (see `docs/registry.md`).
//!
//! ```text
//! 1.2.3  ^1.2  ~1.2.3  =1.2.3  >=1.2, <2  *
//! ```

use std::cmp::Ordering;
use std::fmt;

/// A version: `MAJOR.MINOR.PATCH` with an optional pre-release part.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The dot-separated identifiers after `-`; empty for a release.
    pub pre: Vec<String>,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
            pre: Vec::new(),
        }
    }

    /// Parses `1.2.3` or `1.2.3-beta.1`.
    pub fn parse(text: &str) -> Option<Version> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.').map(number);
        let (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) = (
            numbers.next(),
            numbers.next(),
            numbers.next(),
            numbers.next(),
        ) else {
            return None;
        };
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|part| {
                    let ok = !part.is_empty()
                        && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                        && (!part.chars().all(|c| c.is_ascii_digit())
                            || part.len() == 1
                            || !part.starts_with('0'));
                    ok.then(|| part.to_string())
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

/// A number without leading zeros.
fn number(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.chars().all(|c| c.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        self.triple().cmp(&other.triple()).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release comes before its release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Pre-release precedence: numeric identifiers by value and before
/// alphanumeric ones, which compare as text; a longer list wins a tie.
fn compare_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

/// A version requirement: comparators that must all hold.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VersionReq {
    comparators: Vec<Comparator>,
    /// As written, for messages and the lock file.
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Comparator {
    op: Op,
    major: u64,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    Caret,
    Tilde,
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
}

impl VersionReq {
    /// Any version (`*`).
    pub fn any() -> VersionReq {
        VersionReq {
            comparators: Vec::new(),
            text: "*".into(),
        }
    }

    /// Parses a requirement, or says what is wrong with it.
    pub fn parse(text: &str) -> Result<VersionReq, String> {
        let trimmed = text.trim();
        if trimmed == "*" {
            return Ok(VersionReq::any());
        }
        if trimmed.is_empty() {
            return Err("the version requirement is empty".into());
        }
        let comparators = trimmed
            .split(',')
            .map(|part| parse_comparator(part.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VersionReq {
            comparators,
            text: trimmed.to_string(),
        })
    }

    pub fn matches(&self, version: &Version) -> bool {
        if !self.comparators.iter().all(|c| c.matches(version)) {
            return false;
        }
        // Pre-releases only when a comparator asks for one of the same
        // MAJOR.MINOR.PATCH.
        !version.is_prerelease()
            || self.comparators.iter().any(|c| {
                !c.pre.is_empty()
                    && c.major == version.major
                    && c.minor == Some(version.minor)
                    && c.patch == Some(version.patch)
            })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn parse_comparator(text: &str) -> Result<Comparator, String> {
    let (op, rest) = [
        (">=", Op::GreaterEq),
        ("<=", Op::LessEq),
        (">", Op::Greater),
        ("<", Op::Less),
        ("=", Op::Exact),
        ("^", Op::Caret),
        ("~", Op::Tilde),
    ]
    .into_iter()
    .find_map(|(prefix, op)| {
        text.strip_prefix(prefix)
            .map(|rest| (op, rest.trim_start()))
    })
    .unwrap_or((Op::Caret, text));
    let bad = || {
        format!(
            "`{text}` is not a version requirement; write a version like `1.2` or `1.2.3`, \
             optionally after `^`, `~`, `=`, `>`, `>=`, `<` or `<=`"
        )
    };
    if rest.contains('+') {
        return Err(format!(
            "`{text}`: versions have no build metadata (`+...`)"
        ));
    }
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err(bad());
    }
    let numbers = parts
        .iter()
        .map(|p| number(p))
        .collect::<Option<Vec<u64>>>()
        .ok_or_else(bad)?;
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => {
            if numbers.len() != 3 {
                return Err(format!(
                    "`{text}`: a pre-release needs a full version, like `1.2.0-beta.1`"
                ));
            }
            Version::parse(&format!("0.0.0-{pre}")).ok_or_else(bad)?.pre
        }
    };
    Ok(Comparator {
        op,
        major: numbers[0],
        minor: numbers.get(1).copied(),
        patch: numbers.get(2).copied(),
        pre,
    })
}

impl Comparator {
    /// The lowest version the comparator names, with missing parts as 0.
    fn floor(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor.unwrap_or(0),
            patch: self.patch.unwrap_or(0),
            pre: self.pre.clone(),
        }
    }

    fn matches(&self, v: &Version) -> bool {
        let floor = self.floor();
        // `v` without its pre-release, so `<2` keeps `2.0.0-beta` out.
        let release = Version::new(v.major, v.minor, v.patch);
        match self.op {
            Op::Exact => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v == floor,
                (Some(minor), None) => v.major == self.major && v.minor == minor,
                (None, _) => v.major == self.major,
            },
            Op::Greater => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v > floor,
                (Some(minor), None) => (v.major, v.minor) > (self.major, minor),
                (None, _) => v.major > self.major,
            },
            Op::GreaterEq => *v >= floor,
            Op::Less => {
                if self.pre.is_empty() {
                    release < floor && *v < floor
                } else {
                    *v < floor
                }
            }
            Op::LessEq => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v <= floor,
                (Some(minor), None) => (v.major, v.minor) <= (self.major, minor),
                (None, _) => v.major <= self.major,
            },
            Op::Tilde => {
                *v >= floor
                    && match self.minor {
                        Some(minor) => v.major == self.major && v.minor == minor,
                        None => v.major == self.major,
                    }
            }
            Op::Caret => {
                if *v < floor {
                    return false;
                }
                match (self.major, self.minor, self.patch) {
                    (0, Some(0), Some(patch)) => v.major == 0 && v.minor == 0 && v.patch == patch,
                    (0, Some(minor), _) => v.major == 0 && v.minor == minor,
                    (major, _, _) => v.major == major,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn matches(req: &str, version: &str) -> bool {
        VersionReq::parse(req).unwrap().matches(&v(version))
    }

    #[test]
    fn versions() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("1.0.0-beta.1").to_string(), "1.0.0-beta.1");
        for bad in [
            "", "1", "1.2", "1.2.3.4", "01.2.3", "1.0.0-", "1.0.0-01", "a.b.c",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        let mut list: Vec<Version> = [
            "1.0.0",
            "1.0.0-rc.1",
            "1.0.0-beta.11",
            "1.0.0-beta.2",
            "1.0.0-beta",
            "1.0.0-alpha.beta",
            "1.0.0-alpha.1",
            "1.0.0-alpha",
            "0.9.10",
            "0.9.9",
        ]
        .iter()
        .map(|s| v(s))
        .collect();
        list.sort();
        let sorted: Vec<String> = list.iter().map(ToString::to_string).collect();
        assert_eq!(
            sorted,
            [
                "0.9.9",
                "0.9.10",
                "1.0.0-alpha",
                "1.0.0-alpha.1",
                "1.0.0-alpha.beta",
                "1.0.0-beta",
                "1.0.0-beta.2",
                "1.0.0-beta.11",
                "1.0.0-rc.1",
                "1.0.0"
            ]
        );
    }

    #[test]
    fn caret() {
        for (req, yes, no) in [
            ("1.2.3", &["1.2.3", "1.9.0"][..], &["1.2.2", "2.0.0"][..]),
            ("^1.2", &["1.2.0", "1.99.1"], &["1.1.9", "2.0.0"]),
            ("1", &["1.0.0", "1.5.0"], &["0.9.0", "2.0.0"]),
            ("^0.2.3", &["0.2.3", "0.2.9"], &["0.3.0", "0.2.2"]),
            ("^0.0.3", &["0.0.3"], &["0.0.4", "0.0.2"]),
            ("0.2", &["0.2.0", "0.2.7"], &["0.3.0"]),
            ("0", &["0.0.1", "0.9.0"], &["1.0.0"]),
        ] {
            for version in yes {
                assert!(matches(req, version), "{req} should match {version}");
            }
            for version in no {
                assert!(!matches(req, version), "{req} should not match {version}");
            }
        }
    }

    #[test]
    fn other_operators() {
        assert!(matches("~1.2.3", "1.2.9"));
        assert!(!matches("~1.2.3", "1.3.0"));
        assert!(matches("~1", "1.9.0"));
        assert!(matches("=1.2.3", "1.2.3"));
        assert!(!matches("=1.2.3", "1.2.4"));
        assert!(matches("=1.2", "1.2.7"));
        assert!(matches(">1.2", "1.3.0"));
        assert!(!matches(">1.2", "1.2.9"));
        assert!(matches(">=1.2, <1.5", "1.4.9"));
        assert!(!matches(">=1.2, <1.5", "1.5.0"));
        assert!(matches("<=1.4", "1.4.9"));
        assert!(!matches("<=1.4", "1.5.0"));
        assert!(matches("*", "7.0.0"));
        assert!(!matches("*", "7.0.0-beta"));
    }

    #[test]
    fn prereleases() {
        assert!(!matches("^1.2", "1.3.0-beta.1"));
        assert!(matches("^1.3.0-beta.1", "1.3.0-beta.2"));
        assert!(matches("^1.3.0-beta.1", "1.3.0"));
        assert!(!matches("^1.3.0-beta.1", "1.4.0-beta.1"));
        assert!(!matches("<2", "2.0.0-beta"));
        assert!(matches("=1.0.0-rc.1", "1.0.0-rc.1"));
    }

    #[test]
    fn invalid_requirements() {
        for bad in [
            "",
            "x",
            "1.2.3.4",
            ">>1",
            "1.2-beta",
            "1.0.0+build",
            "01.2",
            "1,",
        ] {
            assert!(VersionReq::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(VersionReq::parse(" ^1.2 ").unwrap().to_string(), "^1.2");
    }
}
