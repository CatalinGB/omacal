//! The sync timestamp, shared by the desktop's status read and the web
//! server's.
//!
//! `read_status` (the full `get_status` payload) stays desktop-side because it
//! carries desktop-only fields; the one number the sync ticker and the server's
//! `get_status` both need lives here, where neither has to depend on
//! `src-tauri`.

use sqlx::SqlitePool;

const LAST_SYNC_KEY: &str = "last_sync_ms";

/// When the last successful sync was recorded, and nothing else.
///
/// The background loop wants this one number to decide whether a sync is due.
/// It has no use for the account list, and — more to the point — no honest
/// value for either of `read_status`'s flags.
pub async fn last_sync_ms(pool: &SqlitePool) -> anyhow::Result<Option<i64>> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?1")
            .bind(LAST_SYNC_KEY)
            .fetch_optional(pool)
            .await?;
    Ok(raw.and_then(|v| v.parse().ok()))
}

pub async fn record_sync(pool: &SqlitePool, at_ms: i64) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(LAST_SYNC_KEY)
    .bind(at_ms.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fresh_database_has_no_sync_recorded() {
        let pool = omacal_store::connect_memory().await.unwrap();
        assert_eq!(last_sync_ms(&pool).await.unwrap(), None);
    }

    #[tokio::test]
    async fn recording_a_sync_round_trips() {
        let pool = omacal_store::connect_memory().await.unwrap();
        record_sync(&pool, 1_785_715_200_000).await.unwrap();
        assert_eq!(last_sync_ms(&pool).await.unwrap(), Some(1_785_715_200_000));
    }

    #[tokio::test]
    async fn recording_a_sync_twice_keeps_the_latest() {
        let pool = omacal_store::connect_memory().await.unwrap();
        record_sync(&pool, 1_000).await.unwrap();
        record_sync(&pool, 2_000).await.unwrap();
        assert_eq!(last_sync_ms(&pool).await.unwrap(), Some(2_000));
    }
}
