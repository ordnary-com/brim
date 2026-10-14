//! "Did you mean ...?" suggestions.

/// Returns the candidate closest to `name`, if it is close enough to be a
/// plausible typo.
pub fn similar<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let max_distance = name.chars().count().div_ceil(3).max(1);
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (levenshtein(name, c), c))
        .filter(|(d, _)| *d <= max_distance)
        .min_by_key(|(d, c)| (*d, *c))
        .map(|(_, c)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != *cb);
            cur.push(substitute.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Suggests a Brim type for type names common in other languages.
pub fn type_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "int" | "i8" | "i16" | "i32" | "i128" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize"
        | "usize" | "long" | "integer" | "Int" | "number" => "i64",
        "str" | "String" | "text" | "char" => "string",
        "boolean" | "Bool" | "Boolean" => "bool",
        "unit" | "()" | "None" | "nil" | "null" => "void",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }

    #[test]
    fn suggestions() {
        assert_eq!(similar("nmae", ["name", "other"]), Some("name"));
        assert_eq!(similar("prnt", ["print"]), Some("print"));
        assert_eq!(similar("x", ["completely_different"]), None);
        assert_eq!(type_name("int"), Some("i64"));
        assert_eq!(type_name("i64"), None);
    }
}
