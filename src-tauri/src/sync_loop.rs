//! Background sync ticker (spec §5): syncs on a timer, on window focus, and
//! implicitly on wake-from-sleep (a long-asleep clock makes `due` fire on the
//! next tick after waking). Demo mode never reaches Google here — both
//! `spawn` and `request_now` route their decision through `may_sync`/
//! `should_sync` rather than carrying their own untested `if demo` check.
//!
//! The pure decisions (`interval_ms`, `due`, `may_sync`, `should_sync`,
//! `focus_should_sync`, `sync_failed_payload`) moved to `omacal-view::sync_loop`
//! in Task 4 so the web server can decide when a sync is due without depending
//! on `src-tauri`. `interval_ms` is re-exported because other desktop modules
//! call it as `crate::sync_loop::interval_ms`.

use tauri::{AppHandle, Emitter, Manager};

// The pure decisions live in `omacal-view::sync_loop`, shared with the web
// server. `interval_ms` is re-exported because other desktop modules call it as
// `crate::sync_loop::interval_ms`; the rest are used only here.
pub use omacal_view::sync_loop::interval_ms;
use omacal_view::sync_loop::{focus_should_sync, may_sync, should_sync, sync_failed_payload};

/// How often the ticker wakes to *consider* syncing. Short enough that a
/// wake-from-sleep is noticed promptly, cheap because `due` is pure.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs one sync and reports the outcome to the UI. Shared by the ticker and
/// the focus path so both record the timestamp and, crucially, both *say* when
/// a sync fails — silence on this path is what let the header read "Synced 4
/// min ago" all night with the network down.
async fn sync_and_report(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    match crate::sync_all(&state).await {
        Ok(n) => {
            if let Err(e) = crate::status::record_sync(&state.pool, crate::now_ms()).await {
                tracing::warn!(%e, "sync succeeded but recording it failed");
            }
            let _ = app.emit("sync-finished", serde_json::json!({ "upserted": n }));
            crate::upcoming::refresh(&state.pool, state.demo).await;
            // The bar widget re-reads the file above by itself; the tray is
            // in this process and has to be told (spec 2026-08-29 §3).
            crate::tray::refresh(app);
            crate::invites::after_sync(app).await;
        }
        // No token yet, offline, revoked consent — all normal. Retry next tick.
        Err(e) => {
            tracing::warn!(%e, "background sync failed");
            let _ = app.emit("sync-failed", sync_failed_payload(&e));
        }
    }
}

/// Starts the background ticker. Never panics the app: a failed sync is logged
/// and retried on the next tick.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            let (pool, demo) = {
                let state = app.state::<crate::AppState>();
                (state.pool.clone(), state.demo)
            };

            let now = crate::now_ms();
            let last = crate::status::last_sync_ms(&pool).await.ok().flatten();

            if !should_sync(demo, last, now, interval_ms(&pool).await) {
                continue;
            }

            sync_and_report(&app).await;
        }
    });
}

/// Nudges the ticker to reconsider immediately — used on window focus.
///
/// Goes through the same due-check as the ticker, on a much shorter interval:
/// a focus is a request for fresh data, but it is not a licence to sync once
/// per alt-tab.
pub fn request_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (pool, demo) = {
            let state = app.state::<crate::AppState>();
            (state.pool.clone(), state.demo)
        };
        if !may_sync(demo) {
            return;
        }

        let last = crate::status::last_sync_ms(&pool).await.ok().flatten();
        if !focus_should_sync(demo, last, crate::now_ms()) {
            return;
        }

        sync_and_report(&app).await;
    });
}
