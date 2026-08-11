//! One-click bug reports: gather context, scrub secrets, open a prefilled GitHub issue.
//!
//! **Redaction is the point of this module, not a nicety.** A report is assembled from error
//! strings and playback state, and those routinely contain live credentials: Pandora's audio URLs
//! carry a signed `token=` that plays the track for anyone holding it, and auth tokens appear in
//! request errors. Everything that leaves here goes through [`redact`] first, and the user sees
//! the finished text in GitHub's editor before anything is submitted.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

const REPO: &str = "cinderblock/jarlid";

/// How many recent problems to keep. Enough to show a pattern, few enough to stay readable.
const HISTORY: usize = 12;

/// Cap on the **encoded** URL, which is what actually has to survive the browser and GitHub.
///
/// Percent-encoding roughly triples the body — every newline becomes `%0A`, every space `%20` —
/// so a limit on the raw text is close to meaningless. Capping the raw body at 6000 produced
/// URLs near 18000 characters, past what browsers reliably open, and the failure mode is a
/// report button that appears to do nothing.
const MAX_URL: usize = 7000;

/// A problem worth reporting, as it happened.
#[derive(Debug, Clone, Serialize)]
pub struct Incident {
    /// Local time, formatted for a human reading the issue later.
    pub at: String,
    /// Where it came from: "engine", "ui", "panic".
    pub source: String,
    pub message: String,
}

#[derive(Default)]
pub struct Diagnostics {
    incidents: Mutex<VecDeque<Incident>>,
}

impl Diagnostics {
    pub fn record(&self, source: &str, message: &str) {
        let incident = Incident {
            at: timestamp(),
            source: source.to_string(),
            message: redact(message),
        };
        let mut incidents = self.incidents.lock().unwrap_or_else(|e| e.into_inner());
        if incidents.len() >= HISTORY {
            incidents.pop_front();
        }
        incidents.push_back(incident);
    }

    pub fn recent(&self) -> Vec<Incident> {
        self.incidents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

/// Seconds since the epoch is useless in a bug report; a readable local time is not.
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Deliberately not pulling in a date crate for this: seconds-of-day is enough to correlate
    // events with each other, which is all a reader needs.
    let seconds = now % 86_400;
    format!("{:02}:{:02}:{:02}Z", seconds / 3600, (seconds % 3600) / 60, seconds % 60)
}

/// Strip anything that could authenticate as the user.
///
/// Conservative by design: it would rather blank something harmless than leak a token. The
/// patterns cover what Pandora actually puts in URLs and errors — a signed `token=` on every audio
/// URL, `auth_token`/`X-AuthToken` on API calls, `lid=` listener ids, and email addresses.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    // Query/header style `key=value` and `key: value` pairs.
    const SECRET_KEYS: [&str; 8] = [
        "token",
        "auth_token",
        "authtoken",
        "x-authtoken",
        "userauthtoken",
        "partnerauthtoken",
        "password",
        "lid",
    ];

