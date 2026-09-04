//! Derived search index over the notes directory.
//!
//! The files are the source of truth; this is a cache that can be deleted at any
//! time and rebuilt in seconds. Nothing is stored here that is not recoverable
//! from a note on disk.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use photomem_core::Note;
use rusqlite::{params, Connection};

mod query;
pub use query::to_fts_query;

/// `remove_diacritics 2` folds Polish accents, so "zazolc" finds "zażółć" —
/// which matters for typing a search fast in the middle of something else.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notes (
    path    TEXT PRIMARY KEY,
    id      TEXT NOT NULL,
    title   TEXT NOT NULL,
    created TEXT NOT NULL,
    mtime   INTEGER NOT NULL,
    size    INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path UNINDEXED,
    title,
    body,
    tokenize='unicode61 remove_diacritics 2'
);
";

/// How much a fresh note may improve its own relevance score. Deliberately
/// small: recency breaks ties between comparable matches, it does not decide
/// them.
const RECENCY_WEIGHT: f64 = 0.25;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub title: String,
    pub created: DateTime<Local>,
    /// Matching text with the hit marked by `»…«`, or the opening of the note
    /// when there is no query to match.
    pub snippet: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

impl SyncStats {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

pub struct Index {
    db: Connection,
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Index::init(db)
    }

