//! All filesystem access: where the vault lives, how notes and drafts are written.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use photomem_core::Note;

/// The notes repository on disk.
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Vault {
        Vault { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    /// Derived state: index, thumbnails, drafts. Gitignored, always rebuildable.
    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".photomem")
    }

    fn draft_path(&self) -> PathBuf {
        self.state_dir().join("draft.md")
    }

    /// Create the vault layout if it is not there yet, including the gitignore
    /// that keeps derived state out of the repo.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(self.notes_dir())?;
        std::fs::create_dir_all(self.root.join("attachments"))?;
        std::fs::create_dir_all(self.state_dir())?;

        let gitignore = self.root.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, "/.photomem/\n")?;
        }
        Ok(())
    }

    /// Write a note, returning the path it landed at.
    ///
    /// Written to a temp file and renamed, so a crash mid-write cannot leave a
    /// half-written note where the indexer will find it.
    pub fn save(&self, note: &Note) -> Result<PathBuf> {
        self.ensure()?;
        let path = self.free_path(&note.filename());

        let tmp = self.state_dir().join(format!("tmp-{}", note.id));
        std::fs::write(&tmp, note.render()).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;

        Ok(path)
    }

    /// First unused path for `filename`, disambiguating with `-2`, `-3`, ….
    /// Two notes captured in the same minute with the same title are unlikely
    /// but must not silently overwrite each other.
    fn free_path(&self, filename: &str) -> PathBuf {
        let dir = self.notes_dir();
        let candidate = dir.join(filename);
        if !candidate.exists() {
            return candidate;
        }
        let stem = filename.strip_suffix(".md").unwrap_or(filename);
        (2..)
            .map(|n| dir.join(format!("{stem}-{n}.md")))
            .find(|p| !p.exists())
            .expect("an unused filename exists")
    }

    /// Stash unsaved editor text. Escape must never destroy what was typed.
    pub fn save_draft(&self, text: &str) -> Result<()> {
        std::fs::create_dir_all(self.state_dir())?;
        if text.trim().is_empty() {
            return self.clear_draft();
        }
        std::fs::write(self.draft_path(), text)?;
        Ok(())
    }

    pub fn load_draft(&self) -> String {
        std::fs::read_to_string(self.draft_path()).unwrap_or_default()
    }

    pub fn clear_draft(&self) -> Result<()> {
        match std::fs::remove_file(self.draft_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(name: &str) -> Vault {
        let root = std::env::temp_dir().join(format!("photomem-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Vault::new(root)
    }

    #[test]
    fn saves_a_note_and_reads_it_back() {
        let v = vault("save");
        let note = Note::new("Kafka rebalance\n\nbody").unwrap();
        let path = v.save(&note).unwrap();

        assert!(path.starts_with(v.notes_dir()));
        assert!(path.file_name().unwrap().to_str().unwrap().ends_with("-kafka-rebalance.md"));
        let doc = std::fs::read_to_string(&path).unwrap();
        assert_eq!(Note::parse(&doc, note.created).unwrap(), note);
    }

    #[test]
    fn ensure_creates_layout_and_gitignore() {
        let v = vault("ensure");
        v.ensure().unwrap();
        assert!(v.notes_dir().is_dir());
        assert!(v.root().join("attachments").is_dir());
        assert_eq!(std::fs::read_to_string(v.root().join(".gitignore")).unwrap(), "/.photomem/\n");
    }

    #[test]
    fn never_overwrites_a_colliding_filename() {
        let v = vault("collide");
        let a = Note::new("Same title").unwrap();
        let mut b = Note::new("Same title").unwrap();
        b.created = a.created;

        let pa = v.save(&a).unwrap();
        let pb = v.save(&b).unwrap();

        assert_ne!(pa, pb);
        assert!(pb.to_str().unwrap().ends_with("-same-title-2.md"));
        assert!(pa.exists() && pb.exists());
    }

    #[test]
    fn drafts_round_trip_and_clear() {
        let v = vault("draft");
        v.ensure().unwrap();

        v.save_draft("half a thought").unwrap();
        assert_eq!(v.load_draft(), "half a thought");

        // Saving an empty draft clears it rather than leaving a stale file.
        v.save_draft("  \n ").unwrap();
        assert_eq!(v.load_draft(), "");
    }

    #[test]
    fn missing_draft_is_empty_not_an_error() {
        assert_eq!(vault("nodraft").load_draft(), "");
    }
}
