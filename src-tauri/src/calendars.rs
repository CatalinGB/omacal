//! The calendar-list commands. The store work lives in `omacal-view::calendars`;
//! this keeps the commands, and the one desktop side-effect (the tray/menubar
//! refresh a label change triggers).

use crate::AppState;

#[tauri::command]
pub async fn get_calendars(state: tauri::State<'_, AppState>) -> Result<Vec<omacal_store::CalendarRow>, String> {
    omacal_view::calendars::list(&state.pool).await
}

#[tauri::command]
pub async fn set_calendar_selected(
    state: tauri::State<'_, AppState>,
    id: i64,
    on: bool,
) -> Result<(), String> {
    omacal_view::calendars::set_selected(&state.pool, id, on).await
}

#[tauri::command]
pub async fn set_calendar_color(
    state: tauri::State<'_, AppState>,
    id: i64,
    hex: Option<String>,
) -> Result<(), String> {
    omacal_view::calendars::set_color(&state.pool, id, hex.as_deref()).await
}

#[tauri::command]
pub async fn set_calendar_sync(
    state: tauri::State<'_, AppState>,
    id: i64,
    on: bool,
) -> Result<u64, String> {
    omacal_view::calendars::set_sync(&state.pool, id, on).await
}

#[tauri::command]
pub async fn set_calendar_label(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: i64,
    label: Option<String>,
) -> Result<(), String> {
    omacal_view::calendars::set_label(&state.pool, id, label.as_deref()).await?;
    crate::settings::refresh_menu_surfaces(&app, &state).await;
    Ok(())
}
