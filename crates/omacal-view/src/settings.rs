//! Settings vocabulary the view assembly needs. The settings *bodies*
//! (read/write, `AppSettings`) stay in `src-tauri` for now; this holds the
//! types the view layer depends on.

use serde::{Deserialize, Serialize};

/// The day a week begins on.
///
/// Three, not seven, and they are Google Calendar's own three — this is a
/// Google Calendar client, and a week starting on a Wednesday is a preference
/// no calendar this one syncs with can express. An enum for the same reason
/// `TimeFormat` is one: the set is closed, so `Setting::WeekStart` needs no
/// refusal path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeekStart {
    #[serde(rename = "monday")]
    Monday,
    #[serde(rename = "sunday")]
    Sunday,
    #[serde(rename = "saturday")]
    Saturday,
}

impl WeekStart {
    /// The stored spelling, which is also the wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            WeekStart::Monday => "monday",
            WeekStart::Sunday => "sunday",
            WeekStart::Saturday => "saturday",
        }
    }

    /// This day as jiff's own weekday, for the grid anchors that walk
    /// backwards to it.
    pub fn weekday(self) -> jiff::civil::Weekday {
        use jiff::civil::Weekday;
        match self {
            WeekStart::Monday => Weekday::Monday,
            WeekStart::Sunday => Weekday::Sunday,
            WeekStart::Saturday => Weekday::Saturday,
        }
    }

    /// How many blank cells precede a month whose 1st falls on `first` — the
    /// month grid's `lead_blanks`.
    ///
    /// Monday-zero throughout rather than jiff's two offset helpers chosen per
    /// variant: one origin, one subtraction, and the modulo does the wrapping.
    /// Mixing the two origins is how this arithmetic goes wrong.
    pub fn lead_blanks(self, first: jiff::civil::Weekday) -> usize {
        let day = first.to_monday_zero_offset() as usize;
        let start = self.weekday().to_monday_zero_offset() as usize;
        (day + 7 - start) % 7
    }

    /// Whether the column at `index` in a week-aligned row is a weekend day.
    ///
    /// Read off the *index*, never off the date the column carries — the
    /// property Big Year's 28-day rows exist to guarantee (see
    /// `every_row_puts_its_weekends_in_the_same_columns`). Note that only a
    /// Monday start puts Saturday and Sunday next to each other; the other two
    /// split the pair to the ends of the row, exactly as they do in every
    /// month grid those readers have ever used.
    ///
    /// Used by the Rust suite rather than by the app: the shading itself is
    /// drawn in the browser, from `weekstart.ts`'s own copy of this rule. That
    /// is exactly why this exists — `the_ribbons_weekend_stripes_stay_straight_under_every_start`
    /// asserts the two agree against real dates, so the copy cannot drift.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_weekend_column(self, index: usize) -> bool {
        use jiff::civil::Weekday;
        let start = self.weekday().to_monday_zero_offset() as usize;
        let weekday = (start + index) % 7;
        weekday == Weekday::Saturday.to_monday_zero_offset() as usize
            || weekday == Weekday::Sunday.to_monday_zero_offset() as usize
    }
}

/// One stored choice's raw value, or `None`.
async fn read(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

const WEEK_START_KEY: &str = "week_start";
const COMBINE_IDENTICAL_EVENTS_KEY: &str = "combine_identical_events";

/// The stored day a week begins on. Absent, garbage, or a future spelling
/// lands on Monday, the week omacal has always drawn.
pub async fn week_start(pool: &sqlx::SqlitePool) -> WeekStart {
    match read(pool, WEEK_START_KEY).await.as_deref() {
        Some("sunday") => WeekStart::Sunday,
        Some("saturday") => WeekStart::Saturday,
        _ => WeekStart::Monday,
    }
}

/// Whether events identical across calendars are drawn as one.
pub async fn combine_identical_events(pool: &sqlx::SqlitePool) -> bool {
    read(pool, COMBINE_IDENTICAL_EVENTS_KEY).await.as_deref() == Some("1")
}
