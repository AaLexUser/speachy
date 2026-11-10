//! What the shell keeps beside the engine session.

use speachy_core::session::Session;
use std::sync::Mutex;
use std::time::Instant;

pub struct AppState {
    /// The engine. Everything that is not user interface lives behind it.
    pub session: Session,
    /// The selection most recently sent to the engine by the shortcut, to
    /// tell "speak this new selection" from "stop".
    pub last_selection: Mutex<String>,
    /// While a notice is on the pill, the time until which it stays up.
    pub notice_until: Mutex<Option<Instant>>,
}
