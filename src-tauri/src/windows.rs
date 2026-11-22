//! The settings window and the floating windows: the status pill and the
//! captions above it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

pub const MAIN: &str = "main";
pub const OVERLAY: &str = "overlay";
pub const CAPTIONS: &str = "captions";

const OVERLAY_WIDTH: f64 = 380.0;
const OVERLAY_HEIGHT: f64 = 56.0;
/// Distance of the pill from the bottom of the screen's usable area (above
/// the Dock or taskbar).
const OVERLAY_MARGIN: f64 = 28.0;
const CAPTIONS_WIDTH: f64 = 680.0;
const CAPTIONS_HEIGHT: f64 = 60.0;

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

/// Which floating windows are meant to be on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Floating {
    pub pill: bool,
    pub captions: bool,
}

impl Floating {
    fn any(self) -> bool {
        self.pill || self.captions
    }
}

/// A floating window to create.
struct Spec {
    label: &'static str,
    page: &'static str,
    width: f64,
    height: f64,
    /// Whether it takes clicks. One that does not lets every click through
    /// to the app under it.
    clickable: bool,
}

const PILL: Spec = Spec {
    label: OVERLAY,
    page: "overlay.html",
    width: OVERLAY_WIDTH,
    height: OVERLAY_HEIGHT,
    clickable: true,
};

const CAPTIONS_WINDOW: Spec = Spec {
    label: CAPTIONS,
    page: "captions.html",
    width: CAPTIONS_WIDTH,
    height: CAPTIONS_HEIGHT,
    clickable: false,
};

/// Creates the pill and the captions window, hidden. The captions window
/// ignores the mouse.
pub fn create_floating(app: &AppHandle) -> tauri::Result<()> {
    for spec in [PILL, CAPTIONS_WINDOW] {
        create(app, &spec)?;
    }
    Ok(())
}

/// Creates a floating window, hidden.
///
/// On macOS it is a non-activating panel rather than a window: only a panel
/// can sit on top of another app's full-screen space, appear on every
/// space, and be shown without taking focus from the app being read in.
#[cfg(target_os = "macos")]
fn create(app: &AppHandle, spec: &Spec) -> tauri::Result<()> {
    use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};
    let clickable = spec.clickable;
    let panel = PanelBuilder::<_, OverlayPanel>::new(app, spec.label)
        .url(WebviewUrl::App(spec.page.into()))
        .title("Speachy")
        .size(tauri::Size::Logical(tauri::LogicalSize::new(
            spec.width,
            spec.height,
        )))
        .level(PanelLevel::Status)
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .ignores_mouse_events(!clickable)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .with_window(move |window| {
            window
                .decorations(false)
                .transparent(true)
                .resizable(false)
                .skip_taskbar(true)
                .focusable(false)
                .accept_first_mouse(clickable)
        })
        .build()?;
    panel.hide();
    Ok(())
}

/// Creates a floating window, hidden.
#[cfg(not(target_os = "macos"))]
fn create(app: &AppHandle, spec: &Spec) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, spec.label, WebviewUrl::App(spec.page.into()))
        .title("Speachy")
        .inner_size(spec.width, spec.height)
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
        .accept_first_mouse(spec.clickable)
        .visible(false)
        .build()?
        .set_ignore_cursor_events(!spec.clickable)
}

/// The floating windows meant to be on screen.
static WANTED: Mutex<Floating> = Mutex::new(Floating {
    pill: false,
    captions: false,
});
/// Whether the thread that keeps the floating windows on the user's screen
/// is running.
static FOLLOWING: AtomicBool = AtomicBool::new(false);

fn wanted() -> Floating {
    *WANTED.lock().unwrap()
}

/// Where the floating windows belong, on the screen the pointer is on,
/// which is the screen the user is working on: the pill at the bottom
/// centre of its usable area, the captions centred above the pill, or in
/// the pill's place when the pill is off.
fn positions(app: &AppHandle, want: Floating) -> Option<[PhysicalPosition<i32>; 2]> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let left = |width: f64| {
        f64::from(area.position.x) + (f64::from(area.size.width) - width * scale) / 2.0
    };
    let bottom = f64::from(area.position.y) + f64::from(area.size.height) - OVERLAY_MARGIN * scale;
    let pill_top = bottom - OVERLAY_HEIGHT * scale;
    let captions_bottom = if want.pill { pill_top } else { bottom };
    let at = |x: f64, y: f64| PhysicalPosition::new(x.round() as i32, y.round() as i32);
    Some([
        at(left(OVERLAY_WIDTH), pill_top),
        at(
            left(CAPTIONS_WIDTH),
            captions_bottom - CAPTIONS_HEIGHT * scale,
        ),
    ])
}

/// Moves the floating windows that are meant to be up to where they belong.
fn place(app: &AppHandle, want: Floating) {
    let Some(targets) = positions(app, want) else {
        return;
    };
    for ((label, shown), target) in [(OVERLAY, want.pill), (CAPTIONS, want.captions)]
        .into_iter()
        .zip(targets)
    {
        let Some(window) = app.get_webview_window(label).filter(|_| shown) else {
            continue;
        };
        if window.outer_position().ok() != Some(target) {
            let _ = window.set_position(target);
        }
    }
}

/// Shows, hides and places the floating windows to match `want`, and keeps
/// them on the screen the pointer is on while either is up.
pub fn set_floating(app: &AppHandle, want: Floating) {
    *WANTED.lock().unwrap() = want;
    place(app, want);
    for (label, shown) in [(OVERLAY, want.pill), (CAPTIONS, want.captions)] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = if shown { window.show() } else { window.hide() };
        }
    }
    if !want.any() || FOLLOWING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        while wanted().any() {
            std::thread::sleep(Duration::from_millis(400));
            place(&app, wanted());
        }
        FOLLOWING.store(false, Ordering::SeqCst);
        // Wanted again between the check and the flag: keep following.
        if wanted().any() {
            set_floating(&app, wanted());
        }
    });
}
