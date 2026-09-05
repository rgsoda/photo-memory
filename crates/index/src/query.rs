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

/// A search as typed: `#tag`s narrow it, everything else is text to match.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased and without the `#`, which is how the index stores them.
    pub tags: Vec<String>,
    /// `None` when only tags were typed, which callers read as "everything
    /// carrying these" rather than "no results".
    pub text: Option<String>,
}

/// Split what was typed into tag filters and search text.
pub fn parse(input: &str) -> Query {
    let mut tags: Vec<String> = Vec::new();
    let mut words: Vec<&str> = Vec::new();

    for token in input.split_whitespace() {
        match token.strip_prefix('#') {
            // The same rule that put them in the index: a tag starts with a
            // letter. `#404` typed into a search is a number being looked for,
            // not a filter that would match nothing.
            Some(name) if name.starts_with(|c: char| c.is_alphabetic()) => {
                let name = name.trim_end_matches(['-', '_', '/']).to_lowercase();
                if !tags.contains(&name) {
                    tags.push(name);
                }
            }
            _ => words.push(token),
        }
    }

    Query { tags, text: to_fts_query(&words.join(" ")) }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    #[test]
    fn separates_tags_from_text() {
        let q = parse("#work kafka rebalance");
        assert_eq!(q.tags, ["work"]);
        assert_eq!(q.text.unwrap(), "\"kafka\"* \"rebalance\"*");
    }

    #[test]
    fn tags_alone_leave_nothing_to_match() {
        let q = parse("#work #debugging");
        assert_eq!(q.tags, ["work", "debugging"]);
        assert_eq!(q.text, None);
    }

    #[test]
    fn a_number_is_not_a_tag() {
        // `#404` is something written in prose, and filtering on it would
        // silently return nothing rather than searching for it.
        let q = parse("#404");
        assert!(q.tags.is_empty());
        assert_eq!(q.text.unwrap(), "\"404\"*");
    }

    #[test]
    fn trailing_punctuation_and_case_match_how_tags_are_stored() {
        assert_eq!(parse("#Kafka/").tags, ["kafka"]);
        assert_eq!(parse("#work #WORK").tags, ["work"]);
    }
}
