//! Boot: resolve the data dir, open the same database the desktop uses, seed
//! the credential store, start the sync ticker and serve the routes.
//!
//! A one-shot `set-credential <key>` reads a secret from stdin and writes it to
//! the credential file without printing it — the import path the scoping doc
//! asks for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use omacal_server::credentials::FileCredentialStore;
use omacal_server::events::Broadcaster;
use omacal_view::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    omacal_view::freeze_effective_timezone();

    let data_dir = data_dir().context("could not resolve the data directory")?;

    // The one-shot import path runs before anything binds a port or opens the
    // database: it only touches the credential file.
    if let Some(key) = set_credential_key() {
        return import_credential(&data_dir, &key);
    }

    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("could not create {}", data_dir.display()))?;
    make_private_dir(&data_dir)?;

    let url = format!("sqlite://{}", data_dir.join("omacal.db").display());
    let pool = omacal_store::connect(&url)
        .await
        .with_context(|| format!("could not open {url}"))?;

    let mut state = omacal_view::AppState::headless(pool);
    state.credentials = Arc::new(FileCredentialStore::at(credentials_path(&data_dir)));
    let state = Arc::new(state);

    let events = Broadcaster::new();
    omacal_server::sync::spawn(state.clone(), events.clone());

    let ui_dir = std::env::var_os("OMACAL_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ui/dist"));
    let app = omacal_server::api::router(state, events, ui_dir);

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

/// The credential file's path on the volume.
fn credentials_path(data_dir: &Path) -> PathBuf {
    data_dir.join("credentials.json")
}

/// Holds the data dir to `0700`, where the credential file and the database
/// live. Best-effort on platforms without Unix modes.
#[cfg(unix)]
fn make_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("could not restrict {}", dir.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_private_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

/// `Some(key)` when invoked as `omacal-server set-credential <key>`.
fn set_credential_key() -> Option<String> {
    let mut args = std::env::args().skip(1);
    match (args.next(), args.next(), args.next()) {
        (Some(command), Some(key), None) if command == "set-credential" => Some(key),
        (Some(command), _, _) if command == "set-credential" => {
            eprintln!("usage: omacal-server set-credential <key>   # secret on stdin");
            std::process::exit(2);
        }
        _ => None,
    }
}

/// Reads one secret from stdin and stores it, printing only the key.
fn import_credential(data_dir: &Path, key: &str) -> Result<()> {
    use std::io::Read;

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("could not read the secret from stdin")?;
    // A trailing newline is what a shell pipe appends; it is not part of the
    // secret.
    let secret = raw.trim_end_matches(['\n', '\r']);
    if secret.is_empty() {
        anyhow::bail!("refusing to store an empty secret for {key}");
    }

    let store = FileCredentialStore::at(credentials_path(data_dir));
    store.set(key, secret)?;
    println!("stored credential for {key}");
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
