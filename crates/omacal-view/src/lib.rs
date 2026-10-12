//! The Tauri-free service layer.
//!
//! `src-tauri` re-exports what it exposed before, so the desktop app is
//! unchanged; `omacal-server` depends on this directly. Nothing in this crate
//! may reach for `tauri`.

pub mod auth;
pub mod caldav_account;
pub mod caldav_write;
pub mod calendars;
pub mod commands;
pub mod combined;
pub mod errors;
pub mod events;
pub mod messages;
/// Test-only: the golden-file mechanism the UI fixtures read. Gated so nothing
/// that reads or writes `ui/tests/` is compiled into the shipped lib.
#[cfg(test)]
pub mod golden;
pub mod meeting;
pub mod settings;
pub mod state;
pub mod sync;
pub mod tasks;
pub mod view;
pub mod webcal;
pub mod write;

pub use state::{
    AppState, CachedTokens, CredentialStore, EventSink, GoogleConfig, NoGoogleConfig, NoopStore,
    UpdateNotice,
};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The window a sync fetches: decisions ("what should I ask the provider for?")
/// must read one definition.
pub fn synced_window(now_ms: i64) -> (i64, i64) {
    const DAY: i64 = 3_600_000 * 24;
    (now_ms - 180 * DAY, now_ms + 365 * DAY)
}

/// What every provider-reaching command returns in demo mode, before any I/O.
pub const DEMO_SYNC_MESSAGE: &str =
    "Demo mode — this is synthetic data, so there is nothing to connect or sync.";

/// `Err` when `demo` is true, `Ok` otherwise. A plain function of the flag —
/// no config or keyring I/O anywhere near it — so callers that check it first
/// cannot reach that I/O in demo mode.
pub fn demo_sync_guard(demo: bool) -> Result<(), String> {
    if demo {
        Err(DEMO_SYNC_MESSAGE.to_string())
    } else {
        Ok(())
    }
}
