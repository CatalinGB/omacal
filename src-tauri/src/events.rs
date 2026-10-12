//! The events commands — the Tauri wrappers around the service layer.
//!
//! The read/write logic lives in `omacal-view::events`; this keeps the Tauri
//! commands and the one desktop side-effect a write has: refreshing the
//! Omarchy widget's feed (`crate::upcoming::refresh_soon`).

use crate::AppState;

// The bodies and view types live in the service crate; re-exported so
// `crate::events::…` paths are unchanged.
pub use omacal_view::events::*;

#[tauri::command]
pub async fn event_detail(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<EventDetail, String> {
    event_detail_impl(&state, id).await.map_err(|e| crate::errors::user_facing(&e))
}

#[tauri::command]
pub async fn respond_to_event(
    state: tauri::State<'_, AppState>,
    id: i64,
    response: String,
    scope: String,
    occurrence_start_ms: i64,
) -> Result<EventDetail, String> {
    let detail = respond_event_body(&state, id, &response, &scope, occurrence_start_ms).await?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(detail)
}

#[tauri::command]
pub async fn refresh_event(state: tauri::State<'_, AppState>, id: i64) -> Result<EventDetail, String> {
    let detail = refresh_event_impl(&state, id).await?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(detail)
}

#[tauri::command]
pub async fn create_event(
    state: tauri::State<'_, AppState>,
    calendar_id: i64,
    fields: crate::write::EventInput,
    send_updates: String,
) -> Result<EventDetail, String> {
    let detail = create_event_body(&state, calendar_id, fields, &send_updates).await?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(detail)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_event(
    state: tauri::State<'_, AppState>,
    id: i64,
    scope: String,
    occurrence_start_ms: i64,
    fields: crate::write::EventInput,
    send_updates: String,
    target_calendar_id: Option<i64>,
) -> Result<EventDetail, String> {
    let detail = update_event_body(
        &state,
        id,
        &scope,
        occurrence_start_ms,
        fields,
        &send_updates,
        target_calendar_id,
    )
    .await?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(detail)
}

#[tauri::command]
pub async fn delete_event_cmd(
    state: tauri::State<'_, AppState>,
    id: i64,
    scope: String,
    occurrence_start_ms: i64,
) -> Result<(), String> {
    delete_event_body(&state, id, &scope, occurrence_start_ms).await?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(())
}

#[tauri::command]
pub(crate) async fn known_guests(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<KnownGuestRow>, String> {
    guest_suggestions(&state.pool).await
}
