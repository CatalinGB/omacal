//! The commands the web build answers with a fixed, honest shape rather than a
//! real one. Each mirrors the desktop payload's fields so the UI's existing
//! readers find what they expect; a later task fills in the live values.

use serde_json::{json, Value};
use sqlx::SqlitePool;

/// `get_status`: no accounts, no update notice. The one live value is the
/// recorded sync time, so the header's "Synced …" reads the truth. The version
/// is this build's own, the same source the update check would use.
pub async fn status(pool: &SqlitePool) -> Value {
    let last_sync_ms = omacal_view::status::last_sync_ms(pool).await.ok().flatten();
    json!({
        "accounts": [],
        "needs_reauth": [],
        "update": null,
        "version": env!("CARGO_PKG_VERSION"),
        "system_tz_change": null,
        "last_sync_ms": last_sync_ms,
        "demo": false,
        "overlay_titlebar": false,
        "self_update": false,
    })
}

/// `get_palette`: the built-in dark palette, the desktop's own fallback.
pub fn palette() -> Value {
    json!({
        "bg": "#17171a",
        "surface": "#1e1e22",
        "text": "#e8e8ea",
        "muted": "#8a8a90",
        "accent": "#5b8def",
        "is_dark": true,
    })
}

/// `get_weather`: an empty report — the UI's rule is "no days, no icons".
pub fn weather() -> Value {
    json!({
        "days": [],
        "place": null,
        "current": null,
        "source": null,
        "fetched_at": null,
    })
}
