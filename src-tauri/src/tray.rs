//! The status bar icon.
//!
//! photomem spends its life hidden, woken by a hotkey. That is the point, and
//! it is also the problem: an app with no window and no icon is one a person
//! cannot tell is running, cannot reach when they have forgotten the binding,
//! and cannot quit. The tray icon is the whole visible surface of the daemon.
//!
//! The two platforms disagree about what a tray icon is, and the disagreement
//! decides the design here. On macOS it is a button that reports clicks. On
//! Linux it is a menu and nothing else: the StatusNotifierItem hosts reached
//! through libappindicator deliver no click events at all, so anything only a
//! click could do would be unreachable there. Everything is therefore in the
//! menu, and the click is a shortcut to the first item rather than the only
//! way to it.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

/// Black, and recoloured by macOS to suit the menu bar; white, and used as-is
/// by a Linux bar. See icons/tray/render.sh for why there are two.
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../icons/tray/tray-template.png");
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../icons/tray/tray-white.png");

/// Put photomem in the status bar.
///
/// Failing to do so is not fatal: a machine with no system tray at all is a
/// normal machine, and the hotkey is the primary way in regardless.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let capture = MenuItem::with_id(app, "capture", "New note", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit photomem", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&capture, &PredefinedMenuItem::separator(app)?, &quit])?;

    TrayIconBuilder::with_id("photomem")
        .icon(tauri::image::Image::from_bytes(ICON)?)
        .icon_as_template(true)
        .tooltip("photomem — new note")
        // Left click captures instead of opening the menu, because capturing is
        // what this app is for and a menu in the way of it is a menu too many.
        // The menu is still there on right click, and on Linux it is the only
        // thing there.
        .show_menu_on_left_click(false)
        .menu(&menu)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            // Act on release, not press: a click is not a click until the
            // button comes back up, and firing on the way down would capture
            // out from under someone dragging the icon.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                present(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn on_menu<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        "capture" => present(app),
        // Ends the daemon. Nothing is lost: notes are on disk the moment they
        // are saved, and an unsaved buffer is already in the draft file.
        "quit" => app.exit(0),
        _ => {}
    }
}

fn present<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        crate::present(&window);
    }
}
