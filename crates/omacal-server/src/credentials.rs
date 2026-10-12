//! The web server's [`CredentialStore`]: a `0600` JSON file on the volume,
//! keyed exactly as the desktop's keyring entries are.
//!
//! The shape is a flat JSON object `{ "<key>": "<secret>" }`. Keys are the ones
//! the service layer has always used — a bare account email for a Google
//! refresh token, `caldav:<name>` for a CalDAV password — so the same bodies
//! run unchanged on either host.
//!
//! On a private, single-user volume a `0600` file is the MVP answer (the
//! scoping doc's decision): encryption at rest is a later nicety, and the
//! container is reached only over Tailscale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use omacal_view::CredentialStore;

/// A credential store backed by one JSON file.
///
/// Reads re-read the file each time: the file is tiny, and a cache is one more
/// thing that can disagree with what is on disk. Writes serialise on a mutex so
/// two read-modify-write cycles cannot lose each other's keys.
pub struct FileCredentialStore {
    path: PathBuf,
    write_lock: std::sync::Mutex<()>,
}

impl FileCredentialStore {
    /// A store at `path`, which need not exist yet.
    pub fn at(path: PathBuf) -> Self {
        Self { path, write_lock: std::sync::Mutex::new(()) }
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_map(&self) -> anyhow::Result<BTreeMap<String, String>> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(BTreeMap::new()),
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
            set_dir_mode(dir, 0o700)?;
        }
        let bytes = serde_json::to_vec_pretty(map)?;
        // Write-then-rename, so a reader never sees a half-written file and the
        // file's permissions are set at creation rather than after the fact.
        let tmp = self.path.with_extension("json.tmp");
        write_private(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl CredentialStore for FileCredentialStore {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.read_map()?.get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().expect("credential store mutex poisoned");
        let mut map = self.read_map()?;
        map.insert(key.to_string(), secret.to_string());
        self.write_map(&map)
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().expect("credential store mutex poisoned");
        let mut map = self.read_map()?;
        map.remove(key);
        self.write_map(&map)
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode(dir: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_dir_mode(_dir: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory under the system temp dir, removed by the
    /// guard when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "omacal-cred-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join("credentials.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_reads_as_no_credentials() {
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        assert_eq!(store.get("caldav:kalio").unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips() {
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        store.set("caldav:kalio", "hunter2").unwrap();
        assert_eq!(store.get("caldav:kalio").unwrap().as_deref(), Some("hunter2"));
    }

    #[test]
    fn set_replaces_an_existing_key_and_keeps_the_others() {
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        store.set("a", "1").unwrap();
        store.set("b", "2").unwrap();
        store.set("a", "3").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("3"));
        assert_eq!(store.get("b").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn delete_removes_only_that_key_and_a_missing_key_is_not_an_error() {
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        store.set("a", "1").unwrap();
        store.set("b", "2").unwrap();
        store.delete("a").unwrap();
        store.delete("never-was").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert_eq!(store.get("b").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn the_file_is_a_flat_json_object() {
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        store.set("caldav:kalio", "secret").unwrap();
        let bytes = std::fs::read(tmp.path()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["caldav:kalio"], "secret");
        assert_eq!(value.as_object().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_0600_and_its_directory_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        let store = FileCredentialStore::at(tmp.path());
        store.set("caldav:kalio", "secret").unwrap();

        let file_mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "credentials file mode was {file_mode:o}");
        let dir_mode = std::fs::metadata(&tmp.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "credentials dir mode was {dir_mode:o}");
    }
}
