//! The sync ticker's WebSocket report, proved end to end: a real `/ws`
//! connection, `sync::run_once` driven by hand, and the frame the browser would
//! receive.
//!
//! There is no real CalDAV server here, so the "success" fixture uses a CalDAV
//! account with no calendars: the password is read from the credential file,
//! the client is built, and the sync returns zero rows — enough to prove the
//! credential store and the success broadcast are wired, without pretending a
//! network round-trip happened.

use std::sync::Arc;

use futures_util::StreamExt;
use omacal_server::credentials::FileCredentialStore;
use omacal_server::events::Broadcaster;
use tokio_tungstenite::tungstenite::Message;

/// A unique scratch data dir, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("omacal-sync-ws-{}-{n}", std::process::id()));
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

/// Boots the router on an ephemeral port and returns its address.
async fn serve(state: Arc<omacal_view::AppState>, events: Broadcaster) -> std::net::SocketAddr {
    let app = omacal_server::api::router(state, events, std::path::PathBuf::from("ui/dist"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// The next `{ event, payload }` frame, or panics on timeout/close.
async fn next_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("no frame arrived within 5s")
        .expect("the stream closed")
        .expect("a websocket error");
    let Message::Text(text) = msg else {
        panic!("expected a text frame, got {msg:?}");
    };
    serde_json::from_str(text.as_str()).expect("the frame was not JSON")
}

/// A successful sync broadcasts `sync-finished`, and the credential file is what
/// made it succeed: with no stored password the CalDAV client would not build,
/// and the account would land in the failed list.
#[tokio::test]
async fn a_successful_sync_broadcasts_sync_finished_over_the_websocket() {
    let tmp = TempDir::new();
    let store = FileCredentialStore::at(tmp.credentials());
    omacal_view::CredentialStore::set(&store, "caldav:kalio", "app-password").unwrap();

    let pool = omacal_store::connect_memory().await.unwrap();
    sqlx::query(
        "INSERT INTO accounts (google_sub, email, provider, server_url, username, created_at)
         VALUES ('caldav:kalio', 'kalio', 'caldav', 'http://127.0.0.1:1/kalio', 'kalio', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut state = omacal_view::AppState::headless(pool);
    state.credentials = Arc::new(store);
    let state = Arc::new(state);

    let events = Broadcaster::new();
    let addr = serve(state.clone(), events.clone()).await;
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    omacal_server::sync::run_once(&state, &events).await;

    let frame = next_frame(&mut ws).await;
    assert_eq!(frame["event"], "sync-finished", "frame was {frame}");
    assert_eq!(frame["payload"]["upserted"], 0, "frame was {frame}");
    assert!(
        omacal_view::status::last_sync_ms(&state.pool).await.unwrap().is_some(),
        "a successful sync must record its timestamp"
    );
}

/// A failed sync broadcasts the fixed sentence and nothing else — no host, no
/// URL, no error text.
#[tokio::test]
async fn a_failed_sync_broadcasts_the_fixed_sentence_without_detail() {
    let pool = omacal_store::connect_memory().await.unwrap();
    // No server_url, so the CalDAV client cannot be built: a clean failure
    // whose underlying message names the account and would leak if forwarded.
    sqlx::query(
        "INSERT INTO accounts (google_sub, email, provider, username, created_at)
         VALUES ('caldav:kalio', 'kalio', 'caldav', 'kalio', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let state = Arc::new(omacal_view::AppState::headless(pool));
    let events = Broadcaster::new();
    let addr = serve(state.clone(), events.clone()).await;
    let url = format!("ws://{addr}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    omacal_server::sync::run_once(&state, &events).await;

    let frame = next_frame(&mut ws).await;
    assert_eq!(frame["event"], "sync-failed", "frame was {frame}");
    assert_eq!(
        frame["payload"]["message"],
        omacal_view::sync_loop::SYNC_FAILED_MESSAGE,
        "frame was {frame}"
    );
    let printed = frame.to_string();
    assert!(!printed.contains("server url"), "the error leaked: {printed}");
    assert!(!printed.contains("kalio"), "the account leaked: {printed}");
}
