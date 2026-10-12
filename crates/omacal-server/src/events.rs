//! The one WebSocket channel: a broadcast of `{ "event", "payload" }` frames
//! that every connected browser subscribes to.
//!
//! It also implements [`EventSink`], so a service-layer body that would have
//! `emit`ed into a webview can broadcast here instead. Today only the sync
//! ticker talks to it; later tasks' write paths will too.

use serde_json::Value;
use tokio::sync::broadcast;

use omacal_view::EventSink;

/// Fan-out to every open `/ws` connection.
#[derive(Clone)]
pub struct Broadcaster {
    tx: broadcast::Sender<String>,
}

impl Broadcaster {
    pub fn new() -> Self {
        // A small buffer: a browser that cannot keep up with a `sync-finished`
        // is a browser that will reconnect, and lagging is not a reason to
        // block the ticker.
        let (tx, _) = broadcast::channel(16);
        Self { tx }
    }

    /// A new subscriber. The receiver starts empty, so a freshly connected
    /// page hears only events that happen after it connected.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Broadcasts an event. No receivers is not an error.
    pub fn send(&self, event: &str, payload: Value) {
        let frame = serde_json::json!({ "event": event, "payload": payload }).to_string();
        let _ = self.tx.send(frame);
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for Broadcaster {
    fn emit(&self, event: &str, payload: Value) {
        self.send(event, payload);
    }
}
