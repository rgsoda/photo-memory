//! The whole API surface the UI has. Four calls.

use photomem_config::Config;
use photomem_core::Note;
use photomem_store::Vault;
use tauri::{Manager, Runtime, Window};

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
