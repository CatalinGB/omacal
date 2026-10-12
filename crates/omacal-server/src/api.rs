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

/// What an unimplemented command answers with: Tauri rejects with the Rust
/// `Err`'s `Display`, so the body is that rejection as a sentence the UI can
/// show.
const REFUSED: &str = "{\"code\":\"refused\",\"message\":\"not available in the web build\"}";

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

/// The event mutations: a success changed the calendar, so the page reloads.
fn is_write(cmd: &str) -> bool {
    matches!(
        cmd,
        "create_event" | "update_event" | "delete_event_cmd" | "respond_to_event"
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

fn to_value<T: Serialize>(value: T) -> ApiResult {
    serde_json::to_value(value).map_err(|e| bad(e.to_string()))
}

fn result<T: Serialize>(value: Result<T, String>) -> ApiResult {
    match value {
        Ok(v) => to_value(v),
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
