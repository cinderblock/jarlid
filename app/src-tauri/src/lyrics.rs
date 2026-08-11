//! Lyrics: fetching them from LRCLIB, keeping local edits, and sending corrections back.
//!
//! LRCLIB has no edit or delete. A correction is a *republish* of the same track — the
//! docs are explicit that "all previous revisions of the lyrics will still be kept when
//! publishing lyrics for a track that already has existing lyrics" — so the four
//! metadata fields (`trackName`, `artistName`, `albumName`, `duration`) are the identity
//! of the record. Publish with Pandora's spelling of those instead of the matched
//! record's and you don't correct anything; you create a sibling record next to the
//! wrong one. That is why [`Lyrics`] carries the record it came from.
//!
//! Publishing and flagging are both gated by a single-use proof-of-work token, which is
//! what [`solve_challenge`] is for.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

const USER_AGENT: &str =
    "PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)";

#[derive(Serialize, Deserialize, Default, Clone)]
// `default` so cache files written before the record fields existed still load.
#[serde(rename_all = "camelCase", default)]
pub struct Lyrics {
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub source: String,
    /// The LRCLIB record this came from. Needed verbatim to publish a correction
    /// *of this record* rather than a new one beside it.
    pub id: Option<i64>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    /// True when this is a local edit rather than what LRCLIB currently serves.
    pub overridden: bool,
}

impl Lyrics {
    fn is_empty(&self) -> bool {
        self.synced.is_none() && self.plain.is_none()
    }
}

/// FNV-1a of `artist|track|album`, lowercased — the identity of a track as *Pandora*
/// names it, which is all we have before a record is matched.
fn track_key(artist: &str, track: &str, album: Option<&str>) -> String {
    format!(
        "{}|{}|{}",
        artist.to_lowercase(),
        track.to_lowercase(),
        album.unwrap_or("").to_lowercase()
    )
}

fn hashed_file(dir: std::path::PathBuf, key: &str) -> Option<std::path::PathBuf> {
    std::fs::create_dir_all(&dir).ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Some(dir.join(format!("{h:016x}.json")))
}

/// Disk cache for LRCLIB responses (the service is slow). One JSON file per track.
fn lyrics_cache_path(app: &tauri::AppHandle, key: &str) -> Option<std::path::PathBuf> {
    hashed_file(app.path().app_cache_dir().ok()?.join("lyrics"), key)
}

/// Local edits. These live in the *data* dir, not the cache dir: the cache is
/// disposable by definition and these are the user's own work.
fn lyrics_override_path(app: &tauri::AppHandle, key: &str) -> Option<std::path::PathBuf> {
    hashed_file(
        app.path().app_data_dir().ok()?.join("lyrics-overrides"),
        key,
    )
}

/// Collapse a string that is exactly one part repeated 2-4 times (Pandora's
/// marquee clones its content while scrolling: "AbcAbcAbc" -> "Abc").
fn undouble(s: &str) -> String {
    let t = s.trim();
    let n = t.len();
    for k in (2..=4).rev() {
        if n >= k && n % k == 0 {
            let part = &t[..n / k];
            if !part.is_empty() && t.as_bytes().chunks(n / k).all(|c| c == part.as_bytes()) {
                return undouble(part);
            }
        }
    }
    t.to_string()
}

/// Strip trailing parentheticals / descriptors that hurt matching, e.g.
/// "Song (I Just Wanna Fall in Love)" -> "Song", "Song - Single" -> "Song".
fn simplify_title(s: &str) -> String {
    let mut t = s.to_string();
    if let Some(i) = t.find('(') {
        t.truncate(i);
    }
    if let Some(i) = t.find(" - ") {
        t.truncate(i);
    }
    t.trim().to_string()
}

