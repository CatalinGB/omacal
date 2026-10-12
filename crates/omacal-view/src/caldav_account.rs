//! The CalDAV sync engine — the half a headless server needs.
//!
//! The connect *command* stays desktop-side (it opens a form and validates a
//! password the user typed); what is here is everything a sync does with an
//! account that already exists: build the client from the stored password,
//! walk its calendars, refresh its address books. The password arrives through
//! [`CredentialStore`], so this has no opinion about where it is kept.

use omacal_caldav::{CalDavClient, CalDavError};
use sqlx::SqlitePool;

use crate::state::{AppState, CredentialStore};

/// The credential key for a CalDAV secret. Prefixed so the same address used
/// as both a Google account and an Apple ID cannot collide on one entry.
pub fn credential_key(email: &str) -> String {
    format!("caldav:{email}")
}

fn password_for(store: &dyn CredentialStore, email: &str) -> anyhow::Result<String> {
    store
        .get(&credential_key(email))?
        .ok_or_else(|| anyhow::anyhow!("no stored CalDAV password for {email}"))
}

/// Builds the client for a stored account, reading its password from `store`.
pub fn client_for(
    store: &dyn CredentialStore,
    email: &str,
    server_url: Option<&str>,
    username: Option<&str>,
) -> anyhow::Result<CalDavClient> {
    let base = server_url.ok_or_else(|| anyhow::anyhow!("account has no server url"))?;
    let login = username.filter(|s| !s.is_empty()).unwrap_or(email);
    let password = password_for(store, email)?;
    CalDavClient::new(base, login, &password)
}

/// The read-only refusal every CalDAV write path shares.
pub const CALENDAR_IS_READ_ONLY: &str = "this calendar is read-only";

/// The client (and collection URL) behind one CalDAV calendar, with the
/// read-only refusal. Errors when the calendar does not belong to a CalDAV
/// account at all.
pub async fn client_for_calendar(
    store: &dyn CredentialStore,
    state: &AppState,
    calendar_id: i64,
) -> anyhow::Result<(CalDavClient, String)> {
    type Row = (String, Option<String>, Option<String>, String, String);
    let (email, server_url, username, access_role, collection_url): Row = sqlx::query_as(
        "SELECT a.email, a.server_url, a.username, c.access_role, c.google_id
         FROM calendars c JOIN accounts a ON a.id = c.account_id
         WHERE c.id = ?1 AND a.provider = 'caldav'",
    )
    .bind(calendar_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("that calendar is not on a CalDAV account"))?;
    if access_role == "reader" {
        anyhow::bail!(CALENDAR_IS_READ_ONLY);
    }
    let client = client_for(store, &email, server_url.as_deref(), username.as_deref())?;
    Ok((client, collection_url))
}

/// Syncs every enabled calendar of one CalDAV account. Returns rows written;
/// an `Unauthorized` anywhere marks the whole account dead (one password
/// covers every collection).
pub async fn sync_account(
    pool: &SqlitePool,
    account_id: i64,
    client: &CalDavClient,
    window_start_ms: i64,
    window_end_ms: i64,
) -> Result<u64, CalDavError> {
    let cals: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id, google_id, supports_events, supports_tasks FROM calendars
         WHERE account_id = ?1 AND sync_enabled = 1 ORDER BY id",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .map_err(anyhow::Error::from)?;

    sync_contacts(pool, account_id, client).await;

    let mut total = 0u64;
    for (cal_id, url, ev, tasks) in cals {
        let outcome = omacal_sync::caldav::sync_caldav_calendar(
            pool,
            client,
            cal_id,
            &url,
            ev != 0,
            tasks != 0,
            window_start_ms,
            window_end_ms,
            crate::now_ms(),
        )
        .await?;
        total += outcome.upserted as u64;
    }
    Ok(total)
}

/// How stale the stored contacts may be before an account's address books
/// are read again (#126). An hour.
const CONTACTS_MAX_AGE_MS: i64 = 60 * 60 * 1000;

/// Refreshes one account's contacts, for the attendee field's suggestions.
///
/// **Never fails the sync.** The calendars are what this account exists for,
/// and every path here — a server with no CardDAV, a REPORT it refuses, a
/// book that errors — leaves the previous contacts alone and says so in the
/// log. A partial read is not stored either: `replace_account_contacts`
/// replaces the whole set, so half an address book would read as the user
/// having deleted the other half.
async fn sync_contacts(pool: &SqlitePool, account_id: i64, client: &CalDavClient) {
    let last: Option<i64> =
        sqlx::query_scalar("SELECT MAX(updated_at) FROM contacts WHERE account_id = ?1")
            .bind(account_id)
            .fetch_one(pool)
            .await
            .unwrap_or(None);
    let now = crate::now_ms();
    if last.is_some_and(|t| now - t < CONTACTS_MAX_AGE_MS) {
        return;
    }

    let books = match client.discover_address_books().await {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(%e, account_id, "no address books read; contacts left as they were");
            return;
        }
    };
    if books.is_empty() {
        return;
    }

    let mut rows: Vec<omacal_store::StoredContact> = Vec::new();
    for book in &books {
        match client.cards(&book.url).await {
            Ok(cards) => {
                for card in cards {
                    for contact in omacal_caldav::parse_cards(&card.ics) {
                        for email in contact.emails {
                            rows.push(omacal_store::StoredContact {
                                href: card.url.clone(),
                                uid: contact.uid.clone(),
                                display_name: contact.name.clone(),
                                email,
                            });
                        }
                    }
                }
            }
            Err(e) => {
                // One unreadable book, and the stored set stays whole.
                tracing::debug!(%e, book = %book.url, "address book not read; contacts unchanged");
                return;
            }
        }
    }

    match omacal_store::replace_account_contacts(pool, account_id, &rows, now).await {
        Ok(n) => tracing::debug!(account_id, contacts = n, books = books.len(), "contacts refreshed"),
        Err(e) => tracing::warn!(%e, account_id, "contacts not stored"),
    }
}
