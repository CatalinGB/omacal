//! Boot: resolve the data dir, open the same database the desktop uses, seed
//! the credential store, start the sync ticker and serve the routes.
//!
//! Two one-shot subcommands run before any of that:
//! - `set-credential <key>` reads a secret from stdin and writes it to the
//!   credential file without printing it — the import path the scoping doc
//!   asks for.
//! - `healthcheck` probes the local `/healthz` and exits 0/1. The distroless
//!   image has no shell or curl, so the Dockerfile's `HEALTHCHECK` uses this
//!   exec-form subcommand instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use omacal_server::credentials::FileCredentialStore;
use omacal_server::events::Broadcaster;
use omacal_view::CredentialStore;

#[tokio::main]
async fn main() -> Result<()> {
    // The health probe is a connected client, not the server: run it before
    // the tracing subscriber (nothing is logged on the success path), before
    // the data dir is touched, and before any port is bound.
    if is_healthcheck() {
        std::process::exit(healthcheck_exit_code());
    }

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

/// `true` when invoked as `omacal-server healthcheck`.
fn is_healthcheck() -> bool {
    matches!(std::env::args().nth(1).as_deref(), Some("healthcheck"))
}

/// `0` when the local `/healthz` answers 200, `1` otherwise. Any failure is
/// written to stderr (Docker surfaces a healthcheck's output in `inspect`).
fn healthcheck_exit_code() -> i32 {
    match probe_healthz() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("healthcheck failed: {err:#}");
            1
        }
    }
}

/// One `GET /healthz` over a raw TCP socket: the container has no HTTP client
/// crate, and the probe only needs the status line.
fn probe_healthz() -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let target = healthcheck_target();
    let timeout = Duration::from_secs(3);
    let addr = target
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {target}"))?
        .next()
        .with_context(|| format!("no address for {target}"))?;

    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("could not connect to {target}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    // HTTP/1.0 with `Connection: close`: the body is not read, and there is no
    // chunked framing to trip over.
    write!(
        stream,
        "GET /healthz HTTP/1.0\r\nHost: {target}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;

    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status)?;
    // "HTTP/1.1 200 OK" → "200".
    match status.split_whitespace().nth(1) {
        Some("200") => Ok(()),
        _ => anyhow::bail!("unexpected status line: {}", status.trim()),
    }
}

/// The address to probe: the port from `OMACAL_BIND`, with a wildcard or empty
/// host swapped for loopback. The default matches the server's own default.
fn healthcheck_target() -> String {
    let bind = std::env::var("OMACAL_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    probe_address(&bind).to_string()
}

/// `bind` → the loopback address a same-host client can reach it on.
fn probe_address(bind: &str) -> String {
    let (host, port) = bind.rsplit_once(':').unwrap_or(("0.0.0.0", "8080"));
    let host = host.trim_matches(['[', ']']);
    let host = if host.is_empty() || host == "0.0.0.0" || host == "::" {
        "127.0.0.1"
    } else {
        host
    };
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
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

#[cfg(test)]
mod tests {
    use super::probe_address;

    #[test]
    fn a_wildcard_bind_probes_loopback_on_the_same_port() {
        assert_eq!(probe_address("0.0.0.0:8080"), "127.0.0.1:8080");
        assert_eq!(probe_address("[::]:8080"), "127.0.0.1:8080");
    }

    #[test]
    fn an_explicit_host_is_probed_as_written() {
        assert_eq!(probe_address("127.0.0.1:8090"), "127.0.0.1:8090");
        assert_eq!(probe_address("192.168.1.200:8080"), "192.168.1.200:8080");
    }

    #[test]
    fn a_missing_host_defaults_to_loopback_and_the_default_port() {
        assert_eq!(probe_address(":8080"), "127.0.0.1:8080");
    }
}
