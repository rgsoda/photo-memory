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
