//! A reimplementation of Pandora's client protocol.
//!
//! This is not a wrapper around Pandora's web player — it speaks Pandora's own APIs directly:
//!
//! - [`tuner`] — the device/partner JSON API (`tuner.pandora.com`). Blowfish-encrypted, no bot
//!   wall. This is where we log in.
//! - [`rest`] — the modern JSON API the pandora.com web player itself calls. Richer surface, but
//!   its credentialed login sits behind PerimeterX bot detection, so we borrow a token instead.
//!
//! See `plans/pandora-native-client.md` for the protocol research this is built on.

pub mod crypto;
pub mod rest;
pub mod tuner;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("could not parse response: {0}")]
    Parse(#[from] serde_json::Error),

    /// An error Pandora reported in its own envelope. Notable codes: 1001 invalid auth token,
    /// 1002 invalid credentials, 9 parameter missing, 13 invalid partner login.
    #[error("pandora error {code}: {message}")]
    Api { code: u64, message: String },

    /// The response was well-formed HTTP but not what the protocol says it should be.
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Pandora's auth tokens expire; callers should re-login rather than surfacing this.
    pub fn is_auth_expired(&self) -> bool {
        matches!(self, Error::Api { code: 1001, .. })
    }

    /// Wrong username or password — re-logging in will not help.
    pub fn is_bad_credentials(&self) -> bool {
        matches!(self, Error::Api { code: 1002, .. })
    }
}
