//! The desktop half of the settings module: everything that needs a `tauri`
//! handle — applying a change to the running window, tray, weather loop or
//! menu surfaces, the restart paths, the two OS surfaces the About pane opens,
//! and the `#[tauri::command]` wrappers.
//!
//! The store path, the read path and every pure helper they sit on now live in
//! `omacal-view::settings` and are re-exported here, so every
//! `crate::settings::…` call site is unchanged.

use crate::AppState;

pub use omacal_view::settings::*;

/// The boot fast-path beside the database: `main()` must export `TZ` before
/// GTK and the webview initialise — both capture the zone at process start —
/// and at that moment there is no Tauri handle to resolve the data dir with,
/// let alone an async pool. So the setter writes the zone to this one-line
/// sidecar too, and `apply_display_tz_early` (lib.rs) reads it with plain
/// std::fs. The database row stays the source of truth for the settings UI;
/// `setup` re-syncs the sidecar from it on every launch, so a divergence
/// (a crash between the two writes) heals itself one restart later.
pub(crate) const DISPLAY_TZ_SIDECAR: &str = "display-tz";

/// Writes (or removes, for "system default") the sidecar. Split out and
/// handed a directory so a test can drive it against a tempdir.
pub(crate) fn write_tz_sidecar(dir: &std::path::Path, tz: Option<&str>) -> std::io::Result<()> {
    let path = dir.join(DISPLAY_TZ_SIDECAR);
    match tz {
        Some(tz) => std::fs::write(path, tz),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
}

/// Whether this process is running under Hyprland — the one compositor the
/// frameless default was made for and verified on. Hyprland exports its
/// instance signature to every client it starts, and nothing else sets it.
/// Other tiling compositors are not detected on purpose: an install there
/// keeps a frame by default and has the setting, rather than inheriting a
/// guess made for a different desktop.
pub(crate) fn on_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty())
}

#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(read_settings(&state.pool).await)
}

/// Every zone the picker may offer, straight from jiff's copy of the IANA
/// database — the same authority `TimeZone::get` validates against, so the
/// list and the validator cannot disagree.
#[tauri::command]
pub fn list_timezones() -> Vec<String> {
    let mut zones: Vec<String> =
        jiff::tz::db().available().map(|name| name.to_string()).collect();
    zones.sort();
    zones
}

/// Stores the display zone and restarts the app to apply it — see
/// [`AppSettings::display_timezone`] for why a restart is the mechanism.
/// `None` returns to the system zone.
///
/// The restart is spawned on a short delay so this command's reply reaches
/// the webview first and the form can say "restarting" instead of dying
/// mid-await. Validation refuses rather than stores: a zone jiff does not
/// know would come back at next launch as a `TZ` nothing honours, which is
/// the system zone wearing the wrong label.
#[tauri::command]
pub async fn set_display_timezone(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    tz: Option<String>,
) -> Result<(), String> {
    let tz = tz.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(name) = tz.as_deref() {
        jiff::tz::TimeZone::get(name)
            .map_err(|_| format!("OmaCal does not know the time zone \"{name}\""))?;
    }

    write(&state.pool, DISPLAY_TZ_KEY, tz.as_deref().unwrap_or(""))
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;

    use tauri::Manager;
    if let Ok(dir) = app.path().app_data_dir() {
        if let Err(e) = write_tz_sidecar(&dir, tz.as_deref()) {
            // The DB row is stored; setup's re-sync writes the sidecar on
            // the next launch, so this costs one restart of staleness.
            tracing::warn!(%e, "could not write the display-tz sidecar");
        }
    }

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        drop(app);
        crate::restart::hard_restart();
    });
    Ok(())
}

/// Restarts the app because the user asked to — the system-tz banner's one
/// action. The same delayed shape as [`set_display_timezone`]'s restart and
/// for the same reason: the reply has to reach the webview before the
/// process re-execs, so the button can say "restarting" instead of dying
/// mid-await. No state to write first: the restart *is* the fix, because the
/// fresh process reads the zone the system already moved to. Through
/// [`crate::restart::hard_restart`], not `app.restart()` — the graceful
/// teardown is the thing that hangs (its module doc has the field story).
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        drop(app);
        crate::restart::hard_restart();
    });
}

/// The OS half of [`decorated`]: sets the main window's decorations from a
/// choice and the desktop it is on. Called by `setup` before the window is
/// shown, and again from [`Setting::WindowFrame`]. A no-op on macOS, where the
/// overlay title bar is the frame and `read_settings` reports no choice —
/// `cfg!` so the Linux-only CI still compiles the macOS arm.
pub(crate) fn apply_window_frame(app: &tauri::AppHandle, frame: WindowFrame) {
    if cfg!(target_os = "macos") {
        return;
    }
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_decorations(decorated(frame, on_hyprland()));
    }
}

