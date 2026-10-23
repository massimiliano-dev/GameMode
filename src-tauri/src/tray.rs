//! Tray icon for the "desktop once" session: console mode is on, but this
//! sign-in landed on the Windows desktop (see `shell::ShellStatus::desktop_once`).
//! GameMode starts hidden, and the tray is the way back in: open it, restart
//! into console mode, or quit. In console mode proper there is no taskbar,
//! so no tray either.

use tauri::AppHandle;

#[cfg(windows)]
mod imp {
    use tauri::menu::MenuBuilder;
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::AppHandle;

    const OPEN: &str = "open";
    const CONSOLE: &str = "console";
    const QUIT: &str = "quit";

    pub fn create(app: &AppHandle) -> tauri::Result<()> {
        let menu = MenuBuilder::new(app)
            .text(OPEN, "Apri GameMode")
            .separator()
            .text(CONSOLE, "Riavvia in modalità console")
            .separator()
            .text(QUIT, "Esci")
            .build()?;
        let mut tray = TrayIconBuilder::with_id("main")
            .tooltip("GameMode")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id().as_ref() {
                OPEN => crate::window::bring_to_front(app),
                CONSOLE => {
                    if let Err(err) = crate::power(app.clone(), crate::system::PowerAction::Restart) {
                        eprintln!("restart to console: {err}");
                    }
                }
                QUIT => crate::quit_app(app.clone()),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    crate::window::bring_to_front(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        // Created before Explorer's taskbar exists: tray-icon re-adds the
        // icon on `TaskbarCreated`, so the order doesn't matter.
        tray.build(app)?;
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::AppHandle;

    pub fn create(_app: &AppHandle) -> tauri::Result<()> {
        Ok(())
    }
}

/// Puts GameMode in the notification area. A failure is logged, not fatal:
/// starting GameMode again (Start menu) brings the window back anyway.
pub fn create(app: &AppHandle) {
    if let Err(err) = imp::create(app) {
        eprintln!("tray icon: {err}");
    }
}
