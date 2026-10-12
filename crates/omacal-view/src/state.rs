//! The process state both interfaces share, and the two seams through which
//! the service layer reaches the host it runs on.
//!
//! The desktop manages this state on its Tauri app; the web server will manage
//! its own with the desktop-only fields left inert. Nothing here reaches for
//! `tauri`.

use std::collections::{BTreeSet, HashMap};

use sqlx::SqlitePool;

/// Live credentials for one account, held for the process lifetime.
///
/// Deliberately has no `Debug`: a derived one would print both secrets, and a
/// leaked refresh token never expires.
#[derive(Clone)]
pub struct CachedTokens {
    pub refresh_token: String,
    pub access_token: String,
    /// When the access token stops being usable, already carrying its safety margin.
    pub expires_at_ms: i64,
}

/// A newer published release, once the daily check has found one.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UpdateNotice {
    /// The newer version, without the tag's `v` — it is displayed inside a
    /// sentence, and "v0.1.2 is available" reads like a git tag escaped.
    pub version: String,
    /// The release page, opened by `open_latest_release`. Kept backend-side
    /// so the interface never chooses what the browser opens.
    pub url: String,
}

/// The state a running instance holds.
///
/// The desktop manages this on its Tauri app. A web server constructs the
/// same struct with the desktop-only fields at their defaults and never reads
/// them — they are here rather than in a parallel type so both interfaces
/// share one `AppState` and the `#[tauri::command]` wrappers keep their
/// signatures.
pub struct AppState {
    pub pool: SqlitePool,
    /// True when running on synthetic demo data; surfaced to the interfaces via
    /// `get_status` so they can show the `DEMO DATA` badge.
    pub demo: bool,
    /// Keyed by account email.
    ///
    /// Without this, every sync re-read the Keychain and re-exchanged a refresh
    /// token that was still valid for the best part of an hour. The macOS
    /// Keychain prompts per binary, and an unsigned `cargo tauri dev` build
    /// changes hash on every rebuild — so "Always Allow" never stuck and the
    /// prompt returned every few minutes. One read per launch instead.
    pub tokens: tokio::sync::Mutex<HashMap<String, CachedTokens>>,
    /// Accounts whose refresh token the token endpoint has pronounced dead —
    /// `invalid_grant` and its siblings, the answers that never change on
    /// retry. Sync skips these instead of re-asking the provider every five
    /// minutes, and `get_status` surfaces them so the interface can offer the
    /// one thing that helps: signing in again.
    ///
    /// In memory only, deliberately: on relaunch the first sync re-discovers a
    /// dead token in one request, which is cheaper than keeping a database
    /// column truthful.
    ///
    /// A `BTreeSet` so `get_status` reports a stable order; a blocking mutex
    /// because no holder crosses an await.
    pub reauth: std::sync::Mutex<BTreeSet<String>>,

    // ---- desktop only; inert on a web server ----
    /// A newer published release, once the daily check has found one.
    pub update: std::sync::Mutex<Option<UpdateNotice>>,
    /// When the release endpoint was last asked, ms epoch; `0` for never.
    pub update_checked_at: std::sync::atomic::AtomicI64,
    /// The `quit_on_close` setting, mirrored where a synchronous handler can
    /// read it.
    pub quit_on_close: std::sync::atomic::AtomicBool,
    /// The system zone's new IANA name, once the tz watcher has seen it move
    /// out from under this process — `None` until then.
    pub system_tz_change: std::sync::Mutex<Option<String>>,
    /// The date a *fresh* launch was asked to open on, parked until the
    /// interface that can act on it exists.
    pub open_date: std::sync::Mutex<Option<String>>,
}

impl AppState {
    /// A state with only the fields the service layer uses — what a headless
    /// server builds. The desktop fills the rest via its own constructor.
    pub fn headless(pool: SqlitePool) -> Self {
        Self {
            pool,
            demo: false,
            tokens: Default::default(),
            reauth: Default::default(),
            update: Default::default(),
            update_checked_at: Default::default(),
            quit_on_close: Default::default(),
            system_tz_change: Default::default(),
            open_date: Default::default(),
        }
    }
}

/// Where the service layer's secrets live.
///
/// The desktop implements this over the OS keyring; a web server implements it
/// over an encrypted file on its volume. Keys are the same strings the desktop
/// has always used: an account email for a Google refresh token, `caldav:<user>`
/// for a CalDAV password.
pub trait CredentialStore: Send + Sync {
    /// The secret at `key`, or `None` when nothing is stored.
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    /// Stores `secret` at `key`, replacing any existing value.
    fn set(&self, key: &str, secret: &str) -> anyhow::Result<()>;
    /// Removes `key`; a missing key is not an error.
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// Where the service layer's "the store changed" (and theme/update/…) events
/// go.
///
/// The desktop implements this by emitting into the webview; a web server
/// implements it by broadcasting over its WebSocket. Event names and payloads
/// are exactly what the UI already listens for.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}