/// Fetch lyrics: a local edit wins, then the disk cache, then LRCLIB — an exact
/// `get`, then progressively looser `search`es, choosing the best synced result by
/// closest duration.
#[tauri::command]
pub async fn fetch_lyrics(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<Lyrics, String> {
    let artist = undouble(&artist);
    let track = undouble(&track);
    let album = album.map(|a| undouble(&a));

    let key = track_key(&artist, &track, album.as_deref());

    if let Some(mut edit) = read_override(&app, &key) {
        edit.overridden = true;
        return Ok(edit);
    }

    let cache_file = lyrics_cache_path(&app, &key);
    if let Some(ref p) = cache_file {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(mut hit) = serde_json::from_slice::<Lyrics>(&bytes) {
                // Entries cached before the record fields existed can't be corrected —
                // there is nothing to identify the record. Treat them as a miss so the
                // cache heals one track at a time instead of needing a wipe.
                if hit.id.is_some() {
                    hit.source = format!("{} (cached)", hit.source);
                    return Ok(hit);
                }
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;

    // 1) exact match via /get (needs duration)
    if let Some(dur) = duration {
        let mut q: Vec<(&str, String)> = vec![
            ("artist_name", artist.clone()),
            ("track_name", track.clone()),
            ("duration", (dur.round() as i64).to_string()),
        ];
        if let Some(al) = album.clone() {
            if !al.is_empty() {
                q.push(("album_name", al));
            }
        }
        if let Ok(resp) = client
            .get("https://lrclib.net/api/get")
            .query(&q)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    return Ok(cache_and_return(from_lrclib(&v, "lrclib/get"), &cache_file));
                }
            }
        }
    }

    // 2) search — full title first, then simplified title
    let mut candidates: Vec<(&str, String)> = vec![("full", track.clone())];
    let simple = simplify_title(&track);
    if simple != track && !simple.is_empty() {
        candidates.push(("simple", simple));
    }
    for (label, t) in candidates {
        if let Ok(resp) = client
            .get("https://lrclib.net/api/search")
            .query(&[("track_name", t.as_str()), ("artist_name", artist.as_str())])
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(arr) = resp.json::<serde_json::Value>().await {
                    if let Some(best) = pick_best(&arr, duration) {
                        return Ok(cache_and_return(
                            from_lrclib(best, &format!("lrclib/search/{label}")),
                            &cache_file,
                        ));
                    }
                }
            }
        }
    }

    Ok(Lyrics {
        source: "none".into(),
        ..Default::default()
    })
}

/// From a search-results array, prefer entries with synced lyrics and the closest
/// duration to what's playing.
fn pick_best<'a>(
    arr: &'a serde_json::Value,
    duration: Option<f64>,
) -> Option<&'a serde_json::Value> {
    let items = arr.as_array()?;
    if items.is_empty() {
        return None;
    }
    let target = duration.unwrap_or(0.0);
    let score = |v: &serde_json::Value| -> (i32, f64) {
        let has_synced = v
            .get("syncedLyrics")
            .and_then(|x| x.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let dur = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let dur_gap = if target > 0.0 && dur > 0.0 {
            (dur - target).abs()
        } else {
            9999.0
        };
        // synced first (lower rank better), then smallest duration gap
        (if has_synced { 0 } else { 1 }, dur_gap)
    };
    items.iter().min_by(|a, b| {
        let (sa, ga) = score(a);
        let (sb, gb) = score(b);
        sa.cmp(&sb)
            .then(ga.partial_cmp(&gb).unwrap_or(std::cmp::Ordering::Equal))
    })
}

/// Persist found lyrics to the cache (misses are not cached so they retry).
fn cache_and_return(l: Lyrics, path: &Option<std::path::PathBuf>) -> Lyrics {
    if !l.is_empty() {
        if let Some(p) = path {
            if let Ok(json) = serde_json::to_vec(&l) {
                let _ = std::fs::write(p, json);
            }
        }
    }
    l
}

fn from_lrclib(v: &serde_json::Value, source: &str) -> Lyrics {
    let text = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
    };
    Lyrics {
        synced: text("syncedLyrics"),
        plain: text("plainLyrics"),
        source: source.to_string(),
        id: v.get("id").and_then(|x| x.as_i64()),
        track_name: text("trackName"),
        artist_name: text("artistName"),
        album_name: text("albumName"),
        duration: v.get("duration").and_then(|x| x.as_f64()),
        overridden: false,
    }
}

// ---- local edits --------------------------------------------------------------

fn read_override(app: &tauri::AppHandle, key: &str) -> Option<Lyrics> {
    let p = lyrics_override_path(app, key)?;
    let bytes = std::fs::read(p).ok()?;
    let l: Lyrics = serde_json::from_slice(&bytes).ok()?;
    if l.is_empty() {
        return None;
    }
    Some(l)
}

