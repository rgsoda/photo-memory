//! A deliberately small YAML subset: a flat, ordered map of string values.
//!
//! We hand-roll rather than pull in a YAML library because the format is fixed
//! and tiny, and because the alternative buys ambiguity we do not want. Every
//! value is written quoted: `2026-09-04T16:18:42+02:00` would otherwise parse
//! as a YAML timestamp, and `[[a-note]]` as a nested sequence.
//!
//! Unknown keys are preserved in order, so a note written by a later version of
//! the app survives a round-trip through an earlier one.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    fields: Vec<(String, String)>,
}

impl Frontmatter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Set `key`, keeping its original position if it is already present.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.fields.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.fields.push((key.to_string(), value)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn render(&self) -> String {
        let mut out = String::from("---\n");
        for (k, v) in &self.fields {
            out.push_str(k);
            out.push_str(": \"");
            out.push_str(&escape(v));
            out.push_str("\"\n");
        }
        out.push_str("---\n");
        out
    }

    /// Split a document into frontmatter and the body that follows it.
    ///
    /// A document without a leading `---` is all body: notes edited by hand in
    /// another editor should never be rejected.
    pub fn split(doc: &str) -> (Frontmatter, &str) {
        let Some(rest) = doc.strip_prefix("---\n") else {
            return (Frontmatter::new(), doc);
        };
        let Some(end) = find_terminator(rest) else {
            return (Frontmatter::new(), doc);
        };

        let mut fm = Frontmatter::new();
        for line in rest[..end].lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                fm.set(k.trim(), unquote(strip_comment(v.trim())));
            }
        }

        let after = &rest[end..];
        let body = after
            .strip_prefix("---\n")
            .or_else(|| after.strip_prefix("---"))
            .unwrap_or(after);
        (fm, body)
    }
}

/// Byte offset of the closing `---` line within `rest`.
fn find_terminator(rest: &str) -> Option<usize> {
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Drop a trailing ` # comment`, but only outside quotes.
fn strip_comment(v: &str) -> &str {
    if v.starts_with('"') {
        return v;
    }
    match v.split_once(" #") {
        Some((before, _)) => before.trim_end(),
        None => v,
    }
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

fn unquote(v: &str) -> String {
    let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return v.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push(chars.next().unwrap_or('\\')),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut fm = Frontmatter::new();
        fm.set("id", "01JBQX");
        fm.set("created", "2026-09-04T16:18:42+02:00");
        fm.set("supersedes", "[[2026-03-11-0902-kafka]]");

        let doc = format!("{}body text\n", fm.render());
        let (parsed, body) = Frontmatter::split(&doc);

        assert_eq!(parsed, fm);
        assert_eq!(body, "body text\n");
        assert_eq!(parsed.get("supersedes"), Some("[[2026-03-11-0902-kafka]]"));
    }

    #[test]
    fn preserves_unknown_keys_and_order() {
        let doc = "---\nid: \"x\"\nfuture_field: \"y\"\n---\nhi\n";
        let (fm, _) = Frontmatter::split(doc);
        assert_eq!(fm.iter().collect::<Vec<_>>(), vec![("id", "x"), ("future_field", "y")]);
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let mut fm = Frontmatter::new();
        fm.set("title", r#"a "quoted" \ thing"#);
        let (parsed, _) = Frontmatter::split(&fm.render());
        assert_eq!(parsed.get("title"), Some(r#"a "quoted" \ thing"#));
    }

    #[test]
    fn document_without_frontmatter_is_all_body() {
        let (fm, body) = Frontmatter::split("just a note\n");
        assert_eq!(fm, Frontmatter::new());
        assert_eq!(body, "just a note\n");
    }

    #[test]
    fn unterminated_frontmatter_is_all_body() {
        let doc = "---\nid: \"x\"\nnever closed\n";
        let (fm, body) = Frontmatter::split(doc);
        assert_eq!(fm, Frontmatter::new());
        assert_eq!(body, doc);
    }

    #[test]
    fn tolerates_unquoted_values_and_comments() {
        let (fm, _) = Frontmatter::split("---\nid: plain\nlang: eng # a comment\n---\n");
        assert_eq!(fm.get("id"), Some("plain"));
        assert_eq!(fm.get("lang"), Some("eng"));
    }
}
