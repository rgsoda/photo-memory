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
    name    TEXT NOT NULL,
    title   TEXT NOT NULL,
    created TEXT NOT NULL,
    mtime   INTEGER NOT NULL,
    size    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS notes_name ON notes (name);

CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path UNINDEXED,
    title,
    body,
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS links (
    from_path TEXT NOT NULL,
    target    TEXT NOT NULL,
    kind      TEXT NOT NULL,
    PRIMARY KEY (from_path, target, kind)
);

CREATE INDEX IF NOT EXISTS links_target ON links (target);

CREATE TABLE IF NOT EXISTS attachments (
    from_path TEXT NOT NULL,
    name      TEXT NOT NULL,
    position  INTEGER NOT NULL,
    PRIMARY KEY (from_path, name)
);

CREATE INDEX IF NOT EXISTS attachments_name ON attachments (name);
";

/// Bumped whenever the tables change shape.
///
/// The index is derived state, so a schema change is handled by throwing it
/// away rather than migrating. Without this, notes already indexed keep their
/// recorded mtime and size, `sync` skips them as unchanged, and they would
/// never acquire the links a newly added table wants — leaving backlinks
/// silently empty for every note captured before the upgrade.
const SCHEMA_VERSION: i64 = 2;

/// An ordinary `[[name]]` mention.
const KIND_LINK: &str = "link";
/// A `supersedes:` declaration, which the viewer raises as a banner.
const KIND_SUPERSEDES: &str = "supersedes";

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

/// Another note pointing at this one: a backlink, or the note that supersedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub path: PathBuf,
    /// This note's `[[link]]` target: its filename without the `.md`.
    pub name: String,
    pub title: String,
    pub created: DateTime<Local>,
}

