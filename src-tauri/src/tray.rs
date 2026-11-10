//! The menu-bar (system tray) icon.

use crate::shortcuts::{self, Action};
use crate::{speech, windows};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let menu = Menu::with_items(
        app,
        &[
            &item("speak_selection", "Speak Selection")?,
            &item("speak_clipboard", "Speak Clipboard")?,
            &item("pause_resume", "Pause / Resume")?,
            &item("stop", "Stop")?,
            &PredefinedMenuItem::separator(app)?,
            &item("open", "Open Speachy…")?,
            &PredefinedMenuItem::separator(app)?,
            &item("quit", "Quit Speachy")?,
        ],
    )?;
    TrayIconBuilder::with_id("speachy")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Speachy")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let app = app.clone();
            let id = event.id().as_ref().to_string();
            // Menu callbacks run on the main thread; speech actions may block.
            std::thread::spawn(move || match id.as_str() {
                "speak_selection" => shortcuts::run(&app, Action::SpeakSelection),
                "speak_clipboard" => speech::speak_clipboard(&app),
                "pause_resume" => shortcuts::run(&app, Action::PauseResume),
                "stop" => shortcuts::run(&app, Action::Stop),
                "open" => windows::show_main(&app),
                "quit" => app.exit(0),
                _ => {}
            });
        })
        .build(app)?;
    Ok(())
}
