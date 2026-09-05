//! Turning what someone typed into an FTS5 query.
//!
//! Search text is typed fast and mid-thought, so it will contain quotes,
//! hyphens, colons and stray punctuation — all of which are operators in FTS5
//! and would otherwise produce a syntax error instead of results. Every term is
//! therefore quoted and given a prefix `*`.

use chrono::{Days, Local, Months, NaiveDate};

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

/// A search as typed: `#tag`s and `since:`/`before:` narrow it, the rest is
/// text to match.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased and without the `#`, which is how the index stores them.
    pub tags: Vec<String>,
    /// Inclusive lower bound on the note's own calendar date.
    pub since: Option<NaiveDate>,
    /// Exclusive upper bound, so `before:2026-03` means "up to March".
    pub before: Option<NaiveDate>,
    /// `None` when only filters were typed, which callers read as "everything
    /// matching these" rather than "no results".
    pub text: Option<String>,
}

/// Split what was typed into filters and search text.
pub fn parse(input: &str) -> Query {
    parse_on(input, Local::now().date_naive())
}

/// `today` is a parameter so the tests are not written against the calendar.
fn parse_on(input: &str, today: NaiveDate) -> Query {
    let mut tags: Vec<String> = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    let mut since = None;
    let mut before = None;

    for token in input.split_whitespace() {
        // A bound that cannot be understood falls through to the text, on the
        // same reasoning as `#404`: silently returning nothing is worse than
        // searching for what was actually typed.
        if let Some(d) = token.strip_prefix("since:").and_then(|v| parse_date(v, today)) {
            since = Some(d);
            continue;
        }
        if let Some(d) = token.strip_prefix("before:").and_then(|v| parse_date(v, today)) {
            before = Some(d);
            continue;
        }
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

    Query { tags, since, before, text: to_fts_query(&words.join(" ")) }
}

/// A date written into a search, resolved against `today`.
///
/// Absolute at three precisions, because "March" is as natural a thing to type
/// as a full date, and a month or a year names its first day. Relative forms
/// exist because the question is usually "the last week or so", asked in the
/// middle of typing something else.
fn parse_date(value: &str, today: NaiveDate) -> Option<NaiveDate> {
    match value {
        "today" => return Some(today),
        "yesterday" => return today.pred_opt(),
        _ => {}
    }

    let count = |suffix: char| value.strip_suffix(suffix).and_then(|n| n.parse::<u32>().ok());
    if let Some(n) = count('d') {
        return today.checked_sub_days(Days::new(n as u64));
    }
    if let Some(n) = count('w') {
        return today.checked_sub_days(Days::new(n as u64 * 7));
    }
    // Months, not thirty-day blocks: "three months ago" means the same day of
    // the month, which is what anyone counting back on a calendar would land on.
    if let Some(n) = count('m') {
        return today.checked_sub_months(Months::new(n));
    }

    match value.split('-').collect::<Vec<_>>().as_slice() {
        [y] => NaiveDate::from_ymd_opt(y.parse().ok()?, 1, 1),
        [y, m] => NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, 1),
        [y, m, d] => NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?),
        _ => None,
    }
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

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn absolute_dates_at_three_precisions() {
        let t = day(2026, 9, 5);
        assert_eq!(parse_on("since:2026-03-11", t).since, Some(day(2026, 3, 11)));
        // A month and a year name their first day, so `before:2026-03` is
        // "up to March" rather than "up to the end of March".
        assert_eq!(parse_on("since:2026-03", t).since, Some(day(2026, 3, 1)));
        assert_eq!(parse_on("before:2026", t).before, Some(day(2026, 1, 1)));
    }

    #[test]
    fn relative_dates_are_resolved_against_today() {
        let t = day(2026, 9, 5);
        assert_eq!(parse_on("since:today", t).since, Some(t));
        assert_eq!(parse_on("since:yesterday", t).since, Some(day(2026, 9, 4)));
        assert_eq!(parse_on("since:7d", t).since, Some(day(2026, 8, 29)));
        assert_eq!(parse_on("since:2w", t).since, Some(day(2026, 8, 22)));
        // Calendar months, so the day of the month is kept.
        assert_eq!(parse_on("since:3m", t).since, Some(day(2026, 6, 5)));
    }

    #[test]
    fn a_bound_is_a_filter_and_not_also_search_text() {
        let q = parse_on("since:2026-01 #work kafka", day(2026, 9, 5));
        assert_eq!(q.since, Some(day(2026, 1, 1)));
        assert_eq!(q.tags, ["work"]);
        assert_eq!(q.text.unwrap(), "\"kafka\"*");
    }

    #[test]
    fn a_bound_that_makes_no_sense_is_searched_for_rather_than_dropped() {
        // Same reasoning as `#404`: a filter nobody can parse must not quietly
        // return nothing.
        let q = parse_on("since:soon", day(2026, 9, 5));
        assert_eq!(q.since, None);
        assert!(q.text.is_some());
    }
}
