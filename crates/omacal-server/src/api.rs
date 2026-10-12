//! The routes: a health probe, one command endpoint, a WebSocket and the
//! static build.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use omacal_view::AppState;

use crate::events::Broadcaster;
use crate::stubs;

/// What an unrecognised command answers with: Tauri rejects with the Rust
/// `Err`'s `Display`, so the body is that rejection as a sentence the UI can
/// show.
const REFUSED: &str = "{\"code\":\"refused\",\"message\":\"not available in the web build\"}";

/// The plain sentence the deliberately deferred commands answer with — account
/// management, `.ics` import/export, updates/restart and the desktop menu
/// surfaces, all out of the web build's scope. Plain text, not the JSON above:
/// the shim throws the body verbatim, and a sentence is what a banner shows.
const NOT_IN_WEB: &str = "not available in the web build";

/// What every handler sees: the shared service state plus the event channel the
/// WebSocket subscribes to.
#[derive(Clone)]
pub struct ServerState {
    pub state: Arc<AppState>,
    pub events: Broadcaster,
}

pub fn router(state: Arc<AppState>, events: Broadcaster, ui_dir: PathBuf) -> Router {
    let server = ServerState { state, events };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/{cmd}", post(api))
        .route("/ws", get(ws))
        .fallback_service(
            tower_http::services::ServeDir::new(ui_dir).append_index_html_on_directories(true),
        )
        .with_state(server)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// `GET /ws`: upgrades to a WebSocket and forwards every broadcast frame.
async fn ws(upgrade: WebSocketUpgrade, State(server): State<ServerState>) -> Response {
    upgrade.on_upgrade(move |socket| forward(socket, server.events.subscribe()))
}

/// Sends each broadcast frame, and drops the connection when the client closes.
/// A lagging client is skipped rather than allowed to block the ticker.
async fn forward(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    loop {
        tokio::select! {
            broadcast = rx.recv() => match broadcast {
                Ok(frame) => {
                    if socket.send(Message::Text(frame.into())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            },
        }
    }
}

async fn api(State(server): State<ServerState>, Path(cmd): Path<String>, body: Bytes) -> Response {
    let args = match parse_args(&body) {
        Ok(v) => v,
        Err(e) => return bad(e).into_response(),
    };
    match dispatch(&server.state, &cmd, &args).await {
        Ok(value) => {
            // A write that landed changed the store, so every open page
            // reloads — the same `{"upserted": 1}` the desktop emits after a
            // successful write (`src-tauri/src/ipc.rs`), and the same signal
            // the sync ticker sends.
            if is_write(&cmd) {
                server.events.send("sync-finished", json!({ "upserted": 1 }));
            }
            Json(value).into_response()
        }
        Err((status, message)) => (status, message).into_response(),
    }
}

/// Every command that changed the store: a success changed the calendar, so
/// every open page reloads. The event mutations arrived with Task 5; the task,
/// calendar-list and settings writes with Task 6.
fn is_write(cmd: &str) -> bool {
    matches!(
        cmd,
        "create_event" | "update_event" | "delete_event_cmd" | "respond_to_event"
            | "create_task" | "update_task" | "delete_task_cmd" | "set_task_completed"
            | "create_local_task_list" | "create_task_list" | "rename_task_list"
            | "delete_task_list"
            | "set_calendar_selected" | "set_calendar_sync" | "set_calendar_color"
            | "set_calendar_label"
            | "set_setting" | "set_weather_location" | "set_display_timezone"
    )
}

/// The invoke payload, `{}` when the body is absent or JSON `null` — the shim
/// sends `JSON.stringify(args ?? {})`, so a body-less call is an empty object.
fn parse_args(body: &[u8]) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Ok(if value.is_null() { json!({}) } else { value })
}

type ApiResult = Result<Value, (StatusCode, String)>;

fn bad(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}

fn refused() -> (StatusCode, String) {
    (StatusCode::NOT_IMPLEMENTED, REFUSED.to_string())
}

/// A deliberate refusal of a command the web build does not carry — a 4xx, so
/// the shim's `.catch` paths show the sentence rather than treating it as a
/// server fault.
fn deferred() -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, NOT_IN_WEB.to_string())
}

