//! The status bar icon.
//!
//! photomem spends its life hidden, woken by a hotkey. That is the point, and
//! it is also the problem: an app with no window and no icon is one a person
//! cannot tell is running, cannot reach when they have forgotten the binding,
//! and cannot quit. The tray icon is the whole visible surface of the daemon.
//!
//! There are two implementations because there is no one library that does
//! this properly on both platforms. macOS uses Tauri's own tray. Linux talks
//! StatusNotifierItem directly through ksni, and the reason is the whole point
//! of the icon: Tauri's Linux tray goes through libappindicator, which exports
//! an `Activate` method and registers no handler behind it, so a left click
//! reaches the process and dies there. The bar is doing its part — there is
//! simply nothing listening. ksni handles `Activate`, so left click captures
//! on Linux exactly as it does on macOS.
//!
//! Dropping libappindicator also drops a system dependency the Linux build had
//! quietly acquired: without ayatana-appindicator3 installed, linking it fails.
//! ksni is Rust and D-Bus all the way down, and needs nothing installed.

/// Black, and recoloured by macOS to suit the menu bar; white, and used as-is
/// by a Linux bar. See icons/tray/render.sh for why there are two.
#[cfg(target_os = "linux")]
const ICON: &[u8] = include_bytes!("../icons/tray/tray-white.png");
#[cfg(not(target_os = "linux"))]
const ICON: &[u8] = include_bytes!("../icons/tray/tray-template.png");

const CAPTURE: &str = "New note";
const QUIT: &str = "Quit photomem";

#[cfg(target_os = "linux")]
pub use linux::build;
#[cfg(not(target_os = "linux"))]
pub use native::build;

fn present<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        crate::present(&window);
    }
}

/// Ends the daemon. Nothing is lost: a note is on disk the moment it is saved,
/// and an unsaved buffer is already in the draft file.
fn quit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    app.exit(0);
}

#[cfg(target_os = "linux")]
mod linux {
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem, Tray};
    use tauri::{AppHandle, Runtime};

    struct Photomem<R: Runtime> {
        app: AppHandle<R>,
    }

    impl<R: Runtime> Tray for Photomem<R> {
        fn id(&self) -> String {
            "photomem".into()
        }

        fn title(&self) -> String {
            "photomem".into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            icon().into_iter().collect()
        }

        /// Left click. The reason this file exists.
        fn activate(&mut self, _x: i32, _y: i32) {
            super::present(&self.app);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: super::CAPTURE.into(),
                    activate: Box::new(|this: &mut Self| super::present(&this.app)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: super::QUIT.into(),
                    activate: Box::new(|this: &mut Self| super::quit(&this.app)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// The icon as the spec wants it: ARGB32, network byte order.
    ///
    /// The PNG decodes to RGBA, which is the same four bytes in a different
    /// order — nothing is resampled here, only reordered.
    fn icon() -> Option<Icon> {
        let image = tauri::image::Image::from_bytes(super::ICON).ok()?;
        let data = image
            .rgba()
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        Some(Icon { width: image.width() as i32, height: image.height() as i32, data })
    }

    pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
        let handle = Photomem { app: app.clone() }
            .spawn()
            .map_err(|e| tauri::Error::Anyhow(anyhow::anyhow!("status bar icon: {e}")))?;

        // The icon lives as long as the process. Dropping this handle would
        // take it out of the bar, and there is nowhere better to keep it: it
        // is owned by nothing and outlives every scope here.
        std::mem::forget(handle);
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod native {
    use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::{AppHandle, Runtime};

    pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
        let capture = MenuItem::with_id(app, "capture", super::CAPTURE, true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", super::QUIT, true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&capture, &PredefinedMenuItem::separator(app)?, &quit])?;

        TrayIconBuilder::with_id("photomem")
            .icon(tauri::image::Image::from_bytes(super::ICON)?)
            .icon_as_template(true)
            .tooltip("photomem — new note")
            // Left click captures instead of opening the menu, because
            // capturing is what this app is for and a menu in the way of it is
            // a menu too many. The menu is still there on right click.
            .show_menu_on_left_click(false)
            .menu(&menu)
            .on_menu_event(on_menu)
            .on_tray_icon_event(|tray, event| {
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                // Act on release, not press: a click is not a click until the
                // button comes back up.
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    super::present(tray.app_handle());
                }
            })
            .build(app)?;

        Ok(())
    }

    fn on_menu<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
        match event.id().as_ref() {
            "capture" => super::present(app),
            "quit" => super::quit(app),
            _ => {}
        }
    }
}
