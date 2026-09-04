//! `#tags` written inline in a note.
//!
//! Tags are not a field. They are typed in the middle of a sentence and stay in
//! the text, which is what "dynamic tags" has to mean in a capture box: a note
//! is one buffer, and anything that demands a separate field is something you
//! stop doing on the third day.
//!
//! The whole difficulty is that `#` is also ordinary punctuation. A URL
//! fragment, a markdown heading, an issue number and a CSS colour all contain
//! one, and none of them is a tag. The rules below are deliberately narrow:
//! false negatives cost a tag that has to be typed differently, false positives
//! put junk in the filter list forever.

/// Every `#tag` in `text`, in order and without duplicates.
///
/// Tags are compared and stored in lower case, so `#Kafka` and `#kafka` are one
/// tag. The note keeps whatever the user typed.
pub fn tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = text.as_bytes();

    for (i, _) in text.match_indices('#') {
        // Must start a word. This is what excludes a URL fragment and a
        // trailing `##`, and it costs nothing a person would want.
        if i > 0 && !bytes[i - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &text[i + 1..];
        let name: String = rest.chars().take_while(is_tag_char).collect();

        // A leading letter, not a digit: `#1`, `#2026` and `#404` are numbers
        // people write in prose, and a heading is `# ` with a space.
        if !name.chars().next().is_some_and(char::is_alphabetic) {
            continue;
        }
        // Trailing punctuation belongs to the sentence, not the tag: "…about
        // #kafka." must not produce a tag that only ever appears once.
        let name = name.trim_end_matches(['-', '_', '/']).to_lowercase();
        if name.is_empty() || out.contains(&name) {
            continue;
        }
        out.push(name);
    }
    out
}

/// Letters, digits, and the three separators people actually use in tags.
///
/// `/` earns its place by allowing `#work/kafka`, which is how a tag list stays
/// navigable once there are fifty of them.
fn is_tag_char(c: &char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tags_in_order_without_duplicates() {
        let body = "Kafka rebalance #kafka\n\nSee also #ops and #kafka again.";
        assert_eq!(tags(body), vec!["kafka", "ops"]);
    }

    #[test]
    fn tags_are_case_insensitive() {
        // One tag, however it was typed — otherwise the filter list grows a
        // near-duplicate every time a sentence starts with one.
        assert_eq!(tags("#Kafka and #kafka and #KAFKA"), vec!["kafka"]);
    }

    #[test]
    fn a_url_fragment_is_not_a_tag() {
        assert!(tags("https://example.com/page#section").is_empty());
        assert!(tags("see http://host/a#b and http://host/c#d").is_empty());
    }

    #[test]
    fn a_markdown_heading_is_not_a_tag() {
        // The space is what makes it a heading, and no tag can contain one.
        assert!(tags("# Heading\n\nbody").is_empty());
        assert!(tags("## Also a heading").is_empty());
    }

    #[test]
    fn a_number_is_not_a_tag() {
        // Issue numbers and years are written this way constantly.
        assert!(tags("fixes #1234 from #2026").is_empty());
        assert_eq!(tags("#v2 release"), vec!["v2"], "but a letter first is fine");
    }

    #[test]
    fn punctuation_after_a_tag_belongs_to_the_sentence() {
        assert_eq!(tags("about #kafka."), vec!["kafka"]);
        assert_eq!(tags("(#ops)"), vec![] as Vec<String>, "a tag must start a word");
        assert_eq!(tags("#kafka, #ops;"), vec!["kafka", "ops"]);
        assert_eq!(tags("#work- and #work_"), vec!["work"]);
    }

    #[test]
    fn tags_can_be_nested_with_a_slash() {
        assert_eq!(tags("#work/kafka and #work/flink"), vec!["work/kafka", "work/flink"]);
    }

    #[test]
    fn accented_tags_are_kept_whole() {
        // Polish is a first-class language here; a tag must not stop at ł.
        assert_eq!(tags("#zażółć and #łódź"), vec!["zażółć", "łódź"]);
    }

    #[test]
    fn a_bare_hash_is_nothing() {
        assert!(tags("#").is_empty());
        assert!(tags("# ").is_empty());
        assert!(tags("nothing here at all").is_empty());
    }
}