fn to_value<T: Serialize>(value: T) -> ApiResult {
    serde_json::to_value(value).map_err(|e| bad(e.to_string()))
}

fn result<T: Serialize>(value: Result<T, String>) -> ApiResult {
    match value {
        Ok(v) => to_value(v),
        Err(e) => Err(bad(e)),
    }
}

/// A store write whose desktop command answers `()`. The UI's `invoke<void>`
/// reads any body; `null` is the honest one.
fn result_unit(value: Result<(), String>) -> ApiResult {
    match value {
        Ok(()) => Ok(Value::Null),
        Err(e) => Err(bad(e)),
    }
}

/// Tauri maps a command's snake_case Rust parameter to a camelCase JS key, so
/// the UI sends `weekStartMs` where the Rust signature says `week_start_ms`.
/// Accept either spelling.
fn camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for ch in snake.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn lookup<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    args.get(key).or_else(|| args.get(camel(key)))
}

fn req_i64(args: &Value, key: &str) -> Result<i64, (StatusCode, String)> {
    lookup(args, key)
        .and_then(Value::as_i64)
        .ok_or_else(|| bad(format!("missing or invalid `{key}`")))
}

fn req_i32(args: &Value, key: &str) -> Result<i32, (StatusCode, String)> {
    i32::try_from(req_i64(args, key)?).map_err(|_| bad(format!("`{key}` is out of range")))
}

fn req_u32(args: &Value, key: &str) -> Result<u32, (StatusCode, String)> {
    u32::try_from(req_i64(args, key)?).map_err(|_| bad(format!("`{key}` is out of range")))
}

fn req_u8(args: &Value, key: &str) -> Result<u8, (StatusCode, String)> {
    u8::try_from(req_i64(args, key)?).map_err(|_| bad(format!("`{key}` is out of range")))
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, (StatusCode, String)> {
    lookup(args, key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("missing or invalid `{key}`")))
}

fn req_bool(args: &Value, key: &str) -> Result<bool, (StatusCode, String)> {
    lookup(args, key)
        .and_then(Value::as_bool)
        .ok_or_else(|| bad(format!("missing or invalid `{key}`")))
}

/// An optional boolean argument: absent, `null`, or the boolean itself.
fn opt_bool(args: &Value, key: &str) -> Result<Option<bool>, (StatusCode, String)> {
    match lookup(args, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("`{key}` must be true or false"))),
    }
}

/// An optional string argument: absent, `null`, or the string itself. Used for
/// the clearable overrides (`hex`, `label`, `notes`, `tz`, `name`) — a `null`
/// is "clear this", which is a real state and not a missing argument.
fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, (StatusCode, String)> {
    match lookup(args, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(bad(format!("`{key}` must be a string"))),
    }
}

/// An optional integer argument: absent, `null`, or the number itself.
fn opt_i64(args: &Value, key: &str) -> Result<Option<i64>, (StatusCode, String)> {
    match lookup(args, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| bad(format!("`{key}` is out of range"))),
        Some(_) => Err(bad(format!("`{key}` must be a number"))),
    }
}

fn req_usize(args: &Value, key: &str) -> Result<usize, (StatusCode, String)> {
    usize::try_from(req_i64(args, key)?).map_err(|_| bad(format!("`{key}` is out of range")))
}

/// The `fields` object of `create_event`/`update_event`, deserialized through
/// the same `write::EventInput` the desktop command takes, so an unreadable
/// payload is refused exactly once.
fn req_fields(
    args: &Value,
    key: &str,
) -> Result<omacal_view::write::EventInput, (StatusCode, String)> {
    let value = lookup(args, key).ok_or_else(|| bad(format!("missing `{key}`")))?;
    serde_json::from_value(value.clone()).map_err(|e| bad(e.to_string()))
}

