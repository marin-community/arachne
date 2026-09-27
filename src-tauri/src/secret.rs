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
pub fn save(token: &str) -> Result<(), String> {
    save_for(ACCOUNT, token)
}

fn save_for(account: &str, token: &str) -> Result<(), String> {
    let e = entry(account).map_err(|e| format!("keychain: {e}"))?;
    if token.is_empty() {
        // Delete is not an error when absent.
        let _ = e.delete_credential();
        return Ok(());
    }
    e.set_password(token).map_err(|e| format!("keychain: {e}"))
}

/// Read the token from the Keychain; `Ok(None)` when none is stored.
pub fn load() -> Result<Option<String>, String> {
    load_for(ACCOUNT)
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
