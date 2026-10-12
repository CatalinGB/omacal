//! The read bodies: stored events in, view payloads out.
//!
//! Pool-only — no `AppHandle`, no clock beyond `now_ms`. `src-tauri`'s
//! `#[tauri::command]` wrappers call these; `omacal-server` calls them
//! directly. Moved out of `lib.rs` so the assembly is reachable without Tauri.

use sqlx::SqlitePool;

use crate::commands;
use crate::settings;

/// The display zone: the user's setting if present, otherwise the system zone.
/// Every day boundary in the week grid is computed against this.
///
/// **The process zone, not the setting** — as the original did: the display-zone
/// setting works by exporting `TZ` at launch, so a server must set `TZ` in its
/// container for this to name the right zone.
pub fn display_tz(_pool: &SqlitePool) -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_string()
}

/// `pad` on every day-grid command (2026-09-03): that many days either side
/// of the window asked for, in the same payload, so the grid can slide into
/// them under a finger before a fetch for the new window lands. Bounded so a
/// malformed request cannot widen one view into an unbounded query.
pub const MAX_PAD_DAYS: u8 = 14;

pub fn checked_pad(pad: Option<u8>) -> Result<usize, String> {
    let pad = pad.unwrap_or(0);
    if pad > MAX_PAD_DAYS {
        return Err(format!("padding is at most {MAX_PAD_DAYS} days"));
    }
    Ok(pad as usize)
}

/// `day_count` days from `day_start_ms`, with `pad` more on either side —
/// see [`MAX_PAD_DAYS`]. The padded start is walked back in the display zone,
/// so the window the caller named begins exactly `pad` columns in.
pub async fn get_days(
    pool: &SqlitePool,
    day_start_ms: i64,
    day_count: usize,
    pad: usize,
) -> Result<commands::WeekPayload, String> {
    let tz = display_tz(pool);
    let start_ms = if pad == 0 {
        day_start_ms
    } else {
        commands::day_start_shifted(day_start_ms, -(pad as i64), &tz)
    };
    let n = day_count + 2 * pad;
    // Widen by a day either side so an event that begins just before the range
    // (or a DST-lengthened final day) is not missed.
    const DAY: i64 = 24 * 3_600_000;
    let events = omacal_store::events_in_window(
        pool,
        start_ms - DAY,
        start_ms + (n as i64 + 1) * DAY,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(if settings::combine_identical_events(pool).await {
        commands::assemble_days_combining(&events, start_ms, n, &tz, true)
    } else if n == 7 {
        commands::assemble_week(&events, start_ms, &tz)
    } else {
        commands::assemble_days(&events, start_ms, n, &tz)
    })
}

pub async fn get_month(
    pool: &SqlitePool,
    year: i32,
    month: u32,
) -> Result<commands::MonthPayload, String> {
    let tz = display_tz(pool);
    // Read once and passed to both, deliberately: `month_grid_start_ms` sizes
    // the fetch window and `assemble_month` recomputes the same anchor, so two
    // separate reads could straddle a settings change and leave the window
    // short of the grid by up to six days.
    let week_start = settings::week_start(pool).await;
    let grid_start_ms = commands::month_grid_start_ms(year, month, &tz, week_start);
    // Same widening as `get_week`/`get_day`, for the same reason: an event
    // that begins just before the 42-day grid, or a DST-lengthened edge day,
    // must not be missed.
    const DAY: i64 = 24 * 3_600_000;
    let events = omacal_store::events_in_window(
        pool,
        grid_start_ms - DAY,
        grid_start_ms + 43 * DAY,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(if settings::combine_identical_events(pool).await {
        commands::assemble_month_combining(&events, year, month, &tz, week_start, true)
    } else {
        commands::assemble_month(&events, year, month, &tz, week_start)
    })
}

pub async fn get_year(pool: &SqlitePool, year: i32) -> Result<commands::YearPayload, String> {
    let tz = display_tz(pool);
    let week_start = settings::week_start(pool).await;
    let year_start_ms = commands::year_start_ms(year, &tz);
    let next_year_start_ms = commands::year_start_ms(year + 1, &tz);
    // Same widening as `get_week`/`get_day`/`get_month`, for the same reason:
    // an event that begins just before the year, or a DST-lengthened edge
    // day, must not be missed.
    const DAY: i64 = 24 * 3_600_000;
    let events = omacal_store::events_in_window(
        pool,
        year_start_ms - DAY,
        next_year_start_ms + DAY,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(commands::assemble_year(&events, year, crate::now_ms(), &tz, week_start))
}

pub async fn get_big_year(pool: &SqlitePool, year: i32) -> Result<commands::BigYearPayload, String> {
    let tz = display_tz(pool);
    let week_start = settings::week_start(pool).await;
    get_big_year_impl(pool, year, &tz, crate::now_ms(), week_start).await
}

/// The body of `get_big_year`, minus the clock and settings reads, so a test
/// can hand it fixed ones.
pub async fn get_big_year_impl(
    pool: &SqlitePool,
    year: i32,
    tz: &str,
    now: i64,
    week_start: settings::WeekStart,
) -> Result<commands::BigYearPayload, String> {
    let ribbon_start_ms = commands::big_year_start_ms(year, tz, week_start);
    // Same widening as the four above, for the same reason: an event that
    // begins just before the 392-day ribbon, or a DST-lengthened edge day,
    // must not be missed.
    const DAY: i64 = 24 * 3_600_000;
    let events = omacal_store::events_in_window(
        pool,
        ribbon_start_ms - DAY,
        ribbon_start_ms + 393 * DAY,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(if settings::combine_identical_events(pool).await {
        commands::assemble_big_year_combining(&events, year, now, tz, week_start, true)
    } else {
        commands::assemble_big_year(&events, year, now, tz, week_start)
    })
}
