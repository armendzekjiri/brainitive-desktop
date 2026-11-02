//! Refresh tokens in the OS keychain (macOS Keychain, Windows Credential Manager, Secret
//! Service on Linux) — one entry per server origin, so switching servers keeps the other
//! sign-ins. Access tokens never leave memory; nothing goes to disk, localStorage or the webview.
//!
//! The calls block (the OS may show a prompt), so callers run them on a blocking thread.

use crate::server::ServerOrigin;

const SERVICE: &str = "app.brainitive.desktop";

#[derive(Debug, thiserror::Error)]
#[error("The system keychain is not available: {0}")]
pub struct KeychainError(String);

#[cfg_attr(test, allow(dead_code))]
fn entry(origin: &ServerOrigin) -> Result<keyring::Entry, KeychainError> {
    // account = the origin, e.g. "https://www.brainitive.app"
    keyring::Entry::new(SERVICE, origin.as_str()).map_err(|e| KeychainError(e.to_string()))
}

pub fn load_refresh_token(origin: &ServerOrigin) -> Result<Option<String>, KeychainError> {
    #[cfg(test)]
    return fake::load(origin);
    #[cfg(not(test))]
    match entry(origin)?.get_password() {
        Ok(token) if token.starts_with("brr_") => Ok(Some(token)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeychainError(e.to_string())),
    }
}

pub fn store_refresh_token(origin: &ServerOrigin, token: &str) -> Result<(), KeychainError> {
    #[cfg(test)]
    return fake::store(origin, token);
    #[cfg(not(test))]
    entry(origin)?
        .set_password(token)
        .map_err(|e| KeychainError(e.to_string()))
}

pub fn delete_refresh_token(origin: &ServerOrigin) -> Result<(), KeychainError> {
    #[cfg(test)]
    return fake::delete(origin);
    #[cfg(not(test))]
    match entry(origin)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(KeychainError(e.to_string())),
    }
}

/// Unit tests never touch the real keychain: an in-memory store per origin, with injectable
/// failures (the sign-out tests make a delete fail).
#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::KeychainError;
    use crate::server::ServerOrigin;

    static ENTRIES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    /// Origin → how many deletes still fail.
    static FAIL_DELETES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

    pub fn load(origin: &ServerOrigin) -> Result<Option<String>, KeychainError> {
        Ok(peek(origin).filter(|t| t.starts_with("brr_")))
    }

    pub fn store(origin: &ServerOrigin, token: &str) -> Result<(), KeychainError> {
        ENTRIES
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(origin.as_str().to_string(), token.to_string());
        Ok(())
    }

    pub fn delete(origin: &ServerOrigin) -> Result<(), KeychainError> {
        {
            let mut fails = FAIL_DELETES.lock().unwrap();
            let left = fails
                .get_or_insert_with(HashMap::new)
                .entry(origin.as_str().to_string())
                .or_insert(0);
            if *left > 0 {
                *left -= 1;
                return Err(KeychainError("the keychain refused the deletion".into()));
            }
        }
        ENTRIES
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .remove(origin.as_str());
        Ok(())
    }

    /// What is stored, without the `brr_` check.
    pub fn peek(origin: &ServerOrigin) -> Option<String> {
        ENTRIES
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .get(origin.as_str())
            .cloned()
    }

    /// The next `times` deletes for `origin` fail.
    pub fn fail_deletes(origin: &ServerOrigin, times: u32) {
        FAIL_DELETES
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(origin.as_str().to_string(), times);
    }
}
