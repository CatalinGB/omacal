//! The tasks commands — VTODO's face toward the UI.
//!
//! The read/write logic lives in `omacal-view::tasks`; this keeps the Tauri
//! commands and the one desktop side-effect a task write has: refreshing the
//! Omarchy widget's feed (`crate::upcoming::refresh_soon`).

use crate::AppState;

// The bodies and view types live in the service crate; re-exported so
// `crate::tasks::…` paths are unchanged.
pub use omacal_view::tasks::*;

#[tauri::command]
pub async fn list_tasks(state: tauri::State<'_, AppState>) -> Result<Vec<TaskVm>, String> {
    omacal_view::tasks::list_tasks(&state).await
}

#[tauri::command]
pub async fn set_task_completed(
    state: tauri::State<'_, AppState>,
    id: i64,
    on: bool,
) -> Result<Vec<TaskVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    omacal_view::tasks::set_completed_impl(&state, id, on)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    list_tasks(state).await
}

/// Edits a task: its title, its due date and its note, in one write.
///
/// Every field is the whole answer rather than a change to apply — the
/// editor knows the complete state, so there is no "leave this alone" to
/// get wrong, and clearing a due date is saying `None` rather than
/// omitting it.
///
/// `due_all_day` is the difference between "by Thursday" and "by Thursday
/// at 18:00", and it is the user's distinction, not a storage detail: a
/// date-only due goes on the wire as `VALUE=DATE` and an instant as a UTC
/// stamp. A due date resolves against the *calendar's* zone, the same one
/// the sync reads it back in, so a task does not move a day when the
/// display zone differs.
///
/// `calendar_id` moves the task to another list in the same save; `None`
/// (or its own list) leaves it where it is.
// A Tauri command takes flat arguments, so the whole-state bundle is wide.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_task(
    state: tauri::State<'_, AppState>,
    id: i64,
    summary: String,
    due_ms: Option<i64>,
    due_all_day: bool,
    notes: Option<String>,
    calendar_id: Option<i64>,
    // The raw stored integer the window passed back (0 = none); passing an
    // untouched value straight through keeps a server's non-canonical `7`.
    priority: i64,
) -> Result<Vec<TaskVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    omacal_view::tasks::update_impl(
        &state,
        id,
        &summary,
        due_ms,
        due_all_day,
        notes.as_deref(),
        calendar_id,
        Some(priority),
    )
    .await
    .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    list_tasks(state).await
}

/// `due_all_day` is `update_task`'s distinction: a date, or an hour on it.
/// Left out, a due is a date — the add row's "by Friday" — and a list's own
/// new line sends `false` when it was given a time.
#[tauri::command]
pub async fn create_task(
    state: tauri::State<'_, AppState>,
    calendar_id: i64,
    summary: String,
    due_ms: Option<i64>,
    due_all_day: Option<bool>,
    priority: Option<i64>,
) -> Result<Vec<TaskVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    omacal_view::tasks::create_impl(
        &state,
        calendar_id,
        &summary,
        due_ms,
        due_all_day.unwrap_or(true),
        priority.unwrap_or(0),
    )
    .await
    .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    list_tasks(state).await
}

#[tauri::command]
pub async fn delete_task_cmd(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<Vec<TaskVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    omacal_view::tasks::delete_impl(&state, id)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    list_tasks(state).await
}

/// One page of the Done list's history.
#[tauri::command]
pub async fn search_done_tasks(
    state: tauri::State<'_, AppState>,
    query: String,
    before_ms: Option<i64>,
    offset: usize,
    limit: usize,
) -> Result<DonePage, String> {
    omacal_view::tasks::done_page(&state, &query, before_ms, offset, limit)
        .await
        .map_err(|e| crate::errors::user_facing(&e))
}

/// Creates the on-this-device task list, or finds the one already there,
/// and answers with the lists as the pickers see them.
#[tauri::command]
pub async fn create_local_task_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TaskListVm>, String> {
    omacal_view::tasks::create_local_task_list(&state).await
}

/// Makes another task list on this device (2026-09-17, Plamen: lists for
/// the tasks that sync with nothing). Answers with the lists as the pickers
/// see them, the new one among them.
#[tauri::command]
pub async fn create_task_list(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<Vec<TaskListVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    let lists = omacal_view::tasks::create_list_impl(&state, &name)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    Ok(lists)
}

/// Renames a list on this device. A server's list keeps the name its server
/// gives it; Settings → Calendars can still label it locally.
#[tauri::command]
pub async fn rename_task_list(
    state: tauri::State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<Vec<TaskListVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    let lists = omacal_view::tasks::rename_list_impl(&state, id, &name)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(lists)
}

/// Deletes a list on this device and its tasks. The pane asks first, naming
/// how many tasks go with it.
#[tauri::command]
pub async fn delete_task_list(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<Vec<TaskListVm>, String> {
    crate::demo_sync_guard(state.demo)?;
    let lists = omacal_view::tasks::delete_list_impl(&state, id)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    crate::upcoming::refresh_soon(state.pool.clone(), state.demo);
    Ok(lists)
}

#[tauri::command]
pub async fn task_lists(state: tauri::State<'_, AppState>) -> Result<Vec<TaskListVm>, String> {
    omacal_view::tasks::writable_task_lists(&state.pool)
        .await
        .map_err(|e| crate::errors::user_facing(&e))
}
