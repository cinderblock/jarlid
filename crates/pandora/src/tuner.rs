//! Pandora's tuner ("partner") JSON API — `tuner.pandora.com/services/json/`.
//!
//! This is the protocol Pandora's own device clients speak, and the one pianobar/Pithos/pydora
//! have used for 15 years. Unlike the web REST API, it has no bot-detection wall in front of
//! login, which is why we authenticate here.
//!
//! Shape of a call:
//!   POST https://tuner.pandora.com/services/json/?method=<m>&partner_id=&auth_token=&user_id=
//!   body = blowfish_ecb_hex(json)   — except auth.partnerLogin, which is plaintext
//!
//! Every request body carries `syncTime`; the server rejects requests whose clock has drifted, so
//! we track the offset between our clock and theirs from the partner-login response onward.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::crypto::Codec;
use crate::{Error, Result};

const ENDPOINT: &str = "https://tuner.pandora.com/services/json/";

/// Credentials identifying a Pandora device client. These are not secrets — they ship inside
/// every copy of the corresponding official app and have been published for over a decade.
pub struct Partner {
    pub username: &'static str,
    pub password: &'static str,
    pub device_model: &'static str,
    pub version: &'static str,
    pub encrypt_key: &'static str,
    pub decrypt_key: &'static str,
}

/// The Android partner. Chosen because it is the most widely exercised (pianobar's default), so
/// it is the least likely to be quietly retired.
pub const ANDROID: Partner = Partner {
    username: "android",
    password: "AC7IBG09A3DTSYM4R41UJWL07VLN8JI7",
    device_model: "android-generic",
    version: "5",
    encrypt_key: "6#26FRL$ZWD",
    decrypt_key: "R=U!LH$O2B#",
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartnerLogin {
    partner_id: String,
    partner_auth_token: String,
    sync_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserLogin {
    user_id: Option<String>,
    user_auth_token: String,
}

/// An authenticated tuner-API session.
pub struct Session {
    http: reqwest::Client,
    encrypt: Codec,
    decrypt: Codec,
    partner: &'static Partner,

    partner_id: String,
    partner_auth_token: String,
    user_id: Option<String>,
    user_auth_token: Option<String>,

    /// Server clock at partner-login, plus how long ago that was — together these reconstruct
    /// "what time does Pandora think it is right now".
    server_sync_time: u64,
    synced_at: Instant,
}

impl Session {
    /// Perform `auth.partnerLogin`. This establishes the device identity and the clock offset;
    /// it does not involve the user's account at all.
    pub async fn connect(partner: &'static Partner) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let mut session = Self {
            http,
            encrypt: Codec::new(partner.encrypt_key),
            decrypt: Codec::new(partner.decrypt_key),
            partner,
            partner_id: String::new(),
            partner_auth_token: String::new(),
            user_id: None,
            user_auth_token: None,
            server_sync_time: 0,
            synced_at: Instant::now(),
        };

        let body = json!({
            "username": partner.username,
            "password": partner.password,
            "deviceModel": partner.device_model,
            "version": partner.version,
            "includeUrls": true,
        });

        let sent_at = Instant::now();
        let result = session.raw_call("auth.partnerLogin", body, false, &[]).await?;
        let login: PartnerLogin = serde_json::from_value(result)?;

        session.server_sync_time = session
            .decrypt
            .decrypt_sync_time(&login.sync_time)
            .ok_or_else(|| Error::Protocol("could not decrypt syncTime".into()))?;
        session.synced_at = sent_at;
        session.partner_id = login.partner_id;
        session.partner_auth_token = login.partner_auth_token;

        Ok(session)
    }

    /// Perform `auth.userLogin` with real account credentials.
    pub async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        let partner_auth_token = self.partner_auth_token.clone();
        let partner_id = self.partner_id.clone();

        let body = json!({
            "loginType": "user",
            "username": username,
            "password": password,
            "partnerAuthToken": partner_auth_token,
            "includePandoraOneInfo": true,
            "includeSubscriptionExpiration": true,
            "includeAdAttributes": true,
            "returnCapped": true,
        });

        // User login is the one authenticated call that keys off partner_id rather than user_id.
        let result = self
            .raw_call(
                "auth.userLogin",
                body,
                true,
                &[("partner_id", &partner_id), ("auth_token", &partner_auth_token)],
            )
            .await?;

        let login: UserLogin = serde_json::from_value(result.clone())?;
        self.user_id = login.user_id;
        self.user_auth_token = Some(login.user_auth_token);
        Ok(())
    }

    /// Call any authenticated method. Adds the user token and current sync time automatically.
    pub async fn call(&self, method: &str, mut body: Value) -> Result<Value> {
        let token = self
            .user_auth_token
            .as_ref()
            .ok_or_else(|| Error::Protocol("not logged in".into()))?;

        if let Some(obj) = body.as_object_mut() {
            obj.insert("userAuthToken".into(), json!(token));
        }

        let mut params: Vec<(&str, &str)> =
            vec![("partner_id", &self.partner_id), ("auth_token", token)];
        if let Some(user_id) = &self.user_id {
            params.push(("user_id", user_id));
        }

        self.raw_call(method, body, true, &params).await
    }

    /// The token the *web* REST API might also accept as `X-AuthToken`.
    pub fn user_auth_token(&self) -> Option<&str> {
        self.user_auth_token.as_deref()
    }

    /// Pandora's current clock: their time at sync, plus our elapsed time since.
    fn sync_time(&self) -> u64 {
        self.server_sync_time + self.synced_at.elapsed().as_secs()
    }

    /// The single request primitive. `encrypted` is false only for `auth.partnerLogin`.
    async fn raw_call(
        &self,
        method: &str,
        body: Value,
        encrypted: bool,
        extra_params: &[(&str, &str)],
    ) -> Result<Value> {
        let mut body = body;
        if encrypted {
            // Every encrypted request must carry syncTime or the server rejects it outright.
            if let Some(obj) = body.as_object_mut() {
                obj.insert("syncTime".into(), json!(self.sync_time()));
            }
        }

        let payload = serde_json::to_vec(&body)?;
        let payload = if encrypted {
            self.encrypt.encrypt(&payload)
        } else {
            String::from_utf8(payload).expect("serde_json emits valid UTF-8")
        };

        let mut params: Vec<(&str, &str)> = vec![("method", method)];
        params.extend_from_slice(extra_params);

        let response = self
            .http
            .post(ENDPOINT)
            .query(&params)
            .header("Content-Type", "text/plain")
            .header("User-Agent", self.user_agent())
            .body(payload)
            .send()
            .await?;

        let envelope: Map<String, Value> = response.json().await?;

        match envelope.get("stat").and_then(Value::as_str) {
            Some("ok") => Ok(envelope
                .get("result")
                .cloned()
                .unwrap_or(Value::Object(Map::new()))),
            _ => Err(Error::Api {
                code: envelope
                    .get("code")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                message: envelope
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string(),
            }),
        }
    }

    fn user_agent(&self) -> String {
        format!("pandora/{} ({})", self.partner.version, self.partner.device_model)
    }
}

