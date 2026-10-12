//! Read-only WebCal subscriptions — the sync half. Subscribing (validating a
//! URL and writing the rows) stays desktop-side with its form; a sync is a
//! plain re-fetch into an existing account.

use sqlx::SqlitePool;

/// Syncs every enabled calendar of one subscribed-feeds account. One bad feed
/// never stops the others — the same per-calendar isolation Google gets.
///
/// Takes no window: a feed arrives whole, so there is no slice to ask for.
/// No `dead`/reauth concept: feeds hold no credentials, so every failure is
/// a plain `failed` label, never a reconnect banner.
pub async fn sync_account(pool: &SqlitePool, account_id: i64) -> (u64, Vec<String>) {
    let account_name: String = sqlx::query_scalar("SELECT email FROM accounts WHERE id = ?1")
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .unwrap_or_else(|| format!("webcal:{account_id}"));
    let cals: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, google_id FROM calendars WHERE account_id = ?1 AND sync_enabled = 1 ORDER BY id",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut total = 0u64;
    let mut failed = Vec::new();
    for (cal_id, feed_url) in cals {
        match omacal_sync::webcal_feed::sync_webcal_calendar(pool, cal_id, &feed_url)
            .await
        {
            Ok(out) => total += (out.upserted + out.deleted) as u64,
            Err(e) => {
                // **The host and the row id, never the address.** For a
                // subscription the URL is frequently the credential itself —
                // a private calendar's "secret address in iCal format" grants
                // read access to anyone holding it — and the failure message
                // the user reads says "see the application log", which is how
                // that address would end up pasted into a bug report.
                let host = omacal_sync::webcal_feed::default_name_for(&feed_url);
                tracing::warn!(account = %account_name, calendar_id = cal_id, %host, %e,
                               "subscribed feed sync failed");
                failed.push(format!("{account_name} / {host}"));
            }
        }
    }
    (total, failed)
}

/// Test seam for the name fallback (kept free of Tauri state).
pub fn fallback_name(feed_url: &str, name: Option<&str>) -> String {
    name.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| omacal_sync::webcal_feed::default_name_for(feed_url))
}
