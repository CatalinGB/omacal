//! Task 6's write routes: the Tasks pane, the calendar list's
//! visibility/colour/label toggles and app settings reach the store from the
//! browser, and the deliberately deferred commands refuse in a sentence rather
//! than panicking or answering a 500.
//!
//! There is no CalDAV server here: every write below lands on an
//! **on-this-device** task list (`ensure_local_task_list`), so the whole route
//! → `omacal-view` body → store path is exercised with no network at all.
//! Success broadcasts `sync-finished` over `/ws`; a refusal broadcasts nothing.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use omacal_server::events::Broadcaster;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

/// Boots the router on an ephemeral port (for the `/ws` half) and returns both
/// the address and the router itself (for the request half). Both share the one
/// `Broadcaster`, so a route answered through `oneshot` still reaches a socket
/// connected to the listening server.
async fn serve(state: Arc<omacal_view::AppState>, events: Broadcaster) -> (std::net::SocketAddr, Router) {
    let app = omacal_server::api::router(state, events, std::path::PathBuf::from("ui/dist"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, server).await;
    });
    (addr, app)
}

/// POSTs a command body and returns the status and the plain-text response.
async fn post(app: &Router, cmd: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/{cmd}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// POSTs and parses a JSON body, panicking if the status is not 2xx.
async fn post_json(app: &Router, cmd: &str, body: &str) -> serde_json::Value {
    let (status, text) = post(app, cmd, body).await;
    assert!(status.is_success(), "{cmd} answered {status}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{cmd} did not answer JSON: {e}: {text}"))
}

/// The next `{ event, payload }` frame, or `None` if nothing arrives in time.
async fn next_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Option<serde_json::Value> {
    let msg = tokio::time::timeout(std::time::Duration::from_millis(500), ws.next())
        .await
        .ok()?
        .expect("the stream closed")
        .expect("a websocket error");
    let Message::Text(text) = msg else {
        panic!("expected a text frame, got {msg:?}");
    };
    Some(serde_json::from_str(text.as_str()).expect("the frame was not JSON"))
}

/// A fresh in-memory store with one on-this-device task list, and its id.
async fn seeded() -> (sqlx::SqlitePool, i64) {
    let pool = omacal_store::connect_memory().await.unwrap();
    let list =
        omacal_store::ensure_local_task_list(&pool, "Tasks on this device", "UTC", 0)
            .await
            .unwrap();
    (pool, list)
}

/// A task create and a tick reach the store and answer the fresh list — the
/// same payload the desktop's command returns.
#[tokio::test]
async fn a_task_create_and_tick_persist_and_answer_the_list() {
    let (pool, list) = seeded().await;
    let state = Arc::new(omacal_view::AppState::headless(pool.clone()));
    let (_, app) = serve(state, Broadcaster::new()).await;

    let created = post_json(
        &app,
        "create_task",
        &format!(
            r#"{{"calendarId":{list},"summary":"Water the plants","dueMs":null,"dueAllDay":true,"priority":0}}"#
        ),
    )
    .await;
    let tasks = created.as_array().expect("the list is an array");
    assert_eq!(tasks.len(), 1, "the new task is in the list: {created}");
    let task = &tasks[0];
    assert_eq!(task["summary"], "Water the plants");
    assert_eq!(task["calendarId"], list);
    assert_eq!(task["completed"], false);
    let id = task["id"].as_i64().unwrap();

    let ticked = post_json(&app, "set_task_completed", &format!(r#"{{"id":{id},"on":true}}"#)).await;
    assert_eq!(ticked[0]["completed"], true, "the tick is reflected: {ticked}");
    assert!(
        ticked[0]["completedMs"].is_i64(),
        "a completion records when: {ticked}"
    );

    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM tasks WHERE id = ?1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "completed", "the row is what the answer said");
}

/// A calendar toggle and a setting write change the store, and the settings
/// answer reflects the value that was stored — the reload the page does must
/// not disagree with the row.
#[tokio::test]
async fn a_calendar_toggle_and_a_setting_write_change_the_store() {
    let (pool, list) = seeded().await;
    let state = Arc::new(omacal_view::AppState::headless(pool.clone()));
    let (_, app) = serve(state, Broadcaster::new()).await;

    let (status, body) =
        post(&app, "set_calendar_selected", &format!(r#"{{"id":{list},"on":false}}"#)).await;
    assert_eq!(status, StatusCode::OK, "body was {body}");
    let (selected,): (i64,) =
        sqlx::query_as("SELECT selected FROM calendars WHERE id = ?1").bind(list).fetch_one(&pool).await.unwrap();
    assert_eq!(selected, 0, "the calendar was hidden");

    let (status, body) =
        post(&app, "set_calendar_color", &format!(r##"{{"id":{list},"hex":"#123456"}}"##)).await;
    assert_eq!(status, StatusCode::OK, "body was {body}");
    let (color,): (Option<String>,) =
        sqlx::query_as("SELECT color_override FROM calendars WHERE id = ?1").bind(list).fetch_one(&pool).await.unwrap();
    assert_eq!(color.as_deref(), Some("#123456"));

    // The week-start setting is a plain stored value; the answer must carry it.
    let answer = post_json(&app, "set_setting", r#"{"setting":{"key":"weekStart","value":"sunday"}}"#).await;
    assert_eq!(answer["weekStart"], "sunday", "the answer reflects the write: {answer}");
    let (stored,): (String,) =
        sqlx::query_as("SELECT value FROM settings WHERE key = 'week_start'").fetch_one(&pool).await.unwrap();
    assert_eq!(stored, "sunday", "the row is what the answer said");

    // And a fresh read agrees, so a page reload shows the same week.
    let read = post_json(&app, "get_settings", "{}").await;
    assert_eq!(read["weekStart"], "sunday");
}

/// Every deliberately deferred command answers a 4xx with the same sentence —
/// never a panic, never a 500.
#[tokio::test]
async fn the_deferred_commands_refuse_with_a_sentence() {
    let pool = omacal_store::connect_memory().await.unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let (_, app) = serve(state, Broadcaster::new()).await;

    for cmd in [
        "connect_caldav",
        "subscribe_webcal",
        "sign_out",
        "plan_ics_import",
        "run_ics_import",
        "export_event",
        "install_update",
        "restart_app",
        "menubar_feed",
        "menubar_action",
    ] {
        let (status, body) = post(&app, cmd, "{}").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{cmd} answered {status}: {body}");
        assert_eq!(body, "not available in the web build", "{cmd} said: {body}");
    }
}

/// A successful write broadcasts `sync-finished`, which is what repaints an
/// open page; a refused write must not, or the page would lose the user's
/// place for a change that did not happen.
#[tokio::test]
async fn a_successful_write_broadcasts_and_a_failure_does_not() {
    let (pool, list) = seeded().await;
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let (addr, app) = serve(state, events.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();

    let _ = post_json(
        &app,
        "create_task",
        &format!(r#"{{"calendarId":{list},"summary":"Milk","dueMs":null,"dueAllDay":true,"priority":0}}"#),
    )
    .await;
    let frame = next_frame(&mut ws).await.expect("no sync-finished frame arrived");
    assert_eq!(frame["event"], "sync-finished", "frame was {frame}");
    assert_eq!(frame["payload"]["upserted"], 1, "frame was {frame}");

    // A tick on a task that is not there: the body's own sentence, no frame.
    let (status, body) = post(&app, "set_task_completed", r#"{"id":9999,"on":true}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, omacal_view::tasks::TASK_GONE);
    assert!(
        next_frame(&mut ws).await.is_none(),
        "a refused write broadcast a reload frame"
    );
}
