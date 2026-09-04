//! Git sync for the vault.
//!
//! This drives the `git` binary rather than linking libgit2, on purpose. The
//! vault is a repository the user owns and will sometimes fix by hand, so sync
//! has to honour the same SSH keys, credential helpers, `commit.gpgsign` and
//! `.gitignore` that their own `git` commands do. A second implementation of
//! git's configuration is exactly the kind of thing that works until the day it
//! matters.
//!
//! Everything here is best-effort. A failed sync must never cost a note: the
//! note is already written to disk before any of this runs, and every error
//! surfaces as a status line rather than a lost capture.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Given to `ssh` so a dead network cannot hang a sync thread forever.
///
/// `Command` has no timeout of its own, and a push to an unreachable host would
/// otherwise sit there until the process exits, holding the sync lock and
/// blocking every later save from committing.
const SSH_TIMEOUT: &str = "ssh -o ConnectTimeout=10 -o BatchMode=yes";

/// A vault that is under git.
pub struct Repo {
    root: PathBuf,
}

/// What a sync actually did, so the UI can say something true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    /// Nothing had changed.
    Nothing,
    /// Committed locally; the vault has no remote to push to.
    Committed,
    /// Committed and pushed.
    Pushed,
}

impl Repo {
    /// Open the vault as a repository, or `None` if it is not one.
    ///
    /// Not being a git repo is the normal state for a new vault, not an error:
    /// photomem works perfectly well without sync, and turning it on is the
    /// user running `git init` when they are ready.
    pub fn open(root: &Path) -> Option<Repo> {
        root.join(".git").exists().then(|| Repo { root: root.to_path_buf() })
    }

    /// Commit everything currently in the vault, then push if there is anywhere
    /// to push to.
    ///
    /// One commit per save, named after the note, because that is the history a
    /// person would want to read later — not a nightly batch that says
    /// "sync 14 files".
    pub fn save(&self, message: &str) -> Result<Synced> {
        if !self.is_dirty()? {
            return Ok(Synced::Nothing);
        }
        self.git(&["add", "-A"])?;
        self.git(&["commit", "-m", message])?;

        if !self.has_upstream() {
            return Ok(Synced::Committed);
        }
        // Rebase rather than merge: the history is a list of captures, and a
        // merge commit between two machines that each added a file says
        // nothing worth reading.
        self.git(&["pull", "--rebase", "--autostash"])?;
        self.git(&["push"])?;
        Ok(Synced::Pushed)
    }

    /// Bring in notes captured on another machine.
    ///
    /// Returns whether anything arrived, since the caller only needs to
    /// reindex when something did.
    pub fn pull(&self) -> Result<bool> {
        if !self.has_upstream() {
            return Ok(false);
        }
        let before = self.head()?;
        // `--autostash` covers the one file that can be dirty here: a note
        // saved while the pull was already in flight.
        self.git(&["pull", "--rebase", "--autostash"])?;
        Ok(self.head()? != before)
    }

