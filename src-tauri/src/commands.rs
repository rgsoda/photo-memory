//! The whole API surface the UI has. Four calls.

use std::sync::Mutex;

use photomem_config::Config;
use photomem_core::Note;
use photomem_index::Index;
use photomem_store::Vault;
use tauri::{Manager, Runtime, State, Window};

/// The index is opened once and reused; reopening it per keystroke would make
/// search-as-you-type noticeably slower than typing.
pub struct Search(pub Mutex<Index>);

/// Errors reach the UI as strings; there is nothing it can do but show them.
type CmdResult<T> = Result<T, String>;

fn vault() -> CmdResult<Vault> {
    let cfg = Config::load().map_err(|e| format!("{e:#}"))?;
    Ok(Vault::new(cfg.vault))
}

/// Save the buffer as a new note and clear the draft. Returns the path, which
/// the UI shows briefly as confirmation that it went somewhere real.
#[tauri::command]
pub fn save_note(body: String) -> CmdResult<String> {
    let note = Note::new(&body).map_err(|e| e.to_string())?;
    let vault = vault()?;
    let path = vault.save(&note).map_err(|e| format!("{e:#}"))?;
    vault.clear_draft().map_err(|e| format!("{e:#}"))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn save_draft(text: String) -> CmdResult<()> {
    vault()?.save_draft(&text).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn load_draft() -> CmdResult<String> {
    Ok(vault()?.load_draft())
}

/// Hide the window without exiting: the process stays warm for the next hotkey.
/// The draft is written first so Escape can never lose what was typed.
#[tauri::command]
pub fn dismiss<R: Runtime>(window: Window<R>, text: String) -> CmdResult<()> {
    vault()?.save_draft(&text).map_err(|e| format!("{e:#}"))?;
    let _ = window.get_webview_window("main").map(|w| w.hide());
    Ok(())
}

/// An image stored and ready to embed.
#[derive(serde::Serialize)]
pub struct Pasted {
    /// Filename to write into the note as `![[name]]`.
    name: String,
    /// The thumbnail, inlined as a data URL.
    ///
    /// These are a few kilobytes each, so inlining them costs less than opening
    /// Tauri's asset protocol to the whole vault would.
    thumb: String,
}

fn data_url(bytes: &[u8]) -> String {
    use base64::Engine;
    format!("data:image/webp;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Take the image currently on the clipboard, store it, and hand back its name.
///
/// The clipboard is read here rather than in the webview because WebKitGTK and
/// WKWebView disagree about what a paste event carries; the system clipboard is
/// the one source both platforms agree on.
#[tauri::command]
pub fn paste_image() -> CmdResult<Pasted> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    let image = clipboard.get_image().map_err(|_| "no image on the clipboard".to_string())?;

    let cfg = Config::load().map_err(|e| format!("{e:#}"))?;
    let attachment = photomem_images::from_rgba(
        image.width as u32,
        image.height as u32,
        &image.bytes,
        cfg.image.into(),
    )
    .map_err(|e| format!("{e:#}"))?;

    let vault = Vault::new(cfg.vault);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let name = vault.save_attachment(&attachment, &date).map_err(|e| format!("{e:#}"))?;

    Ok(Pasted { thumb: data_url(&attachment.thumb), name })
}

/// Thumbnails for images already referenced in the buffer, so a restored draft
/// shows its pictures again instead of bare filenames.
#[tauri::command]
pub fn thumbnails(names: Vec<String>) -> CmdResult<Vec<Pasted>> {
    let vault = vault()?;
    Ok(names
        .into_iter()
        .filter_map(|name| {
            let bytes = vault.read_thumbnail(&name)?;
            Some(Pasted { name, thumb: data_url(&bytes) })
        })
        .collect())
}

/// A search result as the list renders it.
#[derive(serde::Serialize)]
pub struct HitView {
    path: String,
    title: String,
    /// Pre-formatted for display; the UI does no date maths.
    when: String,
    snippet: String,
}

/// Bring the index in line with the notes on disk.
///
/// Called when the window is presented rather than on a timer: notes change
/// while the window is closed, and a scan of a few thousand files is faster
/// than the window takes to appear.
#[tauri::command]
pub fn refresh(search: State<'_, Search>) -> CmdResult<usize> {
    let vault = vault()?;
    let mut index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    index.sync(&vault.notes_dir()).map_err(|e| format!("{e:#}"))?;
    Ok(index.len())
}

#[tauri::command]
pub fn search(search: State<'_, Search>, query: String) -> CmdResult<Vec<HitView>> {
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    let hits = index.search(&query, 40).map_err(|e| format!("{e:#}"))?;
    Ok(hits
        .into_iter()
        .map(|h| HitView {
            path: h.path.display().to_string(),
            title: h.title,
            when: h.created.format("%Y-%m-%d %H:%M").to_string(),
            snippet: h.snippet,
        })
        .collect())
}

/// A note opened from search results. Read-only by design (see DESIGN.md §6).
#[derive(serde::Serialize)]
pub struct NoteView {
    title: String,
    when: String,
    /// Body with the title line removed, since the title is displayed separately.
    body: String,
    images: Vec<Pasted>,
}

#[tauri::command]
pub fn open_note(path: String) -> CmdResult<NoteView> {
    let vault = vault()?;
    let path = std::path::PathBuf::from(path);
    // Only ever read notes from inside the vault, whatever the UI passes.
    if !path.starts_with(vault.notes_dir()) {
        return Err("that note is not in the vault".into());
    }

    let text = std::fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
    let note = Note::parse(&text, chrono::Local::now()).map_err(|e| e.to_string())?;

    let images = embedded_names(note.content())
        .into_iter()
        .filter_map(|name| {
            let bytes = vault.read_thumbnail(&name)?;
            Some(Pasted { name, thumb: data_url(&bytes) })
        })
        .collect();

    Ok(NoteView {
        title: note.title().to_string(),
        when: note.created.format("%Y-%m-%d %H:%M").to_string(),
        body: without_embed_lines(note.content()),
        images,
    })
}

/// Drop lines that are nothing but an embed.
///
/// The viewer renders those images directly underneath, so leaving the raw
/// `![[…]]` in the text shows the reader the filename twice and the picture
/// once. An embed used mid-sentence is left alone.
fn without_embed_lines(body: &str) -> String {
    body.lines()
        .filter(|line| {
            let t = line.trim();
            !(t.starts_with("![[") && t.ends_with("]]") && t.matches("![[").count() == 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Attachment names embedded as `![[name]]`, in order, without duplicates.
fn embedded_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (_, rest) in body.match_indices("![[").map(|(i, _)| (i, &body[i + 3..])) {
        if let Some(end) = rest.find("]]") {
            let name = rest[..end].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::{embedded_names, without_embed_lines};

    #[test]
    fn strips_standalone_embed_lines_only() {
        let body = "before\n![[a.webp]]\nsee ![[b.webp]] inline\nafter";
        assert_eq!(without_embed_lines(body), "before\nsee ![[b.webp]] inline\nafter");
    }

    #[test]
    fn a_note_that_is_only_an_image_has_an_empty_body() {
        assert_eq!(without_embed_lines("![[a.webp]]"), "");
    }

    #[test]
    fn reads_embeds_in_order_without_duplicates() {
        let body = "One ![[a.webp]]\ntext\n![[b.webp]] and ![[a.webp]] again\n";
        assert_eq!(embedded_names(body), vec!["a.webp", "b.webp"]);
    }

    #[test]
    fn ignores_unterminated_or_absent_embeds() {
        assert!(embedded_names("no images here").is_empty());
        assert!(embedded_names("![[never closed").is_empty());
    }
}
