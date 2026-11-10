//! Shared application state.

use speachy_core::engine::EngineCache;
use speachy_core::models::{ModelManager, Progress};
use speachy_core::pipeline::Speaker;
use speachy_core::settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

pub struct Download {
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<Progress>,
}

pub struct AppState {
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub models: ModelManager,
    pub engines: Arc<EngineCache>,
    pub speaker: Speaker,
    /// Installs in progress, by model id.
    pub downloads: Mutex<HashMap<String, Download>>,
    /// The selection most recently sent to the speaker by the shortcut, to
    /// tell "speak this new selection" from "stop".
    pub last_selection: Mutex<String>,
    /// While a notice is on the pill, the time until which it stays up.
    pub notice_until: Mutex<Option<Instant>>,
}