/// One picture on the thumbnail wall, and the note it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// Attachment filename, as the note embeds it.
    pub name: String,
    pub note: PathBuf,
    /// The note's `[[link]]` target.
    pub note_name: String,
    pub title: String,
    pub created: DateTime<Local>,
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

        let found: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        if found != SCHEMA_VERSION {
            db.execute_batch(
                "DROP TABLE IF EXISTS attachments;
                 DROP TABLE IF EXISTS links;
                 DROP TABLE IF EXISTS notes_fts;
                 DROP TABLE IF EXISTS notes;",
            )?;
        }
        db.execute_batch(SCHEMA)?;
        db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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
        // Links are rewritten wholesale rather than diffed: a note edited to drop
        // a reference must not leave the old backlink behind.
        self.db.execute("DELETE FROM links WHERE from_path = ?1", [path])?;
        self.db.execute("DELETE FROM attachments WHERE from_path = ?1", [path])?;
        self.db.execute(
            "INSERT OR REPLACE INTO notes (path, id, name, title, created, mtime, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                path,
                note.id,
                link_name(path),
                note.title(),
                note.created.to_rfc3339(),
                mtime,
                size
            ],
        )?;
        self.db.execute(
            "INSERT INTO notes_fts (path, title, body) VALUES (?1, ?2, ?3)",
            params![path, note.title(), note.content()],
        )?;

        let mut stmt = self
            .db
            .prepare("INSERT OR IGNORE INTO links (from_path, target, kind) VALUES (?1, ?2, ?3)")?;
        for target in note.links() {
            stmt.execute(params![path, target, KIND_LINK])?;
        }
        for target in note.supersedes() {
            stmt.execute(params![path, target, KIND_SUPERSEDES])?;
        }

        let mut stmt = self.db.prepare(
            "INSERT OR IGNORE INTO attachments (from_path, name, position) VALUES (?1, ?2, ?3)",
        )?;
        for (i, name) in note.embeds().iter().enumerate() {
            stmt.execute(params![path, name, i as i64])?;
        }
        Ok(())
    }

    /// Every captured image, newest note first.
    ///
    /// One row per picture, not per embed: the same screenshot pasted into two
    /// notes is one thing you would recognise on the wall, and it belongs to
    /// the note you wrote most recently about it. Within a note the pictures
    /// keep the order they were pasted in.
    pub fn wall(&self, limit: usize) -> Result<Vec<Shot>> {
        let mut stmt = self.db.prepare(
            "SELECT a.name, n.path, n.name, n.title, n.created
             FROM attachments a JOIN notes n ON n.path = a.from_path
             WHERE n.created = (SELECT MAX(n2.created)
                                FROM attachments a2 JOIN notes n2 ON n2.path = a2.from_path
                                WHERE a2.name = a.name)
             GROUP BY a.name
             ORDER BY n.created DESC, a.position ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            let created: String = r.get(4)?;
            Ok(Shot {
                name: r.get(0)?,
                note: PathBuf::from(r.get::<_, String>(1)?),
                note_name: r.get(2)?,
                title: r.get(3)?,
                created: DateTime::parse_from_rfc3339(&created)
                    .map(|t| t.with_timezone(&Local))
                    .unwrap_or_else(|_| Local::now()),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Notes that reference `name`, newest first.
    ///
    /// This is the `links` table read backwards, which is all a backlink is —
    /// hence the index on `target` rather than only on `from_path`.
    pub fn backlinks(&self, name: &str) -> Result<Vec<Ref>> {
        let mut stmt = self.db.prepare(
            "SELECT DISTINCT n.path, n.name, n.title, n.created
             FROM links l JOIN notes n ON n.path = l.from_path
             WHERE l.target = ?1 AND n.name <> ?1
             ORDER BY n.created DESC",
        )?;
        let rows = stmt.query_map([name], row_to_ref)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The newest note declaring that it supersedes `name`, if any.
    ///
    /// Newest wins when several do: the banner says what is currently believed,
    /// and the rest stay reachable in the backlink list rather than vanishing.
    pub fn superseded_by(&self, name: &str) -> Result<Option<Ref>> {
        let mut stmt = self.db.prepare(
            "SELECT n.path, n.name, n.title, n.created
             FROM links l JOIN notes n ON n.path = l.from_path
             WHERE l.target = ?1 AND l.kind = ?2 AND n.name <> ?1
             ORDER BY n.created DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![name, KIND_SUPERSEDES], row_to_ref)?;
        rows.next().transpose().map_err(Into::into)
    }

    /// Every note, newest first.
    ///
    /// Unlike `recent`, this is the whole vault rather than a search result, and
    /// it carries no snippet: the timeline is read by date and title, and a
    /// snippet per row would turn a scannable list into a wall of prose.
    pub fn timeline(&self, limit: usize) -> Result<Vec<Ref>> {
        let mut stmt = self.db.prepare(
            "SELECT path, name, title, created FROM notes ORDER BY created DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], row_to_ref)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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
            self.db.execute("DELETE FROM links WHERE from_path = ?1", [path])?;
            self.db.execute("DELETE FROM attachments WHERE from_path = ?1", [path])?;
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

/// The `[[link]]` target for a note: its filename without the `.md`.
fn link_name(path: &str) -> String {
    Path::new(path).file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

fn row_to_ref(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ref> {
    let created: String = r.get(3)?;
    Ok(Ref {
        path: PathBuf::from(r.get::<_, String>(0)?),
        name: r.get(1)?,
        title: r.get(2)?,
        created: DateTime::parse_from_rfc3339(&created)
            .map(|t| t.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now()),
    })
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

    /// Named rather than derived from the notes, so that two tests building
    /// similar vaults cannot land in one directory and clear it under each
    /// other while the suite runs in parallel.
    fn vault_with(name: &str, notes: &[&str]) -> (PathBuf, Index) {
        let dir = std::env::temp_dir()
            .join(format!("photomem-idx-{}-{name}", std::process::id()));
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
        let (_d, ix) = vault_with("by-word", &["Kafka rebalance\n\nconsumers were evicted", "Tiling window gaps"]);
        let hits = ix.search("evicted", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Kafka rebalance");
        assert!(hits[0].snippet.contains('»'), "expected a marked snippet: {}", hits[0].snippet);
    }

    #[test]
    fn matches_prefixes_so_results_appear_while_typing() {
        let (_d, ix) = vault_with("prefixes", &["Kafka rebalance storm\n\nbody"]);
        for typed in ["k", "kaf", "kafka reb"] {
            assert_eq!(ix.search(typed, 10).unwrap().len(), 1, "typing {typed:?} found nothing");
        }
    }

    #[test]
    fn folds_polish_diacritics_so_accents_can_be_skipped_while_typing() {
        let (_d, ix) = vault_with("folds", &["Zażółć gęślą jaźń\n\nnotatka"]);
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
        let (_d, ix) = vault_with("stroked-l", &["Zażółć gęślą jaźń\n\nnotatka"]);
        assert_eq!(ix.search("zazołc", 10).unwrap().len(), 1);
        assert!(ix.search("zazolc", 10).unwrap().is_empty());
    }

    #[test]
    fn an_empty_query_lists_recent_notes_newest_first() {
        let (_d, ix) = vault_with("recent", &["First note", "Second note", "Third note"]);
        let hits = ix.search("   ", 10).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits[0].created >= hits[1].created);
    }

    #[test]
    fn title_matches_outrank_body_mentions() {
        let (_d, ix) = vault_with("title-rank", &[
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
        let (dir, mut ix) = vault_with("older-title", &[]);
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
        let (dir, mut ix) = vault_with("recency", &[]);
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
        let (_d, ix) = vault_with("snippets", &["Kafka rebalance\n\nconsumers were evicted"]);
        let snippet = &ix.search("evicted", 10).unwrap()[0].snippet;
        assert!(!snippet.contains("Kafka rebalance"), "snippet repeated the title: {snippet}");
    }

    #[test]
    fn sync_tracks_additions_edits_and_deletions() {
        let (dir, mut ix) = vault_with("sync", &["First note", "Second note"]);
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
        let (dir, mut ix) = vault_with("not-notes", &["Real note"]);
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

    /// A note at a known filename, so that another note's `[[link]]` can name it.
    /// The helper above slugs its own filenames, which link targets cannot guess.
    fn write(dir: &Path, name: &str, created: &str, body: &str) {
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!("---\ncreated: \"{created}\"\n---\n{body}\n"),
        )
        .unwrap();
    }

    fn names(refs: Vec<Ref>) -> Vec<String> {
        refs.into_iter().map(|r| r.name).collect()
    }

    #[test]
    fn backlinks_are_the_links_table_read_backwards() {
        let (dir, mut ix) = vault_with("backlinks", &[]);
        write(&dir, "kafka-rebalance", "2026-01-05T10:00:00+01:00", "Kafka rebalance\nthe original");
        write(&dir, "flink-timeouts", "2026-02-05T10:00:00+01:00", "Flink timeouts\nSame shape as [[kafka-rebalance]].");
        write(&dir, "poll-tuning", "2026-03-05T10:00:00+01:00", "Poll tuning\nFollows [[kafka-rebalance]].");
        ix.sync(&dir).unwrap();

        // Newest first, and it is the *linking* note that a backlink names.
        assert_eq!(names(ix.backlinks("kafka-rebalance").unwrap()), ["poll-tuning", "flink-timeouts"]);
        assert!(ix.backlinks("poll-tuning").unwrap().is_empty(), "links are not symmetric");
    }

    #[test]
    fn an_embedded_image_is_not_a_backlink() {
        // `![[shot.webp]]` is an attachment. Counting it would give every pasted
        // screenshot a phantom note pointing at it.
        let (dir, mut ix) = vault_with("embed-backlink", &[]);
        write(&dir, "with-a-picture", "2026-01-05T10:00:00+01:00", "With a picture\n![[2026-01-05-abc123.webp]]");
        ix.sync(&dir).unwrap();
        assert!(ix.backlinks("2026-01-05-abc123.webp").unwrap().is_empty());
    }

    #[test]
    fn a_note_that_links_to_itself_is_not_its_own_backlink() {
        let (dir, mut ix) = vault_with("self-link", &[]);
        write(&dir, "recursive", "2026-01-01T10:00:00+01:00", "Recursive\nSee [[recursive]].");
        ix.sync(&dir).unwrap();
        assert!(ix.backlinks("recursive").unwrap().is_empty());
    }

    #[test]
    fn a_supersession_is_found_from_the_note_it_corrects() {
        let (dir, mut ix) = vault_with("supersede", &[]);
        write(&dir, "poll-tuning", "2026-03-11T09:02:00+01:00", "Kafka poll tuning\nBumped max.poll.records.");
        write(&dir, "rebalance-storm", "2026-09-04T16:52:00+02:00", "Kafka rebalance storm\nsupersedes: [[poll-tuning]]");
        ix.sync(&dir).unwrap();

        let by = ix.superseded_by("poll-tuning").unwrap().expect("the correcting note");
        assert_eq!(by.name, "rebalance-storm");
        assert_eq!(by.title, "Kafka rebalance storm");
        // The relationship lives in the new note and points one way only.
        assert!(ix.superseded_by("rebalance-storm").unwrap().is_none());
    }

    #[test]
    fn the_frontmatter_form_of_supersedes_counts_too() {
        // DESIGN.md §2 documents it in frontmatter; the picker writes a body
        // line. A correction must not go quiet for having used the other one.
        let (dir, mut ix) = vault_with("supersede-fm", &[]);
        write(&dir, "original", "2026-01-01T10:00:00+01:00", "Original\ntext");
        std::fs::write(
            dir.join("correction.md"),
            "---\ncreated: \"2026-02-01T10:00:00+01:00\"\nsupersedes: \"[[original]]\"\n---\nCorrection\n",
        )
        .unwrap();
        ix.sync(&dir).unwrap();

        assert_eq!(ix.superseded_by("original").unwrap().unwrap().name, "correction");
    }

    #[test]
    fn the_newest_correction_is_the_one_the_banner_names() {
        let (dir, mut ix) = vault_with("newest-correction", &[]);
        write(&dir, "original", "2026-01-01T10:00:00+01:00", "Original\nwhat I believed");
        write(&dir, "first-fix", "2026-02-01T10:00:00+01:00", "First fix\nsupersedes: [[original]]");
        write(&dir, "second-fix", "2026-03-01T10:00:00+01:00", "Second fix\nsupersedes: [[original]]");
        ix.sync(&dir).unwrap();

        assert_eq!(ix.superseded_by("original").unwrap().unwrap().name, "second-fix");
        // The correction the banner does not name must stay reachable, or the
        // append-only record would be hiding one of its own entries.
        assert!(names(ix.backlinks("original").unwrap()).contains(&"first-fix".to_string()));
    }

    #[test]
    fn an_ordinary_mention_is_not_a_supersession() {
        let (dir, mut ix) = vault_with("mention-only", &[]);
        write(&dir, "original", "2026-01-01T10:00:00+01:00", "Original\ntext");
        write(&dir, "mentions", "2026-02-01T10:00:00+01:00", "Mentions\nSee [[original]].");
        ix.sync(&dir).unwrap();

        assert!(ix.superseded_by("original").unwrap().is_none());
        assert_eq!(names(ix.backlinks("original").unwrap()), ["mentions"]);
    }

    #[test]
    fn editing_a_note_to_drop_a_link_drops_the_backlink() {
        let (dir, mut ix) = vault_with("drop-link", &[]);
        write(&dir, "target", "2026-01-01T10:00:00+01:00", "Target\ntext");
        write(&dir, "source", "2026-02-01T10:00:00+01:00", "Source\nSee [[target]].");
        ix.sync(&dir).unwrap();
        assert_eq!(ix.backlinks("target").unwrap().len(), 1);

        write(&dir, "source", "2026-02-01T10:00:00+01:00", "Source\nthe reference is gone from this rewritten body");
        assert_eq!(ix.sync(&dir).unwrap().updated, 1);
        assert!(ix.backlinks("target").unwrap().is_empty(), "a stale backlink survived the edit");
    }

    #[test]
    fn a_deleted_note_takes_its_links_with_it() {
        let (dir, mut ix) = vault_with("delete-link", &[]);
        write(&dir, "target", "2026-01-01T10:00:00+01:00", "Target\ntext");
        write(&dir, "source", "2026-02-01T10:00:00+01:00", "Source\nSee [[target]].");
        ix.sync(&dir).unwrap();
        assert_eq!(ix.backlinks("target").unwrap().len(), 1);

        std::fs::remove_file(dir.join("source.md")).unwrap();
        assert_eq!(ix.sync(&dir).unwrap().removed, 1);
        assert!(ix.backlinks("target").unwrap().is_empty());
    }

    fn shot_names(shots: Vec<Shot>) -> Vec<String> {
        shots.into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn the_timeline_is_every_note_newest_first() {
        let (dir, mut ix) = vault_with("timeline", &[]);
        write(&dir, "january", "2026-01-05T10:00:00+01:00", "January\nbody");
        write(&dir, "march", "2026-03-05T10:00:00+01:00", "March\nbody");
        write(&dir, "february", "2026-02-05T10:00:00+01:00", "February\nbody");
        ix.sync(&dir).unwrap();

        assert_eq!(names(ix.timeline(50).unwrap()), ["march", "february", "january"]);
    }

    #[test]
    fn the_timeline_includes_notes_a_search_would_not_match() {
        // It is a browse, not a query: a note with no words in common with
        // anything still has a place in it.
        let (dir, mut ix) = vault_with("timeline-all", &[]);
        write(&dir, "wordless", "2026-01-05T10:00:00+01:00", "?????");
        ix.sync(&dir).unwrap();
        assert_eq!(ix.timeline(50).unwrap().len(), 1);
    }

    #[test]
    fn the_wall_lists_every_picture_newest_note_first() {
        let (dir, mut ix) = vault_with("wall", &[]);
        write(&dir, "older", "2026-01-05T10:00:00+01:00", "Older\n![[a.webp]]");
        write(&dir, "newer", "2026-03-05T10:00:00+01:00", "Newer\n![[b.webp]]\n![[c.webp]]");
        ix.sync(&dir).unwrap();

        // Newest note first, and within a note the order they were pasted in.
        assert_eq!(shot_names(ix.wall(50).unwrap()), ["b.webp", "c.webp", "a.webp"]);
    }

    #[test]
    fn a_wall_entry_names_the_note_it_came_from() {
        let (dir, mut ix) = vault_with("wall-note", &[]);
        write(&dir, "with-a-picture", "2026-01-05T10:00:00+01:00", "With a picture\n![[shot.webp]]");
        ix.sync(&dir).unwrap();

        let shot = &ix.wall(50).unwrap()[0];
        assert_eq!(shot.name, "shot.webp");
        assert_eq!(shot.note_name, "with-a-picture");
        assert_eq!(shot.title, "With a picture");
    }

    #[test]
    fn a_picture_reused_in_two_notes_appears_once() {
        // The wall is navigated by recognising the picture, so the same image
        // twice is two identical tiles and one wasted glance. It belongs to the
        // note most recently written about it.
        let (dir, mut ix) = vault_with("wall-dedupe", &[]);
        write(&dir, "first-time", "2026-01-05T10:00:00+01:00", "First time\n![[shot.webp]]");
        write(&dir, "again-later", "2026-06-05T10:00:00+01:00", "Again later\n![[shot.webp]]");
        ix.sync(&dir).unwrap();

        let shots = ix.wall(50).unwrap();
        assert_eq!(shot_names(shots.clone()), ["shot.webp"]);
        assert_eq!(shots[0].note_name, "again-later");
    }

    #[test]
    fn a_note_without_pictures_puts_nothing_on_the_wall() {
        let (dir, mut ix) = vault_with("wall-empty", &[]);
        write(&dir, "prose-only", "2026-01-05T10:00:00+01:00", "Prose only\nSee [[somewhere]].");
        ix.sync(&dir).unwrap();
        assert!(ix.wall(50).unwrap().is_empty(), "a [[link]] is not a picture");
    }

    #[test]
    fn editing_a_note_to_drop_a_picture_drops_it_from_the_wall() {
        let (dir, mut ix) = vault_with("wall-stale", &[]);
        write(&dir, "shots", "2026-01-05T10:00:00+01:00", "Shots\n![[a.webp]]\n![[b.webp]]");
        ix.sync(&dir).unwrap();
        assert_eq!(ix.wall(50).unwrap().len(), 2);

        write(&dir, "shots", "2026-01-05T10:00:00+01:00", "Shots\n![[a.webp]] and the other one is gone now");
        assert_eq!(ix.sync(&dir).unwrap().updated, 1);
        assert_eq!(shot_names(ix.wall(50).unwrap()), ["a.webp"], "a stale picture survived the edit");
    }

    #[test]
    fn an_index_from_an_older_schema_is_thrown_away_and_rebuilt() {
        let (dir, _) = vault_with("older-schema", &[]);
        write(&dir, "target", "2026-01-01T10:00:00+01:00", "Target\ntext");
        write(&dir, "source", "2026-02-01T10:00:00+01:00", "Source\nSee [[target]].");
        let db = dir.join("index.db");

        {
            let mut ix = Index::open(&db).unwrap();
            ix.sync(&dir).unwrap();
            // Rewind it to look like a file written before `links` existed.
            ix.db.execute_batch("DROP TABLE links;").unwrap();
            ix.db.pragma_update(None, "user_version", 0i64).unwrap();
        }

        // Reopening must rebuild. The notes themselves are unchanged on disk, so
        // without the version check `sync` would skip them as already indexed and
        // every note captured before the upgrade would have no backlinks at all.
        let mut ix = Index::open(&db).unwrap();
        assert!(ix.is_empty(), "the stale index should have been dropped");
        ix.sync(&dir).unwrap();
        assert_eq!(names(ix.backlinks("target").unwrap()), ["source"]);
    }
}
