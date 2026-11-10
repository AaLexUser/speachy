//! The settings window and the floating status pill.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const MAIN: &str = "main";
pub const OVERLAY: &str = "overlay";

const OVERLAY_WIDTH: f64 = 340.0;
const OVERLAY_HEIGHT: f64 = 56.0;
/// Distance of the pill from the bottom of the screen.
const OVERLAY_MARGIN: f64 = 96.0;

pub fn create_main(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Speachy")
        .inner_size(940.0, 660.0)
        .min_inner_size(760.0, 520.0)
        .visible(visible);
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    builder.build()
}

/// Shows the settings window, creating it if it was never opened.
pub fn show_main(app: &AppHandle) {
    // A Dock icon and app menu only while the window is open.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let window = match app.get_webview_window(MAIN) {
        Some(window) => window,
        None => match create_main(app, true) {
            Ok(window) => window,
            Err(err) => {
                log::error!("could not create the main window: {err}");
                return;
            }
        },
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Hides the settings window; the app keeps running in the menu bar.
pub fn hide_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

pub fn create_overlay(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay.html".into()))
        .title("Speachy")
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        // Never take keyboard focus away from the app the user is reading in.
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .visible(false)
        .build()
}

/// Shows the pill at the bottom centre of the screen the main display is on.
pub fn show_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window(OVERLAY) else { return };
    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let size = monitor.size().to_logical::<f64>(scale);
        let origin = monitor.position().to_logical::<f64>(scale);
        let x = origin.x + (size.width - OVERLAY_WIDTH) / 2.0;
        let y = origin.y + size.height - OVERLAY_HEIGHT - OVERLAY_MARGIN;
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    let _ = window.show();
}

pub fn hide_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.hide();
    }
}
