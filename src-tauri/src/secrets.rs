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

fn entry(origin: &ServerOrigin) -> Result<keyring::Entry, KeychainError> {
    // account = the origin, e.g. "https://www.brainitive.app"
    keyring::Entry::new(SERVICE, origin.as_str()).map_err(|e| KeychainError(e.to_string()))
}

pub fn load_refresh_token(origin: &ServerOrigin) -> Result<Option<String>, KeychainError> {
    match entry(origin)?.get_password() {
        Ok(token) if token.starts_with("brr_") => Ok(Some(token)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeychainError(e.to_string())),
    }
}

pub fn store_refresh_token(origin: &ServerOrigin, token: &str) -> Result<(), KeychainError> {
    entry(origin)?
        .set_password(token)
        .map_err(|e| KeychainError(e.to_string()))
}

pub fn delete_refresh_token(origin: &ServerOrigin) -> Result<(), KeychainError> {
    match entry(origin)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(KeychainError(e.to_string())),
    }
}
