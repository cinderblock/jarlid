//! Pandora's modern web REST API — `www.pandora.com/api/v1/...`.
//!
//! This is what pandora.com's own player calls: 135 endpoints, far richer than the tuner API
//! (`action/*`, `playback/*`, `catalog/*`, `event/*` have no public documentation at all).
//!
//! We do not log in here. `POST /api/v1/auth/login` is fronted by PerimeterX bot detection and
//! answers a plain HTTP client with `403 s2s_high_score`. Every *other* endpoint is unguarded, so
//! the open question this module exists to answer is whether a token obtained from the tuner API
//! is accepted here as `X-AuthToken`.

use std::time::Duration;

use serde_json::Value;

use crate::{Error, Result};

const BASE: &str = "https://www.pandora.com/api/";

pub struct Client {
    http: reqwest::Client,
    csrf: String,
    auth_token: Option<String>,
}

impl Client {
    /// Fetch a CSRF token the way the web client does: a bare GET of the homepage sets a
    /// `csrftoken` cookie, which must then be echoed back in the `X-CsrfToken` header.
    ///
    /// Pandora does not actually validate that the two match, or that the value is one it issued —
    /// but we do it properly anyway so our traffic is indistinguishable from the real client.
    pub async fn connect() -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .cookie_store(true)
            .user_agent(WEB_USER_AGENT)
            .build()?;

        let response = http.get("https://www.pandora.com/").send().await?;

        let csrf = response
            .cookies()
            .find(|c| c.name() == "csrftoken")
            .map(|c| c.value().to_string())
            .unwrap_or_else(|| "abc123".into());

        Ok(Self {
            http,
            csrf,
            auth_token: None,
        })
    }

    /// Supply a token obtained elsewhere — e.g. from [`crate::tuner::Session::user_auth_token`].
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Call an endpoint, e.g. `("v1/station/getStations", json!({"pageSize": 250}))`.
    pub async fn call(&self, endpoint: &str, body: Value) -> Result<Value> {
        let mut request = self
            .http
            .post(format!("{BASE}{endpoint}"))
            .header("X-CsrfToken", &self.csrf)
            .header("Cookie", format!("csrftoken={}", self.csrf))
            .json(&body);

        if let Some(token) = &self.auth_token {
            request = request.header("X-AuthToken", token);
        }

        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;

        if status.is_success() {
            return serde_json::from_str(&text).map_err(Error::from);
        }

        // Errors come back as a JSON envelope; PerimeterX blocks use the same shape with
        // errorCode 1215, which is how we can tell a bot wall from a genuine auth failure.
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Err(Error::Api {
            code: parsed
                .get("errorCode")
                .and_then(Value::as_u64)
                .unwrap_or(status.as_u16() as u64),
            message: parsed
                .get("errorString")
                .or_else(|| parsed.get("message"))
                .and_then(Value::as_str)
                .unwrap_or(&text)
                .to_string(),
        })
    }

    /// A free, anonymous listener token. Useful for exercising the API without touching the
    /// user's account — this is the one auth endpoint PerimeterX does not guard.
    pub async fn anonymous_login(&mut self) -> Result<Value> {
        let result = self.call("v1/auth/anonymousLogin", serde_json::json!({})).await?;
        if let Some(token) = result.get("authToken").and_then(Value::as_str) {
            self.auth_token = Some(token.to_string());
        }
        Ok(result)
    }
}

/// PerimeterX scores unusual clients as bots, so present as a current browser.
const WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
