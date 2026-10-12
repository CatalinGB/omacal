//! Sync orchestration: every calendar of every account of every provider, one
//! pass. Provider-agnostic — the Google, CalDAV and WebCal engines all live
//! behind it.

use sqlx::SqlitePool;

use crate::state::{AppState, CredentialStore};
use crate::{auth, caldav_account, webcal};

/// Google's Calendar API root. A constant so the one place that overrides it —
/// a test pointing the sync at a local mock — is the only place a different
/// value can come from.
pub const GOOGLE_CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

/// The accounts a sync should even attempt: everyone not marked as needing
/// re-consent.
pub fn accounts_to_attempt(
    accounts: Vec<(i64, String)>,
    marked: &std::collections::BTreeSet<String>,
) -> Vec<(i64, String)> {
    accounts.into_iter().filter(|(_, email)| !marked.contains(email)).collect()
}

/// Turns what a sync managed to do into what it reports.
///
/// Partial success is still failure: rows written by the accounts that worked
/// do not make the account that did not any less stale. A count, never the
/// labels — the labels are account email addresses.
pub fn sync_result(total: u64, failed: &[String]) -> anyhow::Result<u64> {
    if !failed.is_empty() {
        anyhow::bail!("{} of this sync's targets failed; see the log", failed.len());
    }
    Ok(total)
}

/// The calendars a sync should fetch for one account.
///
/// `sync_enabled`, deliberately — not `selected`. Hiding a calendar in the UI
/// must not stop it syncing, or re-showing it would reveal a gap until the
/// next full sync.
pub async fn calendars_to_sync(
    pool: &SqlitePool,
    account_id: i64,
) -> anyhow::Result<Vec<(i64, String)>> {
    let cals = sqlx::query_as(
        "SELECT id, google_id FROM calendars
         WHERE account_id = ?1 AND sync_enabled = 1
         ORDER BY id",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(cals)
}

/// Syncs every calendar of every Google account, isolating failures.
pub async fn sync_accounts<F, Fut>(
    pool: &SqlitePool,
    accounts: &[(i64, String)],
    api_base: &str,
    window_start_ms: i64,
    window_end_ms: i64,
    token_for: F,
) -> (u64, Vec<String>, Vec<String>)
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    let mut total = 0u64;
    let mut failed: Vec<String> = Vec::new();
    let mut dead: Vec<String> = Vec::new();

    for (account_id, email) in accounts {
        // `%e`, never the token: `Tokens` has a hand-written redacting `Debug`
        // and no credential is interpolated anywhere on this path.
        let access_token = match token_for(email.clone()).await {
            Ok(t) => t,
            Err(e) if omacal_google::auth::needs_reauth(&e) => {
                tracing::warn!(account = %email, %e,
                    "refresh token is dead; the account needs to be reconnected");
                dead.push(email.clone());
                continue;
            }
            Err(e) => {
                tracing::warn!(account = %email, %e, "no usable access token; skipping this account");
                failed.push(email.clone());
                continue;
            }
        };

        let client = omacal_google::CalendarClient::new(api_base, &access_token);
        // Names can change without reconnecting an account. Update only known
        // calendars: this must not import or re-enable calendars during sync.
        if let Err(e) = refresh_calendar_names(pool, *account_id, &client).await {
            tracing::warn!(%e, "could not refresh calendar names; keeping cached names");
        }

        let cals = match calendars_to_sync(pool, *account_id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(account = %email, %e, "could not list calendars; skipping this account");
                failed.push(email.clone());
                continue;
            }
        };

        for (cal_id, google_id) in cals {
            match omacal_sync::sync_calendar(
                pool, &client, cal_id, &google_id, window_start_ms, window_end_ms,
            )
            .await
            {
                Ok(out) => total += (out.upserted + out.deleted) as u64,
                Err(e) => {
                    tracing::warn!(account = %email, calendar = %google_id, %e, "calendar sync failed");
                    failed.push(format!("{email} / {google_id}"));
                }
            }
        }
    }

    (total, failed, dead)
}

