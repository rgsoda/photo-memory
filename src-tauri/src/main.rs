// The window has no decorations of its own; on Windows this also stops a console
// flashing up behind it. Harmless elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::{Emitter, Manager, WebviewWindow};

/// How the process was started.
///
/// `photomem` opens the capture window. `photomem daemon` starts in the
/// background with the window hidden but built, so the first capture is as fast
/// as every later one — this is what the hotkey talks to. `photomem capture`
/// exists for that hotkey: it normally hands off to an already-running instance
/// via the single-instance plugin and exits, and only starts one itself if no
/// daemon is up.
enum Mode {
    Window,
    Daemon,
}

fn mode_from(args: impl Iterator<Item = String>) -> Mode {
    match args.skip(1).find(|a| !a.starts_with('-')).as_deref() {
        Some("daemon") => Mode::Daemon,
        _ => Mode::Window,
    }
}

fn main() {
    let mode = mode_from(std::env::args());

    tauri::Builder::default()
        // A second `photomem capture` must not start a second app. It wakes this
        // one instead, which is the whole reason the popup feels instant.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                present(&window);
            }
        }))
        .invoke_handler(tauri::generate_handler![
            commands::save_note,
            commands::save_draft,
            commands::load_draft,
            commands::dismiss,
            commands::paste_image,
            commands::thumbnails,
            commands::refresh,
            commands::search,
            commands::open_note,
            commands::read_image,
        ])
        .setup(move |app| {
            app.manage(commands::Search(std::sync::Mutex::new(open_index())));

            let window = app.get_webview_window("main").expect("main window exists");
            if let Mode::Window = mode {
                present(&window);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running photomem");
}

/// Open the on-disk index, falling back to an in-memory one.
///
/// A search that is briefly empty is a much better failure than a capture window
/// that will not start, so an unreadable index is never fatal.
fn open_index() -> photomem_index::Index {
    photomem_config::Config::load()
        .map(|cfg| photomem_store::Vault::new(cfg.vault).state_dir().join("index.db"))
        .and_then(|path| photomem_index::Index::open(&path).map_err(Into::into))
        .unwrap_or_else(|e| {
            eprintln!("photomem: falling back to an in-memory index: {e:#}");
            photomem_index::Index::in_memory().expect("in-memory index")
        })
}

/// Bring the capture window up and put the cursor in it.
///
/// Under Hyprland the compositor decides focus, so the window rules in the
/// README do the real work; these calls are what makes it behave on macOS.
fn present(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.center();
    let _ = window.set_focus();
    let _ = window.emit("photomem://present", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(args: &[&str]) -> &'static str {
        match mode_from(args.iter().map(|s| s.to_string())) {
            Mode::Window => "window",
            Mode::Daemon => "daemon",
        }
    }

    #[test]
    fn recognises_modes() {
        assert_eq!(mode(&["photomem"]), "window");
        assert_eq!(mode(&["photomem", "daemon"]), "daemon");
        // `capture` is handled by the single-instance plugin, not here: when no
        // daemon is running it must still open a window rather than exit.
        assert_eq!(mode(&["photomem", "capture"]), "window");
    }

    #[test]
    fn ignores_leading_flags() {
        assert_eq!(mode(&["photomem", "--verbose", "daemon"]), "daemon");
    }
}
