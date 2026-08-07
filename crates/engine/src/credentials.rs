//! Pandora credentials in the Windows Credential Manager.
//!
//! The password is encrypted per-user by the OS, so it is not readable by other accounts and does
//! not sit in a file next to the binary. The `.env` path used by the probes and examples is a
//! development convenience only — the app must never read credentials that way.
//!
//! Username and password are stored together as one JSON blob under a single entry so they cannot
//! drift out of sync (a stored password belonging to a different username is a confusing failure).

use serde::{Deserialize, Serialize};

/// Shown to the user in Windows' credential UI, so it should be recognisable.
const SERVICE: &str = "Jarlid (Pandora)";
const ACCOUNT: &str = "pandora-login";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("credential store unavailable: {0}")]
    Store(#[from] keyring::Error),

    #[error("stored credentials were malformed: {0}")]
    Malformed(#[from] serde_json::Error),
}

type Result<T> = std::result::Result<T, Error>;

fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, ACCOUNT)?)
}

/// Save credentials, replacing anything already stored.
pub fn store(username: &str, password: &str) -> Result<()> {
    let blob = serde_json::to_string(&Credentials {
        username: username.to_string(),
        password: password.to_string(),
    })?;
    entry()?.set_password(&blob)?;
    Ok(())
}

/// Load saved credentials, or `Ok(None)` if the user has not logged in yet.
///
/// A missing entry is a normal first-run state, not an error; anything else is surfaced so a
/// genuine credential-store problem isn't silently mistaken for "please log in again".
pub fn load() -> Result<Option<Credentials>> {
    match entry()?.get_password() {
        Ok(blob) => Ok(Some(serde_json::from_str(&blob)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Forget the saved credentials — the "sign out" path.
pub fn clear() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Whether credentials are saved, without decrypting them.
pub fn exists() -> bool {
    matches!(load(), Ok(Some(_)))
}