/// Sizes the main window's content: at launch, from the stored setting, and
/// again from [`Setting::InterfaceScalePercent`], so a change shows at once rather than
/// at the next start. The webview's zoom, not a CSS transform: layout, hit
/// testing and every pointer coordinate the grid reads stay in one unit.
pub(crate) fn apply_interface_scale(app: &tauri::AppHandle, percent: u32) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        if let Err(e) = w.set_zoom(f64::from(percent) / 100.0) {
            tracing::warn!(%e, percent, "could not apply the interface scale");
        }
    }
}

#[tauri::command]
pub fn open_date_format_guide() -> Result<(), String> {
    crate::browser::open_external("https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html#conversion-specifications")
        .map_err(|e| crate::errors::user_facing(&e.into()))
}

/// The two addresses the About pane offers, resolved here rather than in the
/// webview.
///
/// `open_conference`'s rule, and `open_latest_release`'s before it: the UI
/// sends a *name*, never a URL, so a link the page could be tricked into
/// building is not a link this can open. The pair is exhaustive and the
/// refusal is deliberately not in `errors.rs`'s allowlist, because a name the
/// shipped UI cannot send is a bug rather than something to explain.
#[tauri::command]
pub fn open_project_link(link: String) -> Result<(), String> {
    let url = match link.as_str() {
        "repository" => "https://github.com/x3me/omacal",
        "site" => "https://omacal.app",
        _ => return Err("that link is not available".into()),
    };
    crate::browser::open_external(url).map_err(|e| crate::errors::user_facing(&e.into()))
}