    pub fn in_memory() -> Result<Index> {
        Index::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Index> {
        // Notes are written rarely and read on every keystroke; WAL keeps a
        // save from blocking the search that is running as you type.
        db.pragma_update(None, "journal_mode", "WAL").ok();
        db.execute_batch(SCHEMA)?;
        Ok(Index { db })
    }

    /// Bring the index in line with the notes directory.
    ///
    /// This walks the directory rather than watching it. A scan of a few
    /// thousand files costs milliseconds, runs only when the window opens, and
    /// cannot miss an event or fall out of step the way a watcher can after a
    /// `git pull` rewrites files underneath it.
    pub fn sync(&mut self, notes_dir: &Path) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let mut seen: Vec<String> = Vec::new();

        let entries = match std::fs::read_dir(notes_dir) {
            Ok(e) => e,
            // A vault with nothing captured yet is not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(stats),
            Err(e) => return Err(e).context(format!("reading {}", notes_dir.display())),
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let key = path.to_string_lossy().into_owned();
            seen.push(key.clone());

            let meta = entry.metadata()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default();
            let size = meta.len() as i64;

            let known: Option<(i64, i64)> = self
                .db
                .query_row("SELECT mtime, size FROM notes WHERE path = ?1", [&key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .ok();

            match known {
                Some((m, s)) if (m, s) == (mtime, size) => continue,
                Some(_) => stats.updated += 1,
                None => stats.added += 1,
            }

            let text = std::fs::read_to_string(&path)?;
            let fallback = Local.timestamp_opt(mtime, 0).single().unwrap_or_else(Local::now);
            match Note::parse(&text, fallback) {
                Ok(note) => self.put(&key, &note, mtime, size)?,
                // An empty or unreadable file is skipped, not fatal: the vault
                // is a directory a human can drop anything into.
                Err(_) => {
                    seen.pop();
                    if stats.added > 0 {
                        stats.added -= 1;
                    }
                }
            }
        }

        stats.removed = self.remove_missing(&seen)?;
        Ok(stats)
    }

    fn put(&self, path: &str, note: &Note, mtime: i64, size: i64) -> Result<()> {
        self.db.execute("DELETE FROM notes_fts WHERE path = ?1", [path])?;
        self.db.execute(
            "INSERT OR REPLACE INTO notes (path, id, title, created, mtime, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![path, note.id, note.title(), note.created.to_rfc3339(), mtime, size],
        )?;
        self.db.execute(
            "INSERT INTO notes_fts (path, title, body) VALUES (?1, ?2, ?3)",
            params![path, note.title(), note.content()],
        )?;
        Ok(())
    }

    fn remove_missing(&self, seen: &[String]) -> Result<usize> {
        let existing: Vec<String> = self
            .db
            .prepare("SELECT path FROM notes")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;

        let mut removed = 0;
        for path in existing.iter().filter(|p| !seen.contains(p)) {
            self.db.execute("DELETE FROM notes WHERE path = ?1", [path])?;
            self.db.execute("DELETE FROM notes_fts WHERE path = ?1", [path])?;
            removed += 1;
        }
        Ok(removed)
    }

    pub fn len(&self) -> usize {
        self.db
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Search, or list the most recent notes when the query is empty.
    ///
    /// Opening the picker with nothing typed should show something useful, so an
    /// empty query is "what did I capture lately" rather than no results.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        match to_fts_query(query) {
            Some(fts) => self.matching(&fts, limit),
            None => self.recent(limit),
        }
    }

    fn recent(&self, limit: usize) -> Result<Vec<Hit>> {
        let mut stmt = self.db.prepare(
            "SELECT n.path, n.title, n.created, substr(f.body, 1, 160)
             FROM notes n JOIN notes_fts f ON f.path = n.path
             ORDER BY n.created DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], row_to_hit)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// bm25 takes one weight per column, *including* the unindexed `path`, so
    /// the weights below read path, title, body. A note titled after a thing is
    /// worth more than one that mentions it in passing.
    fn matching(&self, fts: &str, limit: usize) -> Result<Vec<Hit>> {
        let mut stmt = self.db.prepare(
            "SELECT f.path, n.title, n.created,
                    snippet(notes_fts, 2, '»', '«', '…', 12),
                    bm25(notes_fts, 0.0, 4.0, 1.0)
             FROM notes_fts f JOIN notes n ON n.path = f.path
             WHERE notes_fts MATCH ?1
             ORDER BY bm25(notes_fts, 0.0, 4.0, 1.0)
             LIMIT ?2",
        )?;

        // Over-fetch so the recency boost has something to reorder.
        let rows = stmt.query_map(params![fts, (limit * 4) as i64], |r| {
            Ok((row_to_hit(r)?, r.get::<_, f64>(4)?))
        })?;

        let mut scored: Vec<(Hit, f64)> = rows.collect::<std::result::Result<_, _>>()?;
        let now = Local::now();
        for (hit, score) in &mut scored {
            // bm25 is negative, better the lower, and its magnitude depends on
            // the size of the corpus — in a young vault the scores are around
            // 1e-6, in a large one they are units. So recency scales the score
            // rather than being subtracted from it: a fixed bonus would mean
            // nothing at one size and drown out relevance at another.
            let age_days = (now - hit.created).num_days().max(0) as f64;
            let recency = (-age_days / 30.0).exp(); // 1.0 today, ~0 after months
            *score *= 1.0 + RECENCY_WEIGHT * recency;
        }
        scored.sort_by(|a, b| a.1.total_cmp(&b.1));
        scored.truncate(limit);

        Ok(scored.into_iter().map(|(hit, _)| hit).collect())
    }
}

fn row_to_hit(r: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    let created: String = r.get(2)?;
    Ok(Hit {
        path: PathBuf::from(r.get::<_, String>(0)?),
        title: r.get(1)?,
        created: DateTime::parse_from_rfc3339(&created)
            .map(|t| t.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now()),
        snippet: r.get::<_, String>(3)?.replace('\n', " ").trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(notes: &[&str]) -> (PathBuf, Index) {
        let dir = std::env::temp_dir().join(format!(
            "photomem-idx-{}-{}",
            std::process::id(),
            notes.len() * 100 + notes.first().map_or(0, |n| n.len())
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, body) in notes.iter().enumerate() {
            let note = Note::new(body).unwrap();
            std::fs::write(dir.join(format!("{i}-{}", note.filename())), note.render()).unwrap();
        }
        let mut index = Index::in_memory().unwrap();
        index.sync(&dir).unwrap();
        (dir, index)
    }

    #[test]
    fn finds_a_note_by_a_word_in_its_body() {
        let (_d, ix) = vault_with(&["Kafka rebalance\n\nconsumers were evicted", "Tiling window gaps"]);
        let hits = ix.search("evicted", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Kafka rebalance");
        assert!(hits[0].snippet.contains('»'), "expected a marked snippet: {}", hits[0].snippet);
    }

    #[test]
    fn matches_prefixes_so_results_appear_while_typing() {
        let (_d, ix) = vault_with(&["Kafka rebalance storm\n\nbody"]);
        for typed in ["k", "kaf", "kafka reb"] {
            assert_eq!(ix.search(typed, 10).unwrap().len(), 1, "typing {typed:?} found nothing");
        }
    }

    #[test]
    fn folds_polish_diacritics_so_accents_can_be_skipped_while_typing() {
        let (_d, ix) = vault_with(&["Zażółć gęślą jaźń\n\nnotatka"]);
        assert_eq!(ix.search("gesla", 10).unwrap().len(), 1);
        assert_eq!(ix.search("jazn", 10).unwrap().len(), 1);
        assert_eq!(ix.search("gęślą", 10).unwrap().len(), 1);
    }

    #[test]
    fn stroked_l_is_the_one_letter_that_does_not_fold() {
        // `ł` is a distinct letter with no Unicode decomposition, so unlike ą, ć,
        // ę, ń, ó, ś, ż and ź it survives `remove_diacritics`. Typing the word
        // with its ł works; typing a plain `l` in its place does not. Pinned here
        // so the day it starts mattering, the fix is a deliberate one.
        let (_d, ix) = vault_with(&["Zażółć gęślą jaźń\n\nnotatka"]);
        assert_eq!(ix.search("zazołc", 10).unwrap().len(), 1);
        assert!(ix.search("zazolc", 10).unwrap().is_empty());
    }

    #[test]
    fn an_empty_query_lists_recent_notes_newest_first() {
        let (_d, ix) = vault_with(&["First note", "Second note", "Third note"]);
        let hits = ix.search("   ", 10).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits[0].created >= hits[1].created);
    }

    #[test]
    fn title_matches_outrank_body_mentions() {
        let (_d, ix) = vault_with(&[
            "Something unrelated\n\nthis mentions kafka once in passing",
            "Kafka rebalance\n\nnotes about the incident",
        ]);
        let hits = ix.search("kafka", 10).unwrap();
        assert_eq!(hits[0].title, "Kafka rebalance");
    }

    #[test]
    fn a_title_match_wins_even_when_an_older_note_is_the_one_titled() {
        // The recency boost must not be strong enough to promote a passing
        // mention over the note that is actually about the thing.
        let (dir, mut ix) = vault_with(&[]);
        std::fs::write(
            dir.join("2026-03-11-0902-kafka-poll-tuning.md"),
            "---\ncreated: \"2026-03-11T09:02:00+01:00\"\n---\nKafka poll tuning\nBumped max.poll.records to 200.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("2026-06-02-1130-flink-checkpoint-timeouts.md"),
            "---\ncreated: \"2026-06-02T11:30:00+02:00\"\n---\nFlink checkpoint timeouts\nSame shape as the Kafka rebalance problem.\n",
        )
        .unwrap();
        ix.sync(&dir).unwrap();

        let hits = ix.search("kafka", 10).unwrap();
        assert_eq!(hits[0].title, "Kafka poll tuning");
    }

    #[test]
    fn recency_breaks_ties_between_equally_good_matches() {
        let (dir, mut ix) = vault_with(&[]);
        for (day, id) in [("2026-01-05", "older"), ("2026-08-05", "newer")] {
            std::fs::write(
                dir.join(format!("{day}-1000-{id}.md")),
                format!("---\ncreated: \"{day}T10:00:00+01:00\"\n---\nDeploy notes {id}\nthe rollout went fine\n"),
            )
            .unwrap();
        }
        ix.sync(&dir).unwrap();

        let hits = ix.search("rollout", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].title.contains("newer"), "got {:?} first", hits[0].title);
    }

    #[test]
    fn snippets_do_not_repeat_the_title() {
        let (_d, ix) = vault_with(&["Kafka rebalance\n\nconsumers were evicted"]);
        let snippet = &ix.search("evicted", 10).unwrap()[0].snippet;
        assert!(!snippet.contains("Kafka rebalance"), "snippet repeated the title: {snippet}");
    }

    #[test]
    fn sync_tracks_additions_edits_and_deletions() {
        let (dir, mut ix) = vault_with(&["First note", "Second note"]);
        assert_eq!(ix.len(), 2);

        // Re-syncing an unchanged directory must do nothing at all.
        assert!(!ix.sync(&dir).unwrap().changed());

        let extra = dir.join("2026-01-01-0000-added-by-hand.md");
        std::fs::write(&extra, "Added by hand\n\nfrom another editor").unwrap();
        assert_eq!(ix.sync(&dir).unwrap().added, 1);
        assert_eq!(ix.search("another editor", 10).unwrap().len(), 1);

        std::fs::write(&extra, "Added by hand\n\nrewritten completely with plums").unwrap();
        assert_eq!(ix.sync(&dir).unwrap().updated, 1);
        assert_eq!(ix.search("plums", 10).unwrap().len(), 1);
        assert!(ix.search("another editor", 10).unwrap().is_empty(), "stale text still indexed");

        std::fs::remove_file(&extra).unwrap();
        assert_eq!(ix.sync(&dir).unwrap().removed, 1);
        assert!(ix.search("plums", 10).unwrap().is_empty());
        assert_eq!(ix.len(), 2);
    }

    #[test]
    fn skips_files_that_are_not_notes() {
        let (dir, mut ix) = vault_with(&["Real note"]);
        std::fs::write(dir.join("empty.md"), "   \n\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not markdown").unwrap();

        assert!(!ix.sync(&dir).unwrap().changed());
        assert_eq!(ix.len(), 1);
    }

    #[test]
    fn a_missing_notes_directory_is_not_an_error() {
        let mut ix = Index::in_memory().unwrap();
        assert!(ix.sync(Path::new("/nonexistent/photomem/notes")).is_ok());
        assert!(ix.is_empty());
    }
}
