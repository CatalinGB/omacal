//! The web server's sync ticker: one `sync_all` shortly after boot, then on the
//! same schedule the desktop uses.
//!
//! The pure decisions (`interval_ms`, `should_sync`, `SYNC_FAILED_MESSAGE`) live
//! in `omacal_view::sync_loop`, shared with `src-tauri`; this module is only the
//! tokio task and the WebSocket report.

use std::sync::Arc;
use std::time::Duration;

use omacal_view::AppState;

use crate::events::Broadcaster;

/// How often the ticker wakes to *consider* syncing, matching the desktop's.
/// Short enough that a wake-from-sleep is noticed promptly, cheap because
/// `should_sync` is pure.
const TICK: Duration = Duration::from_secs(30);

/// Starts the ticker. The first sync runs immediately — a fresh volume has never
/// synced, so `should_sync` would fire on the first tick anyway, and a
/// boot-time sync is what makes the first page load show real events.
pub fn spawn(app: Arc<AppState>, events: Broadcaster) {
    tokio::spawn(async move {
        run_once(&app, &events).await;
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            let last = omacal_view::status::last_sync_ms(&app.pool).await.ok().flatten();
            let interval = omacal_view::sync_loop::interval_ms(&app.pool).await;
            if !omacal_view::sync_loop::should_sync(false, last, omacal_view::now_ms(), interval) {
                continue;
            }
            run_once(&app, &events).await;
        }
    });
}

/// Runs one `sync_all` and reports the outcome to the WebSocket. Public so an
/// integration test can drive it without waiting on the ticker.
pub async fn run_once(app: &AppState, events: &Broadcaster) {
    // A CalDAV-only build has no Google client pair anywhere; the closure is
    // reached only if a Google account exists, and then it says so plainly.
    let result = omacal_view::sync::sync_all(app, &*app.credentials, || {
        anyhow::bail!("no Google account configured on the web build")
    })
    .await;

    match result {
        Ok(upserted) => {
            if let Err(e) = omacal_view::status::record_sync(&app.pool, omacal_view::now_ms()).await {
                tracing::warn!(%e, "sync succeeded but recording it failed");
            }
            events.send("sync-finished", serde_json::json!({ "upserted": upserted }));
        }
        // The underlying error stays in the log. The webview gets the same
        // fixed sentence the desktop sends.
        Err(e) => {
            tracing::warn!(%e, "background sync failed");
            events.send(
                "sync-failed",
                serde_json::json!({ "message": omacal_view::sync_loop::SYNC_FAILED_MESSAGE }),
            );
        }
    }
}
