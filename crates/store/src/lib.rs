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
        std::fs::read(self.thumbs_dir().join(safe_name(name)?)).ok()
    }

    /// The thumbnail for an attachment, rebuilt from the image if it is absent.
    ///
    /// A vault cloned or pulled from another machine has every attachment and
    /// no thumbnails at all: they are gitignored, being derived. Without this
    /// a note written on one machine shows no pictures on the other — the
    /// images are right there, it is only the cache that is missing.
    pub fn thumbnail(&self, name: &str, opts: photomem_images::Options) -> Option<Vec<u8>> {
        if let Some(cached) = self.read_thumbnail(name) {
            return Some(cached);
        }
        let full = self.read_attachment(name)?;
        let thumb = photomem_images::thumbnail_of(&full, opts).ok()?;

        // Cached on the way out, so this costs one decode per image per machine
        // rather than one per time the note is opened.
        if let Some(safe) = safe_name(name) {
            let _ = std::fs::create_dir_all(self.thumbs_dir());
            let _ = write_atomic(&self.thumbs_dir().join(safe), &thumb, &self.state_dir());
        }
        Some(thumb)
    }

    /// The stored image at full size, for the lightbox.
    pub fn read_attachment(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.attachments_dir().join(safe_name(name)?)).ok()
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

/// Attachment names come out of note text, which is not necessarily ours: a
/// hand-edited note could name any path on the disk. Only a bare filename is
/// ever allowed through.
fn safe_name(name: &str) -> Option<&str> {
    let bad = name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || Path::new(name).is_absolute();
    (!bad).then_some(name)
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

    /// Larger than the thumbnail edge, so the full image and its thumbnail are
    /// genuinely different encodings.
    fn attachment(seed: u8) -> Attachment {
        let (w, h) = (500u32, 400u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i as u8).wrapping_add(seed)).collect();
        photomem_images::from_rgba(w, h, &px, Default::default()).unwrap()
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
    fn reads_refuse_to_escape_the_vault() {
        let v = vault("escape");
        v.ensure().unwrap();
        for bad in ["../../etc/passwd", "/etc/passwd", "sub/dir.webp", ""] {
            assert!(v.read_thumbnail(bad).is_none(), "thumbnail {bad:?} was allowed");
            assert!(v.read_attachment(bad).is_none(), "attachment {bad:?} was allowed");
        }
        assert!(v.read_thumbnail("nope.webp").is_none());
    }

    #[test]
    fn reads_back_a_stored_attachment_at_full_size() {
        let v = vault("fullsize");
        let a = attachment(3);
        let name = v.save_attachment(&a, "2026-09-04").unwrap();

        assert_eq!(v.read_attachment(&name).unwrap(), a.webp);
        // The lightbox must get the image, not the thumbnail.
        assert_ne!(v.read_attachment(&name), v.read_thumbnail(&name));
    }

    #[test]
    fn a_thumbnail_is_rebuilt_when_only_the_image_survived() {
        let v = vault("rethumb");
        let a = attachment(5);
        let name = v.save_attachment(&a, "2026-09-05").unwrap();

        // Exactly what a clone leaves behind: the attachment is committed, the
        // thumbnail is gitignored and is not.
        std::fs::remove_file(v.thumbs_dir().join(&name)).unwrap();
        assert!(v.read_thumbnail(&name).is_none());

        let rebuilt = v.thumbnail(&name, Default::default()).expect("rebuilt from the image");
        assert!(!rebuilt.is_empty());
        // And kept, so opening the note twice does not decode it twice.
        assert!(v.read_thumbnail(&name).is_some());
    }

    #[test]
    fn a_thumbnail_for_an_image_that_is_not_there_is_still_nothing() {
        let v = vault("rethumb-missing");
        v.ensure().unwrap();
        assert!(v.thumbnail("nope.webp", Default::default()).is_none());
        assert!(v.thumbnail("../../etc/passwd", Default::default()).is_none());
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
