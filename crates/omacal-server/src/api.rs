//! The routes: a health probe, one command endpoint and the static build.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};

use omacal_view::AppState;

use crate::stubs;

/// What an unimplemented command answers with: Tauri rejects with the Rust
/// `Err`'s `Display`, so the body is that rejection as a sentence the UI can
/// show.
const REFUSED: &str = "{\"code\":\"refused\",\"message\":\"not available in the web build\"}";

pub fn router(state: Arc<AppState>, ui_dir: PathBuf) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/{cmd}", post(api))
        .fallback_service(
            tower_http::services::ServeDir::new(ui_dir).append_index_html_on_directories(true),
        )
        .with_state(state)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn api(State(state): State<Arc<AppState>>, Path(cmd): Path<String>, body: Bytes) -> Response {
    let args = match parse_args(&body) {
        Ok(v) => v,
        Err(e) => return bad(e).into_response(),
    };
    match dispatch(&state, &cmd, &args).await {
        Ok(value) => Json(value).into_response(),
        Err((status, message)) => (status, message).into_response(),
    }
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
        "get_status" => Ok(stubs::status()),
        "get_palette" => Ok(stubs::palette()),
        "get_weather" => Ok(stubs::weather()),
        "pending_invites" | "declined_guests" | "changed_meetings" | "list_accounts" => Ok(json!([])),
        "take_open_date" => Ok(Value::Null),
        "take_quick_add" | "take_preferences" => Ok(json!(false)),
        _ => Err(refused()),
    }
}
