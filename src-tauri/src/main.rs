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
    /// Print usage and exit. Anything that looks like a request for help is
    /// this: `--help` opening a window instead of answering would be a rude
    /// surprise, and the first thing anyone types at an unfamiliar binary.
    Help,
    /// Print the version and exit.
    Version,
}

const USAGE: &str = "\
photomem — fast visual note capture

USAGE:
    photomem            open the capture window
    photomem capture    wake the running instance, or open one
    photomem daemon     start hidden and stay warm for the hotkey

OPTIONS:
    -h, --help          print this
    -V, --version       print the version

Configuration lives in ~/.config/photomem/config.toml.
The hotkey is a compositor binding on Linux; see the README.";

fn mode_from(args: impl Iterator<Item = String>) -> Mode {
    let mut mode = Mode::Window;
    for arg in args.skip(1) {
        match arg.as_str() {
            "-h" | "--help" | "help" => return Mode::Help,
            "-V" | "--version" | "version" => return Mode::Version,
            "daemon" => mode = Mode::Daemon,
            _ => {}
        }
    }
    mode
}

fn main() {
    let mode = mode_from(std::env::args());

    // Before the Tauri builder, so neither of these starts a GUI runtime.
    match mode {
        Mode::Help => return println!("{USAGE}"),
        Mode::Version => return println!("photomem {}", env!("CARGO_PKG_VERSION")),
        _ => {}
    }

    let builder = tauri::Builder::default()
        // A second `photomem capture` must not start a second app. It wakes this
        // one instead, which is the whole reason the popup feels instant.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                present(&window);
            }
        }));

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::save_note,
            commands::save_draft,
            commands::load_draft,
            commands::dismiss,
            commands::paste_image,
            commands::thumbnails,
            commands::search,
            commands::wall,
            commands::timeline,
            commands::open_note,
            commands::open_link,
            commands::read_image,
            commands::open_image,
            commands::close_image,
            commands::fit_image_window,
        ])
        .setup(move |app| {
            app.manage(commands::Search(std::sync::Mutex::new(open_index())));
            #[cfg(target_os = "macos")]
            register_hotkey(app.handle());

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

/// Register the global capture hotkey.
///
/// macOS only. Wayland has no protocol for an application to register a global
/// shortcut, so on Linux the binding lives in the compositor config and this
/// does not exist — see the README. Here it goes through Carbon's
/// `RegisterEventHotKey`, which asks for no accessibility permission and so
/// raises no permission prompt on first run.
///
/// A hotkey that will not bind is reported and shrugged off. `photomem capture`
/// still opens the window, and a capture tool that starts with one way in is a
/// much better failure than one that refuses to start at all.
#[cfg(target_os = "macos")]
fn register_hotkey(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let spec = photomem_config::Config::load()
        .map(|cfg| cfg.hotkey)
        .unwrap_or_else(|_| photomem_config::DEFAULT_HOTKEY.to_string());

    let shortcut: tauri_plugin_global_shortcut::Shortcut = match spec.parse() {
        Ok(s) => s,
        Err(e) => return eprintln!("photomem: {spec:?} is not a usable hotkey: {e}"),
    };

    let bound = app.global_shortcut().on_shortcut(shortcut, |app, _, event| {
        // Press and release both arrive here; without this the window would be
        // presented twice for every capture.
        if event.state() != ShortcutState::Pressed {
            return;
        }
        if let Some(window) = app.get_webview_window("main") {
            present(&window);
        }
    });
    if let Err(e) = bound {
        eprintln!("photomem: could not bind {spec}: {e}");
    }
}

/// Bring the capture window up and put the cursor in it.
///
/// Under Hyprland the compositor decides focus, so the window rules in the
/// README do the real work; these calls are what makes it behave on macOS.
fn present(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.center();
    let _ = window.set_focus();

    // Bring the index up to date here rather than asking the page to do it:
    // notes change while the window is closed, and this is the moment they are
    // about to be searched. Doing it in the frontend put a backend concern
    // behind an `invoke` that could quietly not happen.
    commands::refresh_index(window.app_handle());

    // Notes captured on the other machine arrive here. Off the UI thread: the
    // window must appear now, not after a network round trip.
    commands::pull_in_background(window.app_handle());

    let _ = window.emit("photomem://present", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(args: &[&str]) -> &'static str {
        match mode_from(args.iter().map(|s| s.to_string())) {
            Mode::Window => "window",
            Mode::Daemon => "daemon",
            Mode::Help => "help",
            Mode::Version => "version",
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

    #[test]
    fn help_and_version_never_open_a_window() {
        for spelling in ["-h", "--help", "help"] {
            assert_eq!(mode(&["photomem", spelling]), "help", "{spelling}");
        }
        for spelling in ["-V", "--version", "version"] {
            assert_eq!(mode(&["photomem", spelling]), "version", "{spelling}");
        }
        // Even alongside a real mode: asking for help wins over doing the thing.
        assert_eq!(mode(&["photomem", "daemon", "--help"]), "help");
    }

    #[test]
    fn usage_names_every_mode() {
        for word in ["capture", "daemon", "--help", "--version"] {
            assert!(USAGE.contains(word), "usage does not mention {word}");
        }
    }
}
