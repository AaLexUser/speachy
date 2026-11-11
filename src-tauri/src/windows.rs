//! The settings window and the floating status pill.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

pub const MAIN: &str = "main";
pub const OVERLAY: &str = "overlay";

const OVERLAY_WIDTH: f64 = 340.0;
const OVERLAY_HEIGHT: f64 = 56.0;
/// Distance of the pill from the bottom of the screen's usable area (above
/// the Dock or taskbar).
const OVERLAY_MARGIN: f64 = 28.0;

pub fn create_main(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Speachy")
        .inner_size(940.0, 660.0)
        .min_inner_size(760.0, 520.0)
        .visible(visible);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
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

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Creates the pill, hidden.
///
/// On macOS it is a non-activating panel rather than a window: only a panel
/// can sit on top of another app's full-screen space, appear on every
/// space, and be shown without taking focus from the app being read in.
#[cfg(target_os = "macos")]
pub fn create_overlay(app: &AppHandle) -> tauri::Result<()> {
    use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};
    let panel = PanelBuilder::<_, OverlayPanel>::new(app, OVERLAY)
        .url(WebviewUrl::App("overlay.html".into()))
        .title("Speachy")
        .size(tauri::Size::Logical(tauri::LogicalSize::new(
            OVERLAY_WIDTH,
            OVERLAY_HEIGHT,
        )))
        .level(PanelLevel::Status)
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .with_window(|window| {
            window
                .decorations(false)
                .transparent(true)
                .resizable(false)
                .skip_taskbar(true)
                .focusable(false)
                .accept_first_mouse(true)
        })
        .build()?;
    panel.hide();
    Ok(())
}

/// Creates the pill, hidden.
#[cfg(not(target_os = "macos"))]
pub fn create_overlay(app: &AppHandle) -> tauri::Result<()> {
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
        .build()?;
    Ok(())
}

/// Whether the pill is meant to be on screen.
static OVERLAY_SHOWN: AtomicBool = AtomicBool::new(false);
/// Whether the thread that keeps the pill on the user's screen is running.
static FOLLOWING: AtomicBool = AtomicBool::new(false);

/// Where the pill belongs: bottom centre of the usable area of the screen
/// the pointer is on, which is the screen the user is working on.
fn overlay_position(app: &AppHandle) -> Option<PhysicalPosition<i32>> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let width = OVERLAY_WIDTH * scale;
    let height = (OVERLAY_HEIGHT + OVERLAY_MARGIN) * scale;
    let x = f64::from(area.position.x) + (f64::from(area.size.width) - width) / 2.0;
    let y = f64::from(area.position.y) + f64::from(area.size.height) - height;
    Some(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

/// Moves the pill to where it belongs, if it is not already there.
fn place_overlay(app: &AppHandle, window: &WebviewWindow) {
    if let Some(target) = overlay_position(app) {
        if window.outer_position().ok() != Some(target) {
            let _ = window.set_position(target);
        }
    }
}

/// Shows the pill on the screen the user is working on, and keeps it there
/// if they move to another screen while it is up.
pub fn show_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window(OVERLAY) else {
        return;
    };
    place_overlay(app, &window);
    OVERLAY_SHOWN.store(true, Ordering::SeqCst);
    let _ = window.show();
    if FOLLOWING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        while OVERLAY_SHOWN.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(400));
            place_overlay(&app, &window);
        }
        FOLLOWING.store(false, Ordering::SeqCst);
        // Shown again between the check and the flag: keep following.
        if OVERLAY_SHOWN.load(Ordering::SeqCst) {
            show_overlay(&app);
        }
    });
}

pub fn hide_overlay(app: &AppHandle) {
    OVERLAY_SHOWN.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.hide();
    }
}
