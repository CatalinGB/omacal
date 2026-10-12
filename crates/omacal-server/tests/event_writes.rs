//! Task 5's write routes: a success broadcasts `sync-finished` on `/ws`, a
//! refusal reaches the page as the body's own sentence and broadcasts nothing.
//!
//! There is no real CalDAV server to write to, so the success fixture stands
//! one up with `wiremock`: a DELETE that answers 204 and an empty resync. That
//! is enough to prove the route reaches `omacal-view`'s body, the body
//! succeeds, and the success is what sends the reload frame — without pretending
//! a real Radicale round-trip happened.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use omacal_server::credentials::FileCredentialStore;
use omacal_server::events::Broadcaster;
use omacal_view::CredentialStore;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

/// A unique scratch data dir, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("omacal-writes-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn credentials(&self) -> std::path::PathBuf {
        self.0.join("credentials.json")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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

fn event(
    calendar_id: i64,
    uid: &str,
    conference_uri: Option<&str>,
    location: Option<&str>,
) -> omacal_store::StoredEvent {
    omacal_store::StoredEvent {
        id: 0,
        calendar_id,
        google_id: uid.to_string(),
        summary: Some("Standup".into()),
        location: location.map(str::to_string),
        start_utc: 1_786_352_400_000,
        end_utc: 1_786_356_000_000,
        start_tz: "UTC".into(),
        end_tz: "UTC".into(),
        is_all_day: false,
        recurrence: None,
        recurring_event_id: None,
        original_start_utc: None,
        status: "confirmed".into(),
        self_response: None,
        conference_uri: conference_uri.map(str::to_string),
        color_hex: None,
        calendar_timezone: "UTC".into(),
        description: None,
        etag: Some("\"1\"".into()),
        sequence: 0,
        organizer_email: None,
        guests_can_modify: false,
        attendees: Vec::new(),
        reminders: omacal_store::Reminders { use_default: true, overrides: Vec::new() },
        calendar_default_reminders: Vec::new(),
    }
}

/// A minimal account and calendar, so an event row can satisfy its foreign key.
/// `open_conference` reads only the event, but the store will not hold one
/// without an owning calendar.
async fn seed_calendar(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO accounts (google_sub, email, created_at) VALUES ('acct', 'a@example.com', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO calendars (account_id, google_id, summary, timezone, access_role,
                                supports_events, supports_tasks)
         VALUES (1, 'cal', 'Calendar', 'UTC', 'owner', 1, 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// One CalDAV account and calendar, and one event row ready to be deleted.
async fn seed_caldav(pool: &sqlx::SqlitePool, server_url: &str) -> i64 {
    sqlx::query(
        "INSERT INTO accounts (google_sub, email, provider, server_url, username, created_at)
         VALUES ('caldav:kalio', 'kalio', 'caldav', ?1, 'kalio', 0)",
    )
    .bind(server_url)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO calendars (account_id, google_id, summary, timezone, access_role,
                                supports_events, supports_tasks, sync_enabled)
         VALUES (1, ?1, 'Personal', 'UTC', 'owner', 1, 0, 1)",
    )
    .bind(format!("{server_url}/cal/"))
    .execute(pool)
    .await
    .unwrap();

    let id = omacal_store::upsert_event(pool, &event(1, "standup-1", None, None))
        .await
        .unwrap();
    sqlx::query("UPDATE events SET caldav_href = ?2, raw_ics = ?3 WHERE id = ?1")
        .bind(id)
        .bind(format!("{server_url}/cal/standup-1.ics"))
        .bind("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:standup-1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
        .execute(pool)
        .await
        .unwrap();
    id
}

fn multistatus(body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">{body}</d:multistatus>"#
    )
}

/// A write that reaches a body which succeeds broadcasts `sync-finished`, which
/// is what makes the open page reload.
#[tokio::test]
async fn a_successful_write_broadcasts_sync_finished() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus("")))
        .mount(&server)
        .await;
    Mock::given(method("REPORT"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus("")))
        .mount(&server)
        .await;

    let tmp = TempDir::new();
    let store = FileCredentialStore::at(tmp.credentials());
    CredentialStore::set(&store, "caldav:kalio", "app-password").unwrap();

    let pool = omacal_store::connect_memory().await.unwrap();
    let id = seed_caldav(&pool, &server.uri()).await;

    let mut state = omacal_view::AppState::headless(pool);
    state.credentials = Arc::new(store);
    let state = Arc::new(state);

    let events = Broadcaster::new();
    let (addr, app) = serve(state.clone(), events.clone()).await;
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let (status, body) = post(
        &app,
        "delete_event_cmd",
        &format!(r#"{{"id":{id},"scope":"all","occurrenceStartMs":1786352400000}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body was {body}");

    let frame = next_frame(&mut ws).await.expect("no sync-finished frame arrived");
    assert_eq!(frame["event"], "sync-finished", "frame was {frame}");
    assert_eq!(frame["payload"]["upserted"], 1, "frame was {frame}");
}

/// A refused write answers with the body's own sentence (Tauri's `Err` Display)
/// and must NOT broadcast — a page that reloaded on a refusal would lose the
/// user's place for a change that did not happen.
#[tokio::test]
async fn a_refused_write_returns_the_sentence_and_does_not_broadcast() {
    let pool = omacal_store::connect_memory().await.unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let (addr, app) = serve(state.clone(), events.clone()).await;

    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let (status, body) = post(
        &app,
        "delete_event_cmd",
        r#"{"id":9999,"scope":"all","occurrenceStartMs":0}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, "that event is no longer here");

    assert!(
        next_frame(&mut ws).await.is_none(),
        "a refused write broadcast a reload frame"
    );
}

/// `open_conference` hands the page the resolved URL for an event whose
/// structured conference data carries one.
#[tokio::test]
async fn open_conference_returns_the_structured_url() {
    let pool = omacal_store::connect_memory().await.unwrap();
    seed_calendar(&pool).await;
    let id = omacal_store::upsert_event(
        &pool,
        &event(1, "meet-1", Some("https://meet.google.com/abc-defg-hij"), None),
    )
    .await
    .unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let (_, app) = serve(state.clone(), events.clone()).await;

    let (status, body) = post(&app, "open_conference", &format!(r#"{{"id":{id}}}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "\"https://meet.google.com/abc-defg-hij\"");
}

/// ...and for one whose only link is a recognised meeting URL in `location`,
/// the desktop's own fallback order.
#[tokio::test]
async fn open_conference_falls_back_to_the_location_link() {
    let pool = omacal_store::connect_memory().await.unwrap();
    seed_calendar(&pool).await;
    let id = omacal_store::upsert_event(
        &pool,
        &event(1, "zoom-1", None, Some("Join at https://us02web.zoom.us/j/1234567890")),
    )
    .await
    .unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let (_, app) = serve(state.clone(), events.clone()).await;

    let (status, body) = post(&app, "open_conference", &format!(r#"{{"id":{id}}}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "\"https://us02web.zoom.us/j/1234567890\"");
}

/// An event with no joinable link is refused with the desktop's own sentence.
#[tokio::test]
async fn open_conference_refuses_an_event_with_no_link() {
    let pool = omacal_store::connect_memory().await.unwrap();
    seed_calendar(&pool).await;
    let id = omacal_store::upsert_event(&pool, &event(1, "plain-1", None, Some("Room 4")))
        .await
        .unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let (_, app) = serve(state.clone(), events.clone()).await;

    let (status, body) = post(&app, "open_conference", &format!(r#"{{"id":{id}}}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, "This event has no meeting link.");
}