    'outer: while !rest.is_empty() {
        for key in SECRET_KEYS {
            // Case-insensitive match at the current position, followed by = or :
            if rest.len() >= key.len() && rest[..key.len()].eq_ignore_ascii_case(key) {
                let after = &rest[key.len()..];
                let sep = after.starts_with('=') || after.starts_with(':');
                // Only treat it as a pair when the key stands alone, so "tokens" or a word
                // ending in "lid" (like "invalid") isn't mangled.
                let boundary_ok = out
                    .chars()
                    .last()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
                if sep && boundary_ok {
                    out.push_str(key);
                    out.push_str(&after[..1]);

                    // Header form is `Key: value` — skip the space *before* looking for the end
                    // of the value, or the very first character terminates the scan and the
                    // secret is copied through verbatim. (It did.)
                    let value = after[1..].trim_start_matches(' ');
                    out.push_str("REDACTED");

                    let end = value
                        .find(['&', ' ', '"', '\'', ',', '}', '\n'])
                        .unwrap_or(value.len());
                    rest = &value[end..];
                    continue 'outer;
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }

    redact_emails(&out)
}

/// Replace email addresses, which are the account identifier here.
fn redact_emails(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_inclusive(|c: char| c.is_whitespace()) {
        let trimmed = word.trim_end();
        let at = trimmed.find('@');
        let looks_like_email = at.is_some_and(|i| {
            i > 0 && trimmed[i + 1..].contains('.') && !trimmed[i + 1..].contains('@')
        });
        if looks_like_email {
            out.push_str("<email redacted>");
            out.push_str(&word[trimmed.len()..]);
        } else {
            out.push_str(word);
        }
    }
    out
}

/// Context the UI supplies, since it knows things the backend doesn't (what's on screen, the
/// WebView2 user agent, whether a remote player is driving).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiContext {
    pub user_agent: String,
    pub station: String,
    pub source_station: String,
    pub mode: String,
    pub remote: bool,
    /// What the user was doing, if they typed anything.
    pub note: String,
}

/// Build the issue body. Public for tests — the redaction guarantees are what matter here.
pub fn build_body(
    app_version: &str,
    context: &UiContext,
    playback: &str,
    incidents: &[Incident],
) -> String {
    let mut body = String::new();

    body.push_str("<!-- Generated by Jarlid. Please check it over before submitting. -->\n\n");

    if !context.note.trim().is_empty() {
        body.push_str("### What happened\n\n");
        body.push_str(&redact(context.note.trim()));
        body.push_str("\n\n");
    } else {
        body.push_str("### What happened\n\n_Describe what you were doing._\n\n");
    }

    body.push_str("### Environment\n\n");
    body.push_str(&format!("- Jarlid: {app_version}\n"));
    body.push_str(&format!("- OS: {}\n", std::env::consts::OS));
    if !context.user_agent.is_empty() {
        body.push_str(&format!("- WebView2: {}\n", redact(&context.user_agent)));
    }

    body.push_str("\n### State\n\n");
    if !context.station.is_empty() {
        body.push_str(&format!("- Station: {}\n", redact(&context.station)));
    }
    if !context.source_station.is_empty() {
        body.push_str(&format!("- Playing from: {}\n", redact(&context.source_station)));
    }
    if !context.mode.is_empty() {
        body.push_str(&format!("- Mode: {}\n", redact(&context.mode)));
    }
    if context.remote {
        body.push_str("- Remote (network player) mode: yes\n");
    }
    if !playback.is_empty() {
        body.push_str(&format!("- Playback: {playback}\n"));
    }

    body.push_str("\n### Recent problems\n\n");
    if incidents.is_empty() {
        body.push_str("_None recorded._\n");
    } else {
        body.push_str("```\n");
        for incident in incidents {
            body.push_str(&format!(
                "{} [{}] {}\n",
                incident.at, incident.source, incident.message
            ));
        }
        body.push_str("```\n");
    }

    body
}

/// Trim the body until the finished URL fits, keeping the beginning — the description and
/// environment are worth more than the oldest incident.
fn fit_url(title: &str, body: &str) -> String {
    let mut end = body.len();
    loop {
        let url = raw_issue_url(title, &body[..end]);
        if url.len() <= MAX_URL || end == 0 {
            return url;
        }
        // Step back proportionally to the overshoot, then to a character boundary so the next
        // slice stays valid.
        end = end.saturating_sub(((url.len() - MAX_URL) / 3).max(64));
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
    }
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn raw_issue_url(title: &str, body: &str) -> String {
    format!(
        "https://github.com/{REPO}/issues/new?title={}&body={}",
        encode(title),
        encode(body)
    )
}

/// Assemble the prefilled issue URL, trimmed to something a browser will actually open.
pub fn issue_url(title: &str, body: &str) -> String {
    let url = raw_issue_url(title, body);
    if url.len() <= MAX_URL {
        return url;
    }
    fit_url(title, &format!("{body}

_…truncated._
"))
}

#[tauri::command]
pub async fn native_report_issue(
    app: AppHandle,
    context: UiContext,
) -> Result<String, String> {
    let version = app.package_info().version.to_string();

    // Playback detail, when the engine is running. Never the audio URL: it is a live credential.
    let playback = match app.state::<crate::native::NativeEngine>().engine().await {
        Ok(engine) => format!(
            // `starved` is the one a dropout report turns on: it is silence the device actually
            // played because decoding fell behind, so a non-zero value settles "is it the app or
            // is it my machine" without anyone having to reproduce it live.
            "position {:.0}s, buffered {:.1}s, drift {:.2}s, starved {:.2}s{}",
            engine.position().as_secs_f64(),
            engine.buffered().as_secs_f64(),
            engine.drift().as_secs_f64(),
            engine.starved().as_secs_f64(),
            if engine.is_paused() { ", paused" } else { "" }
        ),
        Err(_) => "engine not running".to_string(),
    };

    let diagnostics = app.state::<Diagnostics>();
    let incidents = diagnostics.recent();

    let title = incidents
        .last()
        .map(|i| {
            let first_line = i.message.lines().next().unwrap_or("").trim();
            format!("{}: {}", i.source, truncate_title(first_line))
        })
        .unwrap_or_else(|| "Bug report".to_string());

    let body = build_body(&version, &context, &playback, &incidents);
    let url = issue_url(&title, &body);

    // Open in the real browser. GitHub shows the prefilled form, so the user reviews and edits
    // before anything is submitted — we never post on their behalf.
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())?;

    Ok(url)
}

fn truncate_title(text: &str) -> String {
    const MAX: usize = 90;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let clipped: String = text.chars().take(MAX).collect();
    format!("{clipped}…")
}

/// Record a problem from the UI (an uncaught error, a failed command).
#[tauri::command]
pub fn native_record_incident(app: AppHandle, source: String, message: String) {
    app.state::<Diagnostics>().record(&source, &message);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The audio URL is the dangerous one: its `token` plays the track for anyone who has it.
    #[test]
    fn redacts_signed_audio_urls() {
        let url = "https://t1-5.p-cdn.us/access/?version=5&lid=123456789&token=AbCdEf0123456789ZZ";
        let clean = redact(url);
        assert!(!clean.contains("AbCdEf0123456789ZZ"), "token leaked: {clean}");
        assert!(!clean.contains("123456789"), "listener id leaked: {clean}");
        // The shape must survive, or the report stops being useful.
        assert!(clean.contains("t1-5.p-cdn.us"));
        assert!(clean.contains("version=5"));
    }

    #[test]
    fn redacts_auth_tokens_and_passwords() {
        for secret in [
            "X-AuthToken: SECRETVALUE",
            "auth_token=SECRETVALUE&next=1",
            "userAuthToken:SECRETVALUE",
            "password=SECRETVALUE",
        ] {
            let clean = redact(secret);
            assert!(!clean.contains("SECRETVALUE"), "leaked from {secret:?}: {clean}");
        }
    }

    #[test]
    fn redacts_email_addresses() {
        let clean = redact("login failed for someone@example.com after 3 tries");
        assert!(!clean.contains("someone@example.com"));
        assert!(clean.contains("login failed for"));
        assert!(clean.contains("after 3 tries"));
    }

    /// Over-redaction would make reports useless, so ordinary words that merely contain a key
    /// name must survive untouched.
    #[test]
    fn leaves_ordinary_text_alone() {
        let text = "invalid token handling; the playlist was empty";
        assert_eq!(redact(text), text);
        assert_eq!(redact("STREAM_VIOLATION"), "STREAM_VIOLATION");
    }

    /// The limit that matters is the ENCODED url, not the raw body: percent-encoding roughly
    /// triples it, and an over-long url makes the report button silently do nothing.
    #[test]
    fn url_stays_openable_even_with_a_huge_history() {
        let incidents: Vec<Incident> = (0..400)
            .map(|i| Incident {
                at: "12:00:00Z".into(),
                source: "engine".into(),
                message: format!(
                    "failure number {i} with a good deal of explanatory text, newlines
and spaces"
                ),
            })
            .collect();
        let body = build_body("1.1.0", &UiContext::default(), "", &incidents);
        let url = issue_url("a fairly long and descriptive issue title", &body);
        assert!(url.len() <= MAX_URL, "url was {} chars", url.len());
        assert!(url.starts_with("https://github.com/"));
    }

    /// A short report must not be trimmed at all.
    #[test]
    fn short_reports_are_left_whole() {
        let body = build_body("1.1.0", &UiContext::default(), "position 3s", &[]);
        let url = issue_url("bug", &body);
        assert!(url.len() < MAX_URL);
        assert!(!url.contains("truncated"));
    }

    #[test]
    fn url_encodes_the_body() {
        let url = issue_url("a title", "line one\nline two & more");
        assert!(url.starts_with("https://github.com/cinderblock/jarlid/issues/new?"));
        assert!(!url.contains('\n'));
        assert!(url.contains("%0A")); // newline
        assert!(url.contains("%26")); // ampersand, which would otherwise start a new param
    }

    /// A report assembled from a realistic error must carry no secret through the whole path.
    #[test]
    fn end_to_end_report_has_no_secrets() {
        let diagnostics = Diagnostics::default();
        diagnostics.record(
            "engine",
            "could not play https://t1-5.p-cdn.us/access/?lid=99&token=LEAKME for you@example.com",
        );
        let body = build_body("1.1.0", &UiContext::default(), "", &diagnostics.recent());
        assert!(!body.contains("LEAKME"));
        assert!(!body.contains("you@example.com"));
    }
}

#[cfg(test)]
mod inspect {
    use super::*;

    /// Print a realistic report so a human can read what actually gets filed.
    ///
    /// Property assertions cannot catch a report that is technically correct and useless to read.
    /// Run with: cargo test show_a_real_report -- --nocapture --ignored
    #[test]
    #[ignore = "prints a sample report; not an assertion"]
    fn show_a_real_report() {
        let diagnostics = Diagnostics::default();
        diagnostics.record("engine", "pandora error 0: STREAM_VIOLATION");
        diagnostics.record(
            "engine",
            "could not play https://t1-5.p-cdn.us/access/?version=5&lid=987654&token=SIGNEDTOKENVALUE",
        );
        diagnostics.record("ui", "Cannot read properties of null (reading 'textContent') (index.js:412)");
        diagnostics.record("panic", "panicked at src/audio.rs:88: device disconnected");

        let context = UiContext {
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/140.0.0.0".into(),
            station: "QuickMix".into(),
            source_station: "Lindsey Stirling Radio".into(),
            mode: "Deep Cuts".into(),
            remote: false,
            note: "Skipped a track and the audio stopped. Account: me@example.com".into(),
        };

        let body = build_body("1.1.0", &context, "position 42s, buffered 5.0s, drift 0.00s", &diagnostics.recent());
        println!("\n===== ISSUE BODY =====\n{body}\n===== END =====");
        let url = issue_url("engine: pandora error 0: STREAM_VIOLATION", &body);
        println!("url length: {} (limit {MAX_URL})", url.len());
    }
}