pub async fn refresh_calendar_names(
    pool: &SqlitePool, account_id: i64, client: &omacal_google::CalendarClient,
) -> anyhow::Result<()> {
    let calendars = client.list_calendars().await?;
    let mut names = Vec::new();
    for c in calendars {
        let mut name = c.display_name().to_string();
        if name == c.id && c.primary && c.summary_override.as_deref().is_none_or(|s| s.trim().is_empty()) {
            if let Ok(title) = client.calendar_title(&c.id).await {
                if !title.trim().is_empty() { name = title; }
            }
        }
        names.push((c.id, name));
    }
    let mut tx = pool.begin().await?;
    for (id, name) in names {
        sqlx::query("UPDATE calendars SET summary = ?3 WHERE account_id = ?1 AND google_id = ?2")
            .bind(account_id).bind(id).bind(name)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Refreshes the access token and syncs every calendar of every account.
///
/// Pure sync work, with no demo check and no status bookkeeping of its own —
/// shared by the `sync_now` command and the background loop, each of which
/// handles the demo gate and `record_sync` itself.
///
/// `google_cfg` is called only when a Google account exists, so a CalDAV-only
/// install never needs a Google client pair. It returns `(client_id,
/// client_secret)`.
pub async fn sync_all(
    state: &AppState,
    store: &dyn CredentialStore,
    google_cfg: impl Fn() -> anyhow::Result<(String, String)>,
) -> anyhow::Result<u64> {
    let pool = &state.pool;
    // `ORDER BY id`, so which account goes first is a decision rather than
    // whatever rowid order happens to be — it used to decide whose failure
    // stopped the rest.
    // (id, email, provider, server_url, username)
    type AccountRow = (i64, String, String, Option<String>, Option<String>);
    let all: Vec<AccountRow> = sqlx::query_as(
        "SELECT id, email, provider, server_url, username FROM accounts ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    let marked = state.reauth.lock().expect("reauth mark poisoned").clone();
    let google: Vec<(i64, String)> = all
        .iter()
        .filter(|(_, _, p, _, _)| p == "google")
        .map(|(id, email, _, _, _)| (*id, email.clone()))
        .collect();
    let google = accounts_to_attempt(google, &marked);

    let now = crate::now_ms();
    let (window_start, window_end) = crate::synced_window(now);

    let mut total = 0u64;
    let mut failed: Vec<String> = Vec::new();
    let mut dead: Vec<String> = Vec::new();

    // `google_cfg` is called inside the Google branch: a CalDAV-only install
    // has no Google client pair anywhere and must not need one to sync.
    if !google.is_empty() {
        let (client_id, client_secret) = google_cfg()?;
        let (t, f, d) = sync_accounts(
            pool,
            &google,
            GOOGLE_CALENDAR_API,
            window_start,
            window_end,
            move |email| {
                let client_id = client_id.clone();
                let client_secret = client_secret.clone();
                async move {
                    auth::access_token_for(state, store, &client_id, &client_secret, &email).await
                }
            },
        )
        .await;
        total += t;
        failed.extend(f);
        dead.extend(d);
    }

    for (account_id, _, _, _, _) in all.iter().filter(|(_, _, p, _, _)| p == "webcal") {
        let (t, f) = webcal::sync_account(pool, *account_id).await;
        total += t;
        failed.extend(f);
    }

    for (account_id, email, _, server_url, username) in
        all.iter().filter(|(_, _, p, _, _)| p == "caldav")
    {
        if marked.contains(email) {
            continue; // dead credentials; the banner is already up
        }
        let client =
            match caldav_account::client_for(store, email, server_url.as_deref(), username.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(account = %email, %e, "no usable CalDAV credentials");
                    failed.push(email.clone());
                    continue;
                }
            };
        match caldav_account::sync_account(pool, *account_id, &client, window_start, window_end)
            .await
        {
            Ok(n) => total += n,
            Err(e) if e.needs_reauth() => {
                tracing::warn!(account = %email,
                    "the CalDAV password was rejected; the account needs to be reconnected");
                dead.push(email.clone());
            }
            Err(e) => {
                tracing::warn!(account = %email, %e, "CalDAV sync failed");
                failed.push(email.clone());
            }
        }
    }

    if !dead.is_empty() {
        state.reauth.lock().expect("reauth mark poisoned").extend(dead);
    }

    sync_result(total, &failed)
}
