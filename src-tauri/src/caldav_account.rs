//! Connecting CalDAV accounts — the connect *command*, desktop-side.
//!
//! The sync engine it shares with a headless server (building the client,
//! walking the calendars, refreshing contacts) lives in
//! `omacal-view::caldav_account`. This file keeps the one command that opens a
//! form, validates a password the user typed, and writes the account — plus the
//! keyring wiring that command needs.

use omacal_caldav::{CalDavClient, CalDavError};

use crate::AppState;
use omacal_view::CredentialStore;

// Shared with the sync engine; re-exported so `crate::caldav_account::…`
// paths are unchanged.
pub(crate) use omacal_view::caldav_account::credential_key;

/// What `connect_caldav` needs to know about the kind of server it is
/// talking to. "icloud" is a UI nicety, not a protocol difference: it fixes
/// the URL and words the form ("Apple ID", "app-specific password").
fn base_url_for(kind: &str, server_url: Option<&str>) -> Result<String, String> {
    match kind {
        "icloud" => Ok(omacal_caldav::ICLOUD_BASE.to_string()),
        _ => server_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "A server address is required".to_string()),
    }
}

/// The client (and collection URL) behind one CalDAV calendar, with the
/// read-only refusal every write path shares. Desktop wrapper injecting the
/// keyring store; the walk itself is in `omacal-view::caldav_account`.
pub(crate) async fn client_for_calendar(
    state: &AppState,
    calendar_id: i64,
) -> anyhow::Result<(CalDavClient, String)> {
    omacal_view::caldav_account::client_for_calendar(&crate::KeyringStore, state, calendar_id).await
}

/// Connects (or reconnects) a CalDAV account: validates the credentials by
/// running discovery, then stores the password, the account row, and the
/// calendar list. Returns the account's display email.
#[tauri::command]
pub async fn connect_caldav(
    state: tauri::State<'_, AppState>,
    kind: String,
    server_url: Option<String>,
    email: String,
    username: Option<String>,
    password: String,
) -> Result<String, String> {
    crate::demo_sync_guard(state.demo)?;
    let email = email.trim().to_string();
    if email.is_empty() {
        return Err("An email (or account name) is required".to_string());
    }
    if password.trim().is_empty() {
        return Err("A password is required".to_string());
    }
    let base = base_url_for(&kind, server_url.as_deref())?;
    let login = username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&email)
        .to_string();

    let client = CalDavClient::new(&base, &login, &password)
        .map_err(|e| crate::errors::user_facing(&e))?;
    // Discovery is the credential check: a wrong password dies here, before
    // anything at all is written.
    let calendars = client.discover().await.map_err(user_facing_caldav)?;
    if calendars.is_empty() {
        return Err("The server answered, but listed no calendars for this account".to_string());
    }

    crate::KeyringStore
        .set(&credential_key(&email), &password)
        .map_err(|e| crate::errors::user_facing(&e))?;

    let pool = &state.pool;
    let account_id: i64 = sqlx::query_scalar(
        "INSERT INTO accounts (google_sub, email, created_at, provider, server_url, username)
         VALUES (?1, ?2, ?3, 'caldav', ?4, ?5)
         ON CONFLICT (google_sub) DO UPDATE SET
            email = excluded.email, server_url = excluded.server_url,
            username = excluded.username
         RETURNING id",
    )
    .bind(credential_key(&email))
    .bind(&email)
    .bind(crate::now_ms())
    .bind(&base)
    .bind(&login)
    .fetch_one(pool)
    .await
    .map_err(|e| crate::errors::user_facing(&anyhow::Error::from(e)))?;

    // The machine's zone is the fallback for collections that publish none —
    // an all-day date has to resolve against something, and the zone the
    // user is sitting in is the only defensible guess.
    let fallback_tz = jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_string();

    for cal in &calendars {
        // Like the Google upsert: the provider's fields update, the user's
        // (`selected`, `sync_enabled`) are never touched on reconnect.
        sqlx::query(
            "INSERT INTO calendars
                (account_id, google_id, summary, color_hex, timezone, access_role,
                 is_primary, supports_events, supports_tasks)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)
             ON CONFLICT (account_id, google_id) DO UPDATE SET
                summary = excluded.summary,
                color_hex = excluded.color_hex,
                timezone = excluded.timezone,
                access_role = excluded.access_role,
                supports_events = excluded.supports_events,
                supports_tasks = excluded.supports_tasks",
        )
        .bind(account_id)
        .bind(&cal.url)
        .bind(&cal.display_name)
        .bind(&cal.color)
        .bind(cal.timezone.as_deref().unwrap_or(&fallback_tz))
        .bind(if cal.read_only { "reader" } else { "owner" })
        .bind(cal.supports_events as i64)
        .bind(cal.supports_tasks as i64)
        .execute(pool)
        .await
        .map_err(|e| crate::errors::user_facing(&anyhow::Error::from(e)))?;
    }

    // A reconnect clears the account's dead-credential mark, same as the
    // Google path.
    state.reauth.lock().expect("reauth mark poisoned").remove(&email);
    Ok(email)
}

/// Maps a CalDAV error to something the connect form can show.
fn user_facing_caldav(e: CalDavError) -> String {
    match e {
        CalDavError::Unauthorized => {
            "The server rejected the credentials. For iCloud, use an app-specific password from appleid.apple.com, not your Apple ID password.".to_string()
        }
        CalDavError::PreconditionFailed => e.to_string(),
        CalDavError::Http(status) => crate::errors::server_answered(status),
        CalDavError::Other(e) => crate::errors::user_facing(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_key_cannot_collide_with_googles() {
        // Google stores under the bare email; the same address as an Apple ID
        // must land somewhere else or connecting one overwrites the other.
        assert_eq!(credential_key("p@x.com"), "caldav:p@x.com");
        assert_ne!(credential_key("p@x.com"), "p@x.com");
    }

    #[test]
    fn icloud_needs_no_url_and_generic_requires_one() {
        assert_eq!(base_url_for("icloud", None).unwrap(), omacal_caldav::ICLOUD_BASE);
        assert!(base_url_for("caldav", None).is_err());
        assert!(base_url_for("caldav", Some("  ")).is_err());
        assert_eq!(
            base_url_for("caldav", Some(" https://dav.example.com/ ")).unwrap(),
            "https://dav.example.com/"
        );
    }
}
