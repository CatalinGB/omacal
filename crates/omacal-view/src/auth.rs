//! Access tokens: the refresh-token cache and the exchange that backs it.
//!
//! The credential itself lives behind [`CredentialStore`] — the desktop's
//! keyring, a server's file — so this code has no opinion about where secrets
//! are kept.

use crate::state::{AppState, CachedTokens, CredentialStore};

/// Whether a cached entry can be used as-is.
///
/// Pulled out so the decision is testable: an untested condition guarding a
/// keychain read is what produced the prompt-every-few-minutes behaviour.
pub fn cached_is_usable(entry: Option<&CachedTokens>, now_ms: i64) -> bool {
    entry.is_some_and(|t| t.expires_at_ms > now_ms)
}

/// Stores a refresh token under `key`.
pub fn store_refresh_token(
    store: &dyn CredentialStore,
    key: &str,
    token: &str,
) -> anyhow::Result<()> {
    store.set(key, token)
}

/// The refresh token at `key`, or an error when none is stored.
pub fn load_refresh_token(store: &dyn CredentialStore, key: &str) -> anyhow::Result<String> {
    store
        .get(key)?
        .ok_or_else(|| anyhow::anyhow!("no stored refresh token for {key}"))
}

/// Returns a usable access token for `email`, reading the credential store and
/// refreshing only when the cached one is absent or expired.
///
/// This is the whole reason the cache exists: the keychain read is what raises
/// the macOS access prompt, and doing it on every sync made the app unusable.
pub async fn access_token_for(
    state: &AppState,
    store: &dyn CredentialStore,
    client_id: &str,
    client_secret: &str,
    email: &str,
) -> anyhow::Result<String> {
    {
        let cache = state.tokens.lock().await;
        if cached_is_usable(cache.get(email), crate::now_ms()) {
            return Ok(cache[email].access_token.clone());
        }
    } // lock dropped before the network call

    // Prefer a refresh token we already hold; only touch the store if we have
    // none for this account yet.
    let refresh_token = {
        let cache = state.tokens.lock().await;
        cache.get(email).map(|t| t.refresh_token.clone())
    };
    let refresh_token = match refresh_token {
        Some(rt) => rt,
        None => load_refresh_token(store, email)?,
    };

    let fresh = omacal_google::auth::refresh(
        omacal_google::auth::TOKEN_ENDPOINT,
        client_id,
        client_secret,
        &refresh_token,
    )
    .await?;

    let access_token = fresh.access_token.clone();
    let mut cache = state.tokens.lock().await;
    cache.insert(
        email.to_string(),
        CachedTokens {
            // Google omits refresh_token on refresh; keep the one we used.
            refresh_token: fresh.refresh_token.unwrap_or(refresh_token),
            access_token: fresh.access_token,
            expires_at_ms: fresh.expires_at_ms,
        },
    );
    Ok(access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(expires_at_ms: i64) -> CachedTokens {
        CachedTokens {
            refresh_token: "rt".into(),
            access_token: "at".into(),
            expires_at_ms,
        }
    }

    const NOW: i64 = 1_785_715_200_000;

    #[test]
    fn an_absent_entry_is_never_usable() {
        assert!(!cached_is_usable(None, NOW));
    }

    #[test]
    fn a_live_entry_is_usable_so_the_keychain_is_not_read() {
        assert!(cached_is_usable(Some(&cached(NOW + 60_000)), NOW));
    }

    #[test]
    fn an_expired_entry_is_not_usable() {
        assert!(!cached_is_usable(Some(&cached(NOW - 1)), NOW));
    }

    #[test]
    fn an_entry_expiring_exactly_now_is_not_usable() {
        // Strictly greater: a token expiring this millisecond is not worth a
        // request that will fail.
        assert!(!cached_is_usable(Some(&cached(NOW)), NOW));
    }
}