/// Persist the snapshot before notifying either popup; neither waits for a poll.
pub(crate) async fn refresh_menu_surfaces(app: &tauri::AppHandle, state: &AppState) {
    use tauri::Emitter;
    crate::upcoming::refresh(&state.pool, state.demo).await;
    crate::tray::refresh(app);
    let _ = app.emit("menubar-changed", ());
    #[cfg(target_os = "linux")]
    if !state.demo && std::path::Path::new("/usr/share/omarchy/shell/shell.qml").is_file() {
        // Fixed IPC arguments, no output retained, and no orphaned process
        // if the shell is unavailable. The widget keeps its polling fallback.
        if let Ok(mut child) = tokio::process::Command::new("/usr/bin/qs")
            .args(["ipc", "-n", "-p", "/usr/share/omarchy/shell", "call", "--", "omacal.upcoming", "refresh"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()).kill_on_drop(true).spawn() {
            if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
        }
    }
}

/// What a stored setting changes in the running app, in the same breath as
/// the write: a control whose effect waits for the next launch cannot be told
/// from one that does nothing.
async fn apply(app: &tauri::AppHandle, state: &AppState, setting: &Setting) {
    use Setting as S;
    match setting {
        S::WeatherEnabled(true) => {
            crate::weather::refresh_soon(app.clone(), state.pool.clone(), state.demo, true);
        }
        S::TrayIcon(on) => crate::tray::set_visible(app, *on),
        S::Appearance(appearance) => {
            let palette =
                crate::theme::resolve(crate::theme::omarchy_theme_dir().as_deref(), *appearance);
            let dark = palette.is_dark;
            // GTK settings must not be touched from a command's thread.
            let _ = app.run_on_main_thread(move || crate::apply_gtk_dark_hint(dark));
            use tauri::Emitter;
            let _ = app.emit("theme-changed", palette);
        }
        S::WindowFrame(frame) => apply_window_frame(app, *frame),
        S::QuitOnClose(on) => state.quit_on_close.store(*on, std::sync::atomic::Ordering::Relaxed),
        S::StartOnLogin(mode) => crate::tray::apply_autostart(app, state.demo, mode.registers()),
        S::InterfaceScalePercent(percent) => apply_interface_scale(app, *percent),
        _ => {}
    }
    if setting.shows_on_menu_surfaces() {
        refresh_menu_surfaces(app, state).await;
    }
}

/// Stores one setting, applies it, and answers with the settings now in force.
#[tauri::command]
pub async fn set_setting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    setting: Setting,
) -> Result<AppSettings, String> {
    store_setting(&state.pool, &setting).await.map_err(SetError::shown)?;
    apply(&app, &state, &setting).await;
    Ok(read_settings(&state.pool).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        omacal_store::connect_memory().await.unwrap()
    }

    /// The About pane's links, as a pair rather than as a URL the page sends.
    /// A name that is not one of the two is refused rather than opened, which
    /// is the whole point of resolving them here.
    #[test]
    fn only_the_two_project_links_can_be_opened() {
        assert!(open_project_link("https://example.com".into()).is_err());
        assert!(open_project_link("Repository".into()).is_err());
        assert!(open_project_link(String::new()).is_err());
    }

    #[tokio::test]
    async fn menu_date_formats_preserve_legacy_and_follow_general_or_custom() {
        let p = omacal_store::connect_memory().await.unwrap();
        let date = jiff::civil::date(2026, 9, 7);
        assert_eq!(read_settings(&p).await.menubar_date_format, "general");
        write(&p, "show_date", "1").await.unwrap();
        let legacy = read_settings(&p).await;
        assert_eq!(legacy.menubar_date_format, "custom");
        assert_eq!(menu_date(&legacy, date), "7");
        store_setting(&p, &Setting::MenubarDateFormat { format: "general".into(), custom: "%-d".into() }).await.unwrap();
        write(&p, "date_format", "dmy").await.unwrap();
        assert_eq!(menu_date(&read_settings(&p).await, date), "07/09/2026");
        store_setting(&p, &Setting::MenubarDateFormat { format: "iso".into(), custom: "%-d".into() }).await.unwrap();
        assert_eq!(menu_date(&read_settings(&p).await, date), "2026-09-07");
        store_setting(&p, &Setting::MenubarDateFormat { format: "custom".into(), custom: "%a %b %-d".into() }).await.unwrap();
        assert_eq!(menu_date(&read_settings(&p).await, date), "Mon Sep 7");
        assert_eq!(custom_menu_date("%d", date).unwrap(), "07");
        for invalid in ["", "%", "%Q", "%H", "%n", "%1000000000d"] {
            assert!(store_setting(&p, &Setting::MenubarDateFormat { format: "custom".into(), custom: invalid.into() }).await.is_err(), "{invalid}");
        }
        assert_eq!(menu_date(&read_settings(&p).await, date), "Mon Sep 7");
        assert_eq!(crate::upcoming::current(&p, 1_788_804_000_000).await.unwrap().today.unwrap().label,
            menu_date(&read_settings(&p).await, jiff::Timestamp::from_millisecond(1_788_804_000_000).unwrap().to_zoned(jiff::tz::TimeZone::system()).date()));
    }

    /// The interval the *loop* uses still clamps, because a row edited by hand
    /// with `sqlite3` never passed through the command that refuses.
    #[tokio::test]
    async fn a_hand_edited_row_below_the_floor_is_still_clamped_on_the_way_out() {
        let p = pool().await;
        write(&p, "sync_interval_ms", "100").await.unwrap();

        assert_eq!(read_settings(&p).await.sync_interval_ms, 100, "reported as stored");
        assert_eq!(
            crate::sync_loop::interval_ms(&p).await,
            omacal_view::sync_loop::MIN_INTERVAL_MS,
            "and clamped where it is actually used",
        );
    }

    /// The effective zone is not the preference: it is reported whether or
    /// not one is stored, and it is always a name the picker could have
    /// offered and `set_display_timezone` would accept.
    ///
    /// It does **not** pin the spelling, and cannot: jiff answers to
    /// `Europe/Kiev` as readily as to `Europe/Kyiv`, so both would pass
    /// here. Which of the two reaches the screen is the webview's disagreement
    /// with the database, and `ui/tests/timezone-label.spec.ts` is where that
    /// is held (#140). What this holds is the seam underneath it: one
    /// database answering the picker, the validator and this field.
    #[tokio::test]
    async fn the_effective_zone_is_always_a_name_the_validator_accepts() {
        let p = pool().await;

        let reported = read_settings(&p).await.effective_timezone;
        assert!(!reported.is_empty(), "a zone is always named, even as UTC");
        assert!(
            jiff::tz::TimeZone::get(&reported).is_ok(),
            "{reported} is not a zone jiff knows, so the picker never offered it",
        );
        assert!(list_timezones().contains(&reported), "{reported} is missing from the picker");

        // A stored display zone does not change the answer — the process is
        // in the zone it launched in, whatever the row now says.
        write(&p, "display_timezone", "Europe/Sofia").await.unwrap();
        assert_eq!(read_settings(&p).await.effective_timezone, reported);
    }

    /// The sidecar is what `main()` reads before Tauri exists; writing Some
    /// creates it, None removes it, and removing what is absent is not an
    /// error (a fresh install clears to system).
    #[test]
    fn the_tz_sidecar_writes_and_clears() {
        let dir = std::env::temp_dir().join(format!("omacal-tz-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        write_tz_sidecar(&dir, None).unwrap(); // absent: still fine
        write_tz_sidecar(&dir, Some("Europe/Sofia")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(DISPLAY_TZ_SIDECAR)).unwrap(),
            "Europe/Sofia"
        );
        write_tz_sidecar(&dir, None).unwrap();
        assert!(!dir.join(DISPLAY_TZ_SIDECAR).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The picker's list comes from the same database the validator asks, so
    /// everything offered is acceptable — spot-checked on the two zones this
    /// project lives between.
    #[test]
    fn the_zone_list_is_sorted_and_knows_the_real_world() {
        let zones = list_timezones();
        assert!(zones.iter().any(|z| z == "Europe/Sofia"));
        assert!(zones.iter().any(|z| z == "Asia/Kolkata"));
        let mut sorted = zones.clone();
        sorted.sort();
        assert_eq!(zones, sorted);
        for z in ["Europe/Sofia", "Asia/Kolkata"] {
            assert!(jiff::tz::TimeZone::get(z).is_ok(), "the validator must accept {z}");
        }
    }
}
