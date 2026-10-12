//! Re-export of the assemblers moved to `omacal-view`, kept so every
//! `crate::commands::…` call site is unchanged. `open_conference` stays here:
//! it resolves an event's meeting link and opens it with the *desktop's*
//! browser, so it has no meaning without a host to open a browser on.

pub use omacal_view::commands::*;

/// Opens an event's meeting link with its preferred application — the Join
/// control, moved backend-side.
///
/// The webview's `<a target="_blank">` used to carry this click, which
/// routed it through the opener plugin's raw spawn — inside an AppImage,
/// the environment-poisoning path of issue #1, and a browser that crashes
/// on launch. Now the UI sends the event *id* and this resolves the URL
/// itself: `open_latest_release`'s rule, for the same reason — the webview
/// never chooses what the browser is pointed at, it can only name an event
/// the user can already see.
///
/// The URL comes from the same places the popover's own derivation reads,
/// in the same order: structured conference data first, then a recognised
/// meeting link in `location`, then one in `description`
/// (`conference_join_url` — the widget feed's helper, so this, the feed and
/// the CLI all agree on what is joinable).
#[tauri::command]
pub(crate) async fn open_conference(
    state: tauri::State<'_, crate::AppState>,
    id: i64,
) -> Result<(), String> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT conference_uri, location, description FROM events WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| crate::errors::user_facing(&e.into()))?;
    let Some((conference, location, description)) = row else {
        return Err("This event is no longer on the calendar.".into());
    };
    let url = conference
        .or_else(|| crate::upcoming::conference_join_url(location.as_deref(), description.as_deref()))
        .ok_or_else(|| "This event has no meeting link.".to_string())?;
    crate::browser::open_external(&url).map_err(|e| {
        tracing::warn!(%e, "could not open the meeting link");
        crate::BROWSER_FAILED.to_string()
    })
}
