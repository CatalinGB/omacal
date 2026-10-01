//! The resize cursor tao leaves on the window, taken back off.
//!
//! WebKitGTK shows the page's plain arrow as *no cursor at all* — a null
//! `GdkCursor`, "the default cursor for the window" (`CursorGtk.cpp`) — so
//! wherever the page has nothing more specific to say, its view inherits
//! whatever cursor the toplevel window carries. Only tao ever puts one
//! there: for an undecorated window it paints the resize arrows itself, on
//! every pointer motion, from a 5px band round the edge (`event_loop.rs`,
//! "Allow resizing unmaximized non-fullscreen undecorated window"). But only
//! while the window is undecorated, resizable **and not maximized** — the
//! moment any of those stops holding, tao stops writing, and the last cursor
//! it wrote stays on the window for good.
//!
//! On Hyprland tao has stopped almost from the start: Hyprland marks every
//! window maximized when it maps ("this forces apps to not draw CSD",
//! `XDGShell.cpp`) and never clears it. So any `e-resize` tao wrote while
//! the window was briefly otherwise stays for the life of the process, and
//! every arrow in the page — most of the event form's labels and padding —
//! shows `→|` (Plamen, 2026-10-01, a window up for a week). Which moment
//! wrote it was not pinned down: a launch with the pointer parked in the
//! tile's right band did not reproduce it. A stale `e-resize` planted on
//! the window did, exactly, and this cleared it on the first motion.
//!
//! So once tao has stopped painting, the window's own cursor is cleared on
//! the next motion. Connected after tao's handler, so it runs after it for
//! the same event, and it never acts while tao is painting: on a floating
//! frameless window the edge arrows still work exactly as tao draws them.

/// Whether tao is the one painting the window's cursor — its own guard,
/// restated. Outside it tao writes nothing, so anything left on the window
/// is stale.
fn tao_paints_cursor(decorated: bool, resizable: bool, maximized: bool) -> bool {
    !decorated && resizable && !maximized
}

/// Wires the clearing handler onto the main window. Best effort, like
/// `pinch::install`: no main window means nothing to wire.
pub fn install(app: &tauri::AppHandle) {
    use gtk::prelude::*;
    use tauri::Manager;

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(window) = window.gtk_window() else {
        return;
    };
    window.connect_motion_notify_event(|window, _| {
        if !tao_paints_cursor(window.is_decorated(), window.is_resizable(), window.is_maximized()) {
            if let Some(gdk_window) = window.window().filter(|w| w.cursor().is_some()) {
                gdk_window.set_cursor(None);
            }
        }
        gtk::glib::Propagation::Proceed
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window tao draws arrows on: frameless, resizable, floating.
    /// Clearing there would wipe the arrows tao just drew.
    #[test]
    fn a_floating_frameless_window_is_left_to_tao() {
        assert!(tao_paints_cursor(false, true, false));
    }

    /// Hyprland's case: every tiled window is maximized, so tao has stopped
    /// and whatever it last drew is stale.
    #[test]
    fn a_maximized_window_is_not_tao_s() {
        assert!(!tao_paints_cursor(false, true, true));
    }

    /// A frame turned back on from the settings hands the edges to GTK's own
    /// border windows; tao has stopped there too.
    #[test]
    fn a_decorated_window_is_not_tao_s() {
        assert!(!tao_paints_cursor(true, true, false));
    }
}
