//! The Tauri-free HTTP skin over `omacal-view`.
//!
//! `api::router` wires `/healthz`, `POST /api/{cmd}` and the static Svelte
//! build; `main` builds the pool and binds the listener. Nothing here reaches
//! for `tauri`.

pub mod api;
pub mod credentials;
pub mod events;
pub mod stubs;
pub mod sync;
