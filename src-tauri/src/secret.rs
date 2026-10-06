//! Keychain-backed storage for the loom bearer token.
//!
//! Spec: "Do not store sensitive credentials in frontend localStorage." The
//! remote-loom token is a personal API credential, so it lives in the macOS
//! Keychain via the `keyring` crate. The frontend only ever holds it in memory
//! (read once at boot, passed to `connect` on save). Keychain ops block, so
//! they run on the tokio blocking pool.

use keyring::Entry;

const SERVICE: &str = "com.arachne.app";
const ACCOUNT: &str = "loom-token";

fn entry(account: &str) -> keyring::Result<Entry> {
    Entry::new(SERVICE, account)
}

/// Persist the token to the Keychain. An empty string deletes the entry (so
/// clearing the field in settings actually clears the secret).
pub fn save(base_url: &str, token: &str) -> Result<(), String> {
    save_for(&account_for_url(base_url)?, token)
}

fn account_for_url(base_url: &str) -> Result<String, String> {
    let mut url =
        reqwest::Url::parse(base_url.trim()).map_err(|e| format!("invalid server URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "server URL must use HTTP(S) without embedded credentials, query, or fragment".into(),
        );
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(format!("loom-token:{}", url.as_str().trim_end_matches('/')))
}

fn save_for(account: &str, token: &str) -> Result<(), String> {
    let e = entry(account).map_err(|e| format!("keychain: {e}"))?;
    if token.is_empty() {
        // Delete is not an error when absent.
        return match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("keychain: {error}")),
        };
    }
    e.set_password(token).map_err(|e| format!("keychain: {e}"))
}

/// Read the token from the Keychain; `Ok(None)` when none is stored.
pub fn load(base_url: &str, migrate_legacy: bool) -> Result<Option<String>, String> {
    let account = account_for_url(base_url)?;
    let token = load_for(&account)?;
    if token.is_some() || !migrate_legacy {
        return Ok(token);
    }
    // Only the startup caller can opt in, using its persisted current URL.
    // Saved-server selection never falls back to another server's credential.
    let legacy = load_for(ACCOUNT)?;
    if let Some(ref token) = legacy {
        save_for(&account, token)?;
        save_for(ACCOUNT, "")?;
    }
    Ok(legacy)
}

fn load_for(account: &str) -> Result<Option<String>, String> {
    let e = entry(account).map_err(|e| format!("keychain: {e}"))?;
    match e.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keychain: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_accounts_are_normalized_and_isolated() {
        assert_eq!(
            account_for_url("HTTP://LOCALHOST:80/").unwrap(),
            account_for_url("http://localhost").unwrap()
        );
        assert_ne!(
            account_for_url("http://localhost:7878").unwrap(),
            account_for_url("http://localhost:7879").unwrap()
        );
        assert_ne!(
            account_for_url("http://localhost/a").unwrap(),
            account_for_url("http://localhost/b").unwrap()
        );
        assert!(account_for_url("http://user:secret@localhost").is_err());
        assert!(account_for_url("file:///tmp/loom").is_err());
    }

    #[test]
    fn save_load_roundtrip() {
        // Round-trips through the real Keychain. This is a macOS dev machine;
        // if the Keychain is unavailable the test is skipped rather than
        // failing the build.
        let account = format!("loom-token-test-{}", std::process::id());
        if save_for(&account, "test-token-abc").is_err() {
            eprintln!("keychain unavailable; skipping");
            return;
        }
        assert_eq!(
            load_for(&account).unwrap().as_deref(),
            Some("test-token-abc")
        );
        // Empty deletes.
        save_for(&account, "").unwrap();
        assert_eq!(load_for(&account).unwrap(), None);
    }
}