fn pad(args: &Value) -> Result<usize, (StatusCode, String)> {
    let raw = match lookup(args, "pad") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            u8::try_from(
                v.as_u64()
                    .ok_or_else(|| bad("`pad` must be a non-negative number"))?,
            )
            .map_err(|_| bad("padding is at most 14 days"))?,
        ),
    };
    omacal_view::view::checked_pad(raw).map_err(bad)
}

async fn dispatch(state: &AppState, cmd: &str, args: &Value) -> ApiResult {
    match cmd {
        "get_settings" => to_value(omacal_view::settings::read_settings(&state.pool).await),
        "list_timezones" => to_value(omacal_view::settings::list_timezones()),
        "set_setting" => {
            // The form sends `{ setting: { key, value } }`; the `Setting` enum's
            // own serde reads it, so the wire vocabulary is the desktop's.
            let value = lookup(args, "setting").ok_or_else(|| bad("missing `setting`"))?;
            let setting: omacal_view::settings::Setting = serde_json::from_value(value.clone())
                .map_err(|e| bad(e.to_string()))?;
            if let Err(e) = omacal_view::settings::store_setting(&state.pool, &setting).await {
                return Err(bad(e.shown()));
            }
            to_value(omacal_view::settings::read_settings(&state.pool).await)
        }
        "set_weather_location" => {
            let name = opt_str(args, "name")?;
            match omacal_view::weather::set_weather_location(&state.pool, name).await {
                Ok(settings) => to_value(settings),
                Err(e) => Err(bad(e)),
            }
        }
        "set_display_timezone" => {
            let tz = opt_str(args, "tz")?;
            result_unit(omacal_view::settings::set_display_timezone(&state.pool, tz).await)
        }
        "get_calendars" => match omacal_view::calendars::list(&state.pool).await {
            Ok(v) => to_value(v),
            Err(e) => Err(bad(e)),
        },
        "get_week" => {
            let start = req_i64(args, "week_start_ms")?;
            let pad = pad(args)?;
            result(omacal_view::view::get_days(&state.pool, start, 7, pad).await)
        }
        "get_day" => {
            let start = req_i64(args, "day_start_ms")?;
            let pad = pad(args)?;
            result(omacal_view::view::get_days(&state.pool, start, 1, pad).await)
        }
        "get_range" => {
            let start = req_i64(args, "day_start_ms")?;
            let count = req_u8(args, "day_count")?;
            if !matches!(count, 3 | 5 | 7) {
                return Err(bad("the rolling week can show 3, 5, or 7 days"));
            }
            let pad = pad(args)?;
            result(omacal_view::view::get_days(&state.pool, start, count as usize, pad).await)
        }
        "get_month" => {
            let year = req_i32(args, "year")?;
            let month = req_u32(args, "month")?;
            result(omacal_view::view::get_month(&state.pool, year, month).await)
        }
        "get_year" => {
            let year = req_i32(args, "year")?;
            result(omacal_view::view::get_year(&state.pool, year).await)
        }
        "get_big_year" => {
            let year = req_i32(args, "year")?;
            result(omacal_view::view::get_big_year(&state.pool, year).await)
        }
        "list_tasks" => result(omacal_view::tasks::list_tasks(state).await),
        "task_lists" => match omacal_view::tasks::writable_task_lists(&state.pool).await {
            Ok(v) => to_value(v),
            Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
        },
        // The task writes go through the same bodies the desktop's commands
        // and the CLI's socket call: `create_task`/`update_task`/
        // `set_task_completed` answer with the fresh list, `delete_task_cmd`
        // assembles it here because no `delete_body` exists (the desktop's own
        // command does exactly this after its write).
        "create_task" => {
            let calendar_id = req_i64(args, "calendar_id")?;
            let summary = req_str(args, "summary")?;
            let due_ms = opt_i64(args, "due_ms")?;
            let due_all_day = opt_bool(args, "due_all_day")?.unwrap_or(true);
            let priority = opt_i64(args, "priority")?.unwrap_or(0);
            result(
                omacal_view::tasks::create_body(
                    state, calendar_id, summary, due_ms, due_all_day, priority,
                )
                .await,
            )
        }
        // `update_impl` directly, not `update_body`: the desktop command
        // carries `calendarId` to move a task between lists, and the CLI's
        // `update_body` deliberately has no move verb. The list is answered
        // fresh, as the desktop's command does.
        "update_task" => {
            let id = req_i64(args, "id")?;
            let summary = req_str(args, "summary")?;
            let due_ms = opt_i64(args, "due_ms")?;
            let due_all_day = req_bool(args, "due_all_day")?;
            let notes = opt_str(args, "notes")?;
            let calendar_id = opt_i64(args, "calendar_id")?;
            let priority = opt_i64(args, "priority")?;
            omacal_view::demo_sync_guard(state.demo).map_err(bad)?;
            omacal_view::tasks::update_impl(
                state, id, summary, due_ms, due_all_day, notes, calendar_id, priority,
            )
            .await
            .map_err(|e| bad(omacal_view::errors::user_facing(&e)))?;
            to_value(omacal_view::tasks::list_body(state).await)
        }
        "set_task_completed" => {
            let id = req_i64(args, "id")?;
            let on = req_bool(args, "on")?;
            result(omacal_view::tasks::complete_body(state, id, on).await)
        }
        "delete_task_cmd" => {
            let id = req_i64(args, "id")?;
            omacal_view::demo_sync_guard(state.demo).map_err(bad)?;
            omacal_view::tasks::delete_impl(state, id)
                .await
                .map_err(|e| bad(omacal_view::errors::user_facing(&e)))?;
            to_value(omacal_view::tasks::list_body(state).await)
        }
        "search_done_tasks" => {
            let query = req_str(args, "query")?;
            let before_ms = opt_i64(args, "before_ms")?;
            let offset = req_usize(args, "offset")?;
            let limit = req_usize(args, "limit")?;
            match omacal_view::tasks::done_page(state, query, before_ms, offset, limit).await {
                Ok(page) => to_value(page),
                Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
            }
        }
        "create_local_task_list" => match omacal_view::tasks::create_local_task_list(state).await {
            Ok(v) => to_value(v),
            Err(e) => Err(bad(e)),
        },
        "create_task_list" => {
            let name = req_str(args, "name")?;
            omacal_view::demo_sync_guard(state.demo).map_err(bad)?;
            match omacal_view::tasks::create_list_impl(state, name).await {
                Ok(v) => to_value(v),
                Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
            }
        }
        "rename_task_list" => {
            let id = req_i64(args, "id")?;
            let name = req_str(args, "name")?;
            omacal_view::demo_sync_guard(state.demo).map_err(bad)?;
            match omacal_view::tasks::rename_list_impl(state, id, name).await {
                Ok(v) => to_value(v),
                Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
            }
        }
        "delete_task_list" => {
            let id = req_i64(args, "id")?;
            omacal_view::demo_sync_guard(state.demo).map_err(bad)?;
            match omacal_view::tasks::delete_list_impl(state, id).await {
                Ok(v) => to_value(v),
                Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
            }
        }
        "set_calendar_selected" => {
            let id = req_i64(args, "id")?;
            let on = req_bool(args, "on")?;
            result_unit(omacal_view::calendars::set_selected(&state.pool, id, on).await)
        }
        "set_calendar_sync" => {
            let id = req_i64(args, "id")?;
            let on = req_bool(args, "on")?;
            result(omacal_view::calendars::set_sync(&state.pool, id, on).await)
        }
        "set_calendar_color" => {
            let id = req_i64(args, "id")?;
            let hex = opt_str(args, "hex")?;
            result_unit(omacal_view::calendars::set_color(&state.pool, id, hex).await)
        }
        // The desktop refreshes the tray/menubar after this; a headless server
        // has no tray, so the store write is the whole of it.
        "set_calendar_label" => {
            let id = req_i64(args, "id")?;
            let label = opt_str(args, "label")?;
            result_unit(omacal_view::calendars::set_label(&state.pool, id, label).await)
        }
        "event_detail" => {
            let id = req_i64(args, "id")?;
            match omacal_view::events::event_detail_impl(state, id).await {
                Ok(v) => to_value(v),
                Err(e) => Err(bad(omacal_view::errors::user_facing(&e))),
            }
        }
        "create_event" => {
            let calendar_id = req_i64(args, "calendar_id")?;
            let fields = req_fields(args, "fields")?;
            let send_updates = req_str(args, "send_updates")?;
            result(omacal_view::events::create_event_body(state, calendar_id, fields, send_updates).await)
        }
        "update_event" => {
            let id = req_i64(args, "id")?;
            let scope = req_str(args, "scope")?;
            let occurrence_start_ms = req_i64(args, "occurrence_start_ms")?;
            let fields = req_fields(args, "fields")?;
            let send_updates = req_str(args, "send_updates")?;
            let target_calendar_id = opt_i64(args, "target_calendar_id")?;
            result(
                omacal_view::events::update_event_body(
                    state,
                    id,
                    scope,
                    occurrence_start_ms,
                    fields,
                    send_updates,
                    target_calendar_id,
                )
                .await,
            )
        }
        "delete_event_cmd" => {
            let id = req_i64(args, "id")?;
            let scope = req_str(args, "scope")?;
            let occurrence_start_ms = req_i64(args, "occurrence_start_ms")?;
            result(omacal_view::events::delete_event_body(state, id, scope, occurrence_start_ms).await)
        }
        "respond_to_event" => {
            let id = req_i64(args, "id")?;
            let response = req_str(args, "response")?;
            let scope = req_str(args, "scope")?;
            let occurrence_start_ms = req_i64(args, "occurrence_start_ms")?;
            result(
                omacal_view::events::respond_event_body(state, id, response, scope, occurrence_start_ms)
                    .await,
            )
        }
        "refresh_event" => {
            let id = req_i64(args, "id")?;
            result(omacal_view::events::refresh_event_impl(state, id).await)
        }
        "open_conference" => open_conference(state, req_i64(args, "id")?).await,
        "get_status" => Ok(stubs::status(&state.pool).await),
        "get_palette" => Ok(stubs::palette()),
        "get_weather" => Ok(stubs::weather()),
        "pending_invites" | "declined_guests" | "changed_meetings" | "list_accounts" => Ok(json!([])),
        "take_open_date" => Ok(Value::Null),
        "take_quick_add" | "take_preferences" => Ok(json!(false)),
        // Deliberately out of the web build: account management, `.ics`
        // import/export and the update/restart/menu surfaces. A 4xx with a
        // sentence, never a panic and never a 500.
        "connect_caldav" | "subscribe_webcal" | "sign_out" | "plan_ics_import"
        | "run_ics_import" | "export_event" | "install_update" | "restart_app"
        | "menubar_feed" | "menubar_action" => Err(deferred()),
        _ => Err(refused()),
    }
}

/// The web build's `open_conference`: where the desktop opens the OS browser,
/// this hands the resolved URL back to the page, which opens it. The URL is
/// resolved from the store in the desktop's own order — structured conference
/// first, then a recognised meeting link in `location`, then one in
/// `description` — so the webview still never chooses a destination, only an
/// event.
async fn open_conference(state: &AppState, id: i64) -> ApiResult {
    let row: Option<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT conference_uri, location, description FROM events WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                let err: anyhow::Error = e.into();
                bad(omacal_view::errors::user_facing(&err))
            })?;
    let Some((conference, location, description)) = row else {
        return Err(bad("This event is no longer on the calendar."));
    };
    let url = conference.or_else(|| {
        omacal_view::meeting::conference_join_url(location.as_deref(), description.as_deref())
    });
    match url {
        Some(url) => to_value(url),
        None => Err(bad("This event has no meeting link.")),
    }
}
