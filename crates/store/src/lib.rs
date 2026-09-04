//! All filesystem access: where the vault lives, how notes and drafts are written.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use photomem_core::Note;
use photomem_images::Attachment;

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

    pub fn attachments_dir(&self) -> PathBuf {
        self.root.join("attachments")
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.state_dir().join("thumbs")
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

        write_atomic(&path, note.render().as_bytes(), &self.state_dir())?;
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

    /// Write an attachment and its thumbnail, returning the filename to embed.
    ///
    /// The same image pasted twice reuses the existing file rather than storing
    /// a second copy — the name is content-derived, so this is safe.
    pub fn save_attachment(&self, a: &Attachment, date: &str) -> Result<String> {
        self.ensure()?;
        std::fs::create_dir_all(self.thumbs_dir())?;

        let name = match self.existing_attachment(&a.hash) {
            Some(name) => name,
            None => {
                let name = format!("{date}-{}.webp", a.hash);
                write_atomic(&self.attachments_dir().join(&name), &a.webp, &self.state_dir())?;
                name
            }
        };

        // The thumbnail is derived state and may be missing even when the image
        // is not, after a fresh clone or a cleared cache.
        let thumb = self.thumbs_dir().join(&name);
        if !thumb.exists() {
            write_atomic(&thumb, &a.thumb, &self.state_dir())?;
        }
        Ok(name)
    }

    /// An already-stored attachment with this content hash, whatever date it
    /// was first captured on.
    fn existing_attachment(&self, hash: &str) -> Option<String> {
        let suffix = format!("-{hash}.webp");
        std::fs::read_dir(self.attachments_dir())
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.ends_with(&suffix))
    }

    pub fn read_thumbnail(&self, name: &str) -> Option<Vec<u8>> {
        // Reject anything that could climb out of the thumbnails directory: the
        // name arrives from note text, which is not necessarily ours.
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return None;
        }
        std::fs::read(self.thumbs_dir().join(name)).ok()
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

/// Write via a temp file in `scratch` and rename, so a crash mid-write cannot
/// leave a half-written file where a reader will find it.
fn write_atomic(path: &Path, bytes: &[u8], scratch: &Path) -> Result<()> {
    std::fs::create_dir_all(scratch)?;
    let tmp = scratch.join(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
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

    fn attachment(seed: u8) -> Attachment {
        let px: Vec<u8> = (0..64 * 64 * 4).map(|i| (i as u8).wrapping_add(seed)).collect();
        photomem_images::from_rgba(64, 64, &px, Default::default()).unwrap()
    }

    #[test]
    fn saves_an_attachment_with_its_thumbnail() {
        let v = vault("attach");
        let a = attachment(0);
        let name = v.save_attachment(&a, "2026-09-04").unwrap();

        assert_eq!(name, format!("2026-09-04-{}.webp", a.hash));
        assert_eq!(std::fs::read(v.attachments_dir().join(&name)).unwrap(), a.webp);
        assert_eq!(v.read_thumbnail(&name).unwrap(), a.thumb);
    }

    #[test]
    fn the_same_image_is_stored_once_even_on_a_later_day() {
        let v = vault("dedupe");
        let a = attachment(0);
        let first = v.save_attachment(&a, "2026-09-04").unwrap();
        let second = v.save_attachment(&a, "2026-11-20").unwrap();

        assert_eq!(first, second, "a re-paste must reuse the stored file");
        assert_eq!(std::fs::read_dir(v.attachments_dir()).unwrap().count(), 1);
    }

    #[test]
    fn different_images_get_different_names() {
        let v = vault("distinct");
        let a = v.save_attachment(&attachment(0), "2026-09-04").unwrap();
        let b = v.save_attachment(&attachment(9), "2026-09-04").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn thumbnail_reads_refuse_to_escape_the_vault() {
        let v = vault("escape");
        v.ensure().unwrap();
        assert!(v.read_thumbnail("../../etc/passwd").is_none());
        assert!(v.read_thumbnail("nope.webp").is_none());
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
