//! Turning what someone typed into an FTS5 query.
//!
//! Search text is typed fast and mid-thought, so it will contain quotes,
//! hyphens, colons and stray punctuation — all of which are operators in FTS5
//! and would otherwise produce a syntax error instead of results. Every term is
//! therefore quoted and given a prefix `*`.

/// `None` when there is nothing to search for, which callers read as "show
/// recent notes" rather than "no results".
pub fn to_fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|term| term.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect::<String>())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();

    if terms.is_empty() {
        return None;
    }
    // Space between terms is an implicit AND: more words must narrow, not widen.
    Some(terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_prefixes_each_term() {
        assert_eq!(to_fts_query("kafka reb").unwrap(), "\"kafka\"* \"reb\"*");
    }

    #[test]
    fn nothing_to_search_for() {
        assert_eq!(to_fts_query(""), None);
        assert_eq!(to_fts_query("   "), None);
        // Punctuation alone carries no terms once stripped.
        assert_eq!(to_fts_query("-- \"\" ()"), None);
    }

    #[test]
    fn strips_fts_operators_rather_than_failing_on_them() {
        // Each of these is a syntax error if passed through to FTS5.
        for input in ["\"unclosed", "a AND OR", "foo:bar", "x - y", "NEAR(a b)", "*"] {
            let q = to_fts_query(input);
            assert!(q.as_deref().is_none_or(|q| !q.contains(['(', ')', ':', '-'])), "{input:?} -> {q:?}");
        }
    }

    #[test]
    fn keeps_unicode_letters() {
        assert_eq!(to_fts_query("zażółć").unwrap(), "\"zażółć\"*");
    }
}
