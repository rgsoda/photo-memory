use chrono::{DateTime, Local, SecondsFormat, Timelike};

use crate::{slugify, Frontmatter};

/// One captured entry.
///
/// Notes are append-only by design: `id` is assigned once and never changes, and
/// nothing in the app mutates an existing note. `modified` exists only to record
/// edits made outside the app, in a text editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub created: DateTime<Local>,
    pub modified: DateTime<Local>,
    /// Everything after the frontmatter. The first non-empty line is the title.
    pub body: String,
    /// Frontmatter keys we do not model, preserved so a round-trip is lossless.
    extra: Frontmatter,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The body held nothing but whitespace, so there is no title to slug.
    Empty,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => write!(f, "note is empty"),
        }
    }
}

impl std::error::Error for ParseError {}

const MODELLED_KEYS: [&str; 3] = ["id", "created", "modified"];

impl Note {
    /// Build a new note from raw editor text, assigning a fresh id and timestamp.
    pub fn new(body: &str) -> Result<Note, ParseError> {
        let body = normalize(body);
        if body.is_empty() {
            return Err(ParseError::Empty);
        }
        // Truncated to whole seconds because that is all the on-disk format
        // records; without this a note differs from itself after a round-trip.
        let now = truncate(Local::now());
        Ok(Note {
            id: ulid::Ulid::new().to_string(),
            created: now,
            modified: now,
            body,
            extra: Frontmatter::new(),
        })
    }

    /// Read a note back from disk. Missing or unparsable timestamps fall back to
    /// `fallback` (the file's mtime) rather than failing: a hand-written note
    /// with no frontmatter at all is still a valid note.
    pub fn parse(doc: &str, fallback: DateTime<Local>) -> Result<Note, ParseError> {
        let (fm, rest) = Frontmatter::split(doc);
        let body = normalize(rest);
        if body.is_empty() {
            return Err(ParseError::Empty);
        }

        let created = fm.get("created").and_then(parse_time).unwrap_or(fallback);
        let mut extra = Frontmatter::new();
        for (k, v) in fm.iter() {
            if !MODELLED_KEYS.contains(&k) {
                extra.set(k, v);
            }
        }

        Ok(Note {
            id: fm.get("id").unwrap_or_default().to_string(),
            created,
            modified: fm.get("modified").and_then(parse_time).unwrap_or(created),
            body,
            extra,
        })
    }

    /// The first non-empty line, which doubles as the display name and slug source.
    pub fn title(&self) -> &str {
        self.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default()
    }

    /// The body with the title line removed.
    ///
    /// The title is displayed and indexed on its own, so repeating it in the
    /// body would make every search snippet open with text the reader is
    /// already looking at, and would blur the difference between a note *about*
    /// a thing and one that merely mentions it.
    pub fn content(&self) -> &str {
        match self.body.split_once('\n') {
            Some((_, rest)) => rest.trim_start_matches('\n'),
            None => "",
        }
    }

    /// `2026-09-04-1618-kafka-consumer-rebalance-storm.md`
    ///
    /// Purely for humans reading `ls`; the app never parses this back.
    pub fn filename(&self) -> String {
        format!("{}-{}.md", self.created.format("%Y-%m-%d-%H%M"), slugify(self.title()))
    }

    pub fn render(&self) -> String {
        let mut fm = Frontmatter::new();
        fm.set("id", &self.id);
        fm.set("created", fmt_time(self.created));
        fm.set("modified", fmt_time(self.modified));
        for (k, v) in self.extra.iter() {
            fm.set(k, v);
        }
        format!("{}{}\n", fm.render(), self.body)
    }
}

/// Trim trailing whitespace on every line and collapse the edges of the note.
/// Editors leave ragged whitespace behind and it would otherwise show up in
/// diffs on every capture.
fn normalize(body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim_end).collect();
    lines.join("\n").trim().to_string()
}

/// Drop sub-second precision, matching what `fmt_time` writes.
fn truncate(t: DateTime<Local>) -> DateTime<Local> {
    t.with_nanosecond(0).unwrap_or(t)
}

fn fmt_time(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn parse_time(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Local))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Local)
    }

    #[test]
    fn title_is_first_non_empty_line() {
        let n = Note::new("\n\n  Kafka rebalance  \n\nbody\n").unwrap();
        assert_eq!(n.title(), "Kafka rebalance");
    }

    #[test]
    fn content_drops_the_title_line() {
        let n = Note::new("Kafka rebalance\n\nConsumers were evicted.\nTwice.").unwrap();
        assert_eq!(n.content(), "Consumers were evicted.\nTwice.");
    }

    #[test]
    fn a_title_only_note_has_no_content() {
        assert_eq!(Note::new("Just a title").unwrap().content(), "");
    }

    #[test]
    fn empty_body_is_rejected() {
        assert_eq!(Note::new("   \n\n  ").unwrap_err(), ParseError::Empty);
    }

    #[test]
    fn filename_combines_timestamp_and_slug() {
        let mut n = Note::new("Kafka rebalance storm").unwrap();
        n.created = at("2026-09-04T16:18:42+02:00");
        assert_eq!(n.filename(), "2026-09-04-1618-kafka-rebalance-storm.md");
    }

    #[test]
    fn new_note_matches_second_precision_of_the_file_format() {
        assert_eq!(Note::new("Title").unwrap().created.nanosecond(), 0);
    }

    #[test]
    fn round_trips_through_render() {
        let n = Note::new("Title here\n\nSome body.\n").unwrap();
        let back = Note::parse(&n.render(), Local::now()).unwrap();
        assert_eq!(back, n);
    }

    #[test]
    fn preserves_unmodelled_frontmatter() {
        let doc = "---\nid: \"abc\"\ncreated: \"2026-09-04T16:18:42+02:00\"\nsupersedes: \"[[old-note]]\"\n---\nTitle\n";
        let n = Note::parse(doc, Local::now()).unwrap();
        assert!(n.render().contains("supersedes: \"[[old-note]]\""));
    }

    #[test]
    fn bare_markdown_file_still_parses() {
        let fallback = at("2026-01-01T00:00:00+01:00");
        let n = Note::parse("Just a note I typed in vim\n", fallback).unwrap();
        assert_eq!(n.title(), "Just a note I typed in vim");
        assert_eq!(n.created, fallback);
        assert!(n.id.is_empty());
    }

    #[test]
    fn modified_defaults_to_created() {
        let doc = "---\ncreated: \"2026-09-04T16:18:42+02:00\"\n---\nTitle\n";
        let n = Note::parse(doc, Local::now()).unwrap();
        assert_eq!(n.modified, n.created);
    }
}
