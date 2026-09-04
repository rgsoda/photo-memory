/// Turn a title into a filename-safe slug.
///
/// Lowercased ASCII, non-alphanumerics collapsed to single dashes, trimmed to
/// whole words within `MAX_LEN`. Non-ASCII letters are kept only if they are
/// alphanumeric in Unicode terms, so Polish titles slug sensibly.
const MAX_LEN: usize = 60;

pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len().min(MAX_LEN));
    let mut pending_dash = false;

    for ch in title.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_dash = true;
        }
    }

    if out.chars().count() > MAX_LEN {
        let cut: String = out.chars().take(MAX_LEN).collect();
        // Prefer cutting at a word boundary, but not if that loses most of it.
        out = match cut.rfind('-') {
            Some(i) if i > MAX_LEN / 2 => cut[..i].to_string(),
            _ => cut,
        };
    }

    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        assert_eq!(slugify("Kafka consumer rebalance storm"), "kafka-consumer-rebalance-storm");
    }

    #[test]
    fn collapses_and_trims_punctuation() {
        assert_eq!(slugify("  Hello --- World!! "), "hello-world");
        assert_eq!(slugify("a/b:c"), "a-b-c");
    }

    #[test]
    fn keeps_unicode_letters() {
        assert_eq!(slugify("Zażółć gęślą jaźń"), "zażółć-gęślą-jaźń");
    }

    #[test]
    fn empty_title_is_untitled() {
        assert_eq!(slugify("   ...   "), "untitled");
        assert_eq!(slugify(""), "untitled");
    }

    #[test]
    fn truncates_at_word_boundary() {
        let s = slugify(&"word ".repeat(40));
        assert!(s.len() <= MAX_LEN, "{s}");
        assert!(!s.ends_with('-'), "{s}");
    }
}