    /// Whether the working tree has anything git would record.
    ///
    /// Checked before committing so an unchanged vault does not produce an
    /// empty commit on every save of a note that was already synced.
    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.git(&["status", "--porcelain"])?.trim().is_empty())
    }

    /// Whether the current branch tracks a remote one.
    ///
    /// A vault that is a local repo with no remote is a perfectly good setup —
    /// versioned, just not shared — so this is a question, not a precondition.
    pub fn has_upstream(&self) -> bool {
        self.git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).is_ok()
    }

    fn head(&self) -> Result<String> {
        self.git(&["rev-parse", "HEAD"])
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            // Never stop for a passphrase or a username: there is no terminal
            // behind this thread, and a prompt nobody can answer is a hang.
            .arg("-c")
            .arg(format!("core.sshCommand={SSH_TIMEOUT}"))
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .with_context(|| format!("running git {}", args.join(" ")))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.lines().last().unwrap_or("git failed").trim();
            bail!("git {}: {}", args[0], err);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// A one-line commit message for a saved note.
///
/// Git treats the first line as the subject, so a note whose title runs long or
/// wraps would otherwise produce a commit whose subject is a paragraph.
pub fn message_for(title: &str) -> String {
    const MAX: usize = 60;
    let title = title.split('\n').next().unwrap_or("").trim();
    if title.is_empty() {
        return "note".to_string();
    }
    if title.chars().count() <= MAX {
        return title.to_string();
    }
    let cut: String = title.chars().take(MAX).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).unwrap_or(&cut);
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("photomem-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A vault repo with an identity, so commits work on a bare CI machine too.
    fn vault(root: &Path) -> Repo {
        std::fs::create_dir_all(root.join("notes")).unwrap();
        run(root, &["init", "-b", "main"]);
        run(root, &["config", "user.email", "test@example.com"]);
        run(root, &["config", "user.name", "Test"]);
        run(root, &["config", "commit.gpgsign", "false"]);
        Repo::open(root).unwrap()
    }

    fn write_note(root: &Path, name: &str, body: &str) {
        std::fs::write(root.join("notes").join(format!("{name}.md")), body).unwrap();
    }

    #[test]
    fn a_vault_without_git_is_not_a_repo() {
        let dir = tmpdir("plain");
        assert!(Repo::open(&dir).is_none(), "sync is opt-in, not a requirement");
    }

    #[test]
    fn saving_commits_and_reports_no_remote() {
        let dir = tmpdir("commit");
        let repo = vault(&dir);
        write_note(&dir, "one", "First\nbody");

        assert_eq!(repo.save("First").unwrap(), Synced::Committed);
        // Committed, so the tree is clean and a second save has nothing to do.
        assert_eq!(repo.save("First").unwrap(), Synced::Nothing);
    }

    #[test]
    fn saving_pushes_when_there_is_an_upstream() {
        let dir = tmpdir("push");
        let remote = tmpdir("push-remote");
        run(&remote, &["init", "--bare", "-b", "main"]);

        let repo = vault(&dir);
        write_note(&dir, "one", "First\nbody");
        repo.save("First").unwrap();
        run(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        run(&dir, &["push", "-u", "origin", "main"]);

        write_note(&dir, "two", "Second\nbody");
        assert_eq!(repo.save("Second").unwrap(), Synced::Pushed);

        // And the note is really in the remote, not just committed locally.
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(["ls-tree", "-r", "--name-only", "main"])
            .output()
            .unwrap();
        let files = String::from_utf8_lossy(&out.stdout);
        assert!(files.contains("notes/two.md"), "remote has: {files}");
    }

    #[test]
    fn pulling_reports_whether_anything_arrived() {
        let dir = tmpdir("pull");
        let other = tmpdir("pull-other");
        let remote = tmpdir("pull-remote");
        run(&remote, &["init", "--bare", "-b", "main"]);

        let repo = vault(&dir);
        write_note(&dir, "one", "First\nbody");
        repo.save("First").unwrap();
        run(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        run(&dir, &["push", "-u", "origin", "main"]);

        assert!(!repo.pull().unwrap(), "nothing has changed on the remote yet");

        // A second machine captures a note.
        run(&other, &["clone", remote.to_str().unwrap(), "."]);
        run(&other, &["config", "user.email", "test@example.com"]);
        run(&other, &["config", "user.name", "Test"]);
        write_note(&other, "elsewhere", "From the laptop\nbody");
        run(&other, &["add", "-A"]);
        run(&other, &["commit", "-m", "From the laptop"]);
        run(&other, &["push"]);

        assert!(repo.pull().unwrap(), "the other machine's note should arrive");
        assert!(dir.join("notes/elsewhere.md").exists());
    }

    #[test]
    fn a_note_saved_on_both_machines_rebases_rather_than_conflicting() {
        // Notes are append-only and named for their own timestamp and slug, so
        // two machines adding notes touch different files. This is the case
        // sync has to get right, and it is the common one.
        let dir = tmpdir("concurrent");
        let other = tmpdir("concurrent-other");
        let remote = tmpdir("concurrent-remote");
        run(&remote, &["init", "--bare", "-b", "main"]);

        let repo = vault(&dir);
        write_note(&dir, "one", "First\nbody");
        repo.save("First").unwrap();
        run(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        run(&dir, &["push", "-u", "origin", "main"]);

        run(&other, &["clone", remote.to_str().unwrap(), "."]);
        run(&other, &["config", "user.email", "test@example.com"]);
        run(&other, &["config", "user.name", "Test"]);
        write_note(&other, "laptop", "From the laptop\nbody");
        run(&other, &["add", "-A"]);
        run(&other, &["commit", "-m", "From the laptop"]);
        run(&other, &["push"]);

        // Meanwhile this machine captures its own, without having pulled.
        write_note(&dir, "desktop", "From the desktop\nbody");
        assert_eq!(repo.save("From the desktop").unwrap(), Synced::Pushed);

        assert!(dir.join("notes/laptop.md").exists(), "the other note arrived");
        assert!(dir.join("notes/desktop.md").exists(), "and ours survived");
    }

    #[test]
    fn commit_subjects_stay_one_short_line() {
        assert_eq!(message_for("Kafka poll tuning"), "Kafka poll tuning");
        assert_eq!(message_for("Title\nand the body"), "Title");
        assert_eq!(message_for("   "), "note");

        let long = "a note whose title just keeps going and going well past the point of usefulness";
        let msg = message_for(long);
        assert!(msg.chars().count() <= 61, "{msg:?}");
        assert!(msg.ends_with('…'));
        // Cut at a word boundary, not mid-word.
        assert!(long.starts_with(msg.trim_end_matches('…')));
    }
}
