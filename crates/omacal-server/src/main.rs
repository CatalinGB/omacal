//! Boot: resolve the data dir, open the same database the desktop uses, and
//! serve the routes.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    omacal_view::freeze_effective_timezone();

    let data_dir = data_dir().context("could not resolve the data directory")?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("could not create {}", data_dir.display()))?;

    let url = format!("sqlite://{}", data_dir.join("omacal.db").display());
    let pool = omacal_store::connect(&url)
        .await
        .with_context(|| format!("could not open {url}"))?;

    let state = Arc::new(omacal_view::AppState::headless(pool));
    let ui_dir = std::env::var_os("OMACAL_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ui/dist"));
    let app = omacal_server::api::router(state, ui_dir);

    let bind = std::env::var("OMACAL_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("could not bind {bind}"))?;
    tracing::info!(address = %bind, data_dir = %data_dir.display(), "omacal-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// The data directory, in the order the task names: `$OMACAL_DATA_DIR`, then
/// an absolute `$XDG_DATA_HOME`, then `$HOME/.local/share` — each joined with
/// the `com.omacal.app` identifier the desktop's `app_data_dir` uses, so the
/// server opens the same `omacal.db`.
fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OMACAL_DATA_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    Some(
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|h| h.join(".local/share")))?
            .join("com.omacal.app"),
    )
}