/// Save a local edit. Applies immediately and survives restarts, whether or not it is
/// ever published — publishing needs the network and a few seconds of proof-of-work,
/// and a typo should be fixable without either.
#[tauri::command]
pub async fn save_lyrics_override(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    lyrics: Lyrics,
) -> Result<Lyrics, String> {
    let mut lyrics = lyrics;
    lyrics.overridden = true;
    lyrics.source = "local edit".into();
    if lyrics.is_empty() {
        return Err("Nothing to save — the lyrics are empty".into());
    }
    let key = track_key(
        &undouble(&artist),
        &undouble(&track),
        album.as_deref().map(undouble).as_deref(),
    );
    let path = lyrics_override_path(&app, &key).ok_or("No data directory")?;
    let json = serde_json::to_vec_pretty(&lyrics).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(lyrics)
}

/// Drop a local edit, falling back to whatever LRCLIB serves.
#[tauri::command]
pub async fn clear_lyrics_override(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
) -> Result<(), String> {
    let key = track_key(
        &undouble(&artist),
        &undouble(&track),
        album.as_deref().map(undouble).as_deref(),
    );
    if let Some(p) = lyrics_override_path(&app, &key) {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    // The cached LRCLIB copy is about to be shown again; make sure it is current.
    if let Some(p) = lyrics_cache_path(&app, &key) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

// ---- publishing corrections ---------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Publication {
    /// The record's own metadata — the identity LRCLIB matches on. Must be the matched
    /// record's spelling, not Pandora's, or this creates a new record instead of a new
    /// revision of the one on screen.
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration: f64,
    pub plain: Option<String>,
    pub synced: Option<String>,
    /// How Pandora names this track, so the disk cache entry that this publication
    /// supersedes can be dropped.
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
}

#[derive(Deserialize)]
struct Challenge {
    prefix: String,
    target: String,
}

/// Ask LRCLIB for a challenge and solve it. The token is `{prefix}:{nonce}` and is
/// good for exactly one request.
async fn publish_token(client: &reqwest::Client) -> Result<String, String> {
    let challenge: Challenge = client
        .post("https://lrclib.net/api/request-challenge")
        .send()
        .await
        .map_err(|e| format!("Could not reach LRCLIB: {e}"))?
        .json()
        .await
        .map_err(|e| format!("LRCLIB sent a challenge we couldn't read: {e}"))?;

    let prefix = challenge.prefix.clone();
    let nonce = tauri::async_runtime::spawn_blocking(move || {
        solve_challenge(&challenge.prefix, &challenge.target)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("Could not solve the LRCLIB proof-of-work challenge")?;

    Ok(format!("{prefix}:{nonce}"))
}

/// Find a nonce where `SHA256(prefix + nonce)` sorts at or below `target`.
///
/// The published target (`000000FF00…`) means roughly 2^24 hashes, so this is spread
/// across the machine's cores — a couple of seconds rather than tens of them. Any valid
/// nonce is accepted, so the workers can stride independently and the first one home
/// wins.
fn solve_challenge(prefix: &str, target_hex: &str) -> Option<String> {
    let target = decode_hex(target_hex)?;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let found = std::sync::atomic::AtomicBool::new(false);
    let answer = std::sync::Mutex::new(None::<u64>);

    std::thread::scope(|s| {
        for t in 0..threads {
            let (target, found, answer) = (&target, &found, &answer);
            s.spawn(move || {
                let mut nonce = t as u64;
                loop {
                    // Checking a shared flag every hash would cost more than it saves.
                    if nonce % (threads as u64 * 4096) == t as u64
                        && found.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        return;
                    }
                    let mut hasher = Sha256::new();
                    hasher.update(prefix.as_bytes());
                    hasher.update(nonce.to_string().as_bytes());
                    if hasher.finalize().as_slice() <= target.as_slice() {
                        found.store(true, std::sync::atomic::Ordering::Relaxed);
                        *answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(nonce);
                        return;
                    }
                    nonce += threads as u64;
                }
            });
        }
    });

    answer
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .map(|n| n.to_string())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Publish a correction. LRCLIB keeps the old revision, so this is additive rather
/// than destructive — but it does change what everyone else sees, so the UI asks first.
#[tauri::command]
pub async fn publish_lyrics(app: tauri::AppHandle, publication: Publication) -> Result<(), String> {
    if publication.plain.is_none() && publication.synced.is_none() {
        // Empty means "this track is instrumental" to LRCLIB. Never say that by accident.
        return Err("Refusing to publish empty lyrics".into());
    }

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;
    let token = publish_token(&client).await?;

    let resp = client
        .post("https://lrclib.net/api/publish")
        .header("X-Publish-Token", token)
        .json(&serde_json::json!({
            "trackName": publication.track_name,
            "artistName": publication.artist_name,
            "albumName": publication.album_name,
            "duration": publication.duration,
            "plainLyrics": publication.plain.clone().unwrap_or_default(),
            "syncedLyrics": publication.synced.clone().unwrap_or_default(),
        }))
        .send()
        .await
        .map_err(|e| format!("Could not reach LRCLIB: {e}"))?;

    if !resp.status().is_success() {
        return Err(describe_failure(resp).await);
    }

    // What LRCLIB serves for this track has changed; don't keep handing out the old copy.
    let key = track_key(
        &undouble(&publication.artist),
        &undouble(&publication.track),
        publication.album.as_deref().map(undouble).as_deref(),
    );
    if let Some(p) = lyrics_cache_path(&app, &key) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Report a track whose published lyrics are wrong — the right move when a record is
/// matched to the wrong song entirely and there is nothing worth correcting by hand.
#[tauri::command]
pub async fn flag_lyrics(track_id: i64, content: Option<String>) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;
    let token = publish_token(&client).await?;

    let mut body = serde_json::json!({ "trackId": track_id });
    if let Some(reason) = content.filter(|c| !c.trim().is_empty()) {
        body["content"] = serde_json::Value::String(reason);
    }

    let resp = client
        .post("https://lrclib.net/api/flag")
        .header("X-Publish-Token", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach LRCLIB: {e}"))?;

    if !resp.status().is_success() {
        return Err(describe_failure(resp).await);
    }
    Ok(())
}

/// LRCLIB reports failures as `{code, name, message}`; surface its message when there
/// is one, since it says things like "the provided publish token is incorrect".
async fn describe_failure(resp: reqwest::Response) -> String {
    let status = resp.status();
    match resp.json::<serde_json::Value>().await {
        Ok(v) => match v.get("message").and_then(|m| m.as_str()) {
            Some(m) => format!("LRCLIB refused it: {m}"),
            None => format!("LRCLIB refused it (HTTP {status})"),
        },
        Err(_) => format!("LRCLIB refused it (HTTP {status})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The solver has to agree with LRCLIB's definition of a valid nonce, which is a
    /// byte-wise `<=` against the target — not a leading-zero count.
    #[test]
    fn solves_a_challenge() {
        // Deliberately easy target (one byte of work) so the test stays fast.
        let target = "00FF000000000000000000000000000000000000000000000000000000000000";
        let nonce = solve_challenge("abc", target).expect("solved");

        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        hasher.update(nonce.as_bytes());
        let hash = hasher.finalize();
        assert!(hash.as_slice() <= decode_hex(target).unwrap().as_slice());
    }

    /// Not run by default — this does the real ~2^24 hashes. Run it (`cargo test
    /// -- --ignored --nocapture`) to check how long a publish actually blocks for on a
    /// given machine; it should be seconds, not minutes.
    #[test]
    #[ignore]
    fn solves_the_live_difficulty() {
        let target = "000000FF00000000000000000000000000000000000000000000000000000000";
        let t0 = std::time::Instant::now();
        let nonce = solve_challenge("GyytT7uxw0WjY8o0GZLpA8lgPBt4QnhQ", target).expect("solved");
        println!("nonce {nonce} in {:?}", t0.elapsed());

        let mut hasher = Sha256::new();
        hasher.update(b"GyytT7uxw0WjY8o0GZLpA8lgPBt4QnhQ");
        hasher.update(nonce.as_bytes());
        assert!(hasher.finalize().as_slice() <= decode_hex(target).unwrap().as_slice());
    }

    #[test]
    fn decodes_hex_targets() {
        assert_eq!(decode_hex("000000FF").unwrap(), vec![0, 0, 0, 255]);
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }

    /// The cache and override files are keyed by how Pandora names the track, and that
    /// naming is case-noisy.
    #[test]
    fn track_key_is_case_insensitive() {
        assert_eq!(
            track_key("Klaas", "Better Off Alone", Some("Better Off Alone")),
            track_key("KLAAS", "better off alone", Some("BETTER OFF ALONE"))
        );
    }
}
