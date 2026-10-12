//! The calendar-list setters: visibility, colour, sync and label overrides.
//!
//! Pool-only. The desktop wrapper adds the tray/menubar refresh a headless
//! server has no need of; everything a mutation does to the store is here.

use omacal_store::CalendarRow;
use sqlx::SqlitePool;

use crate::errors::user_facing;

pub async fn list(pool: &SqlitePool) -> Result<Vec<CalendarRow>, String> {
    omacal_store::list_calendars(pool).await.map_err(|e| user_facing(&e))
}

pub async fn set_selected(pool: &SqlitePool, id: i64, on: bool) -> Result<(), String> {
    omacal_store::set_selected(pool, id, on).await.map_err(|e| user_facing(&e))
}

/// Sets or clears a calendar's colour, **locally and only locally**.
///
/// `hex` of `None` clears the override and the calendar follows the provider's
/// own colour again — a different state from setting it to whatever the
/// provider happens to use today, which would silently stop following it.
pub async fn set_color(pool: &SqlitePool, id: i64, hex: Option<&str>) -> Result<(), String> {
    omacal_store::set_color_override(pool, id, hex).await.map_err(|e| user_facing(&e))
}

/// Returns how many events were removed, so the interface can say what
/// happened.
pub async fn set_sync(pool: &SqlitePool, id: i64, on: bool) -> Result<u64, String> {
    omacal_store::set_sync_enabled(pool, id, on).await.map_err(|e| user_facing(&e))
}

pub async fn set_label(pool: &SqlitePool, id: i64, label: Option<&str>) -> Result<(), String> {
    omacal_store::set_label_override(pool, id, label).await.map_err(|e| user_facing(&e))
}
