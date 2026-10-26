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
use std::time::Duration;
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
    /// True when this came from a cache entry old enough that LRCLIB may serve something
    /// newer. The caller shows it anyway and asks again with `refresh`. Only ever set on
    /// the way out, never stored or accepted back.
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// True when the listener chose this record over the automatic match. Output only,
    /// like `stale`: whether a choice exists is decided by the pin file, not by a flag
    /// that a cache file or the frontend could carry.
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// True when `synced` was generated on this machine (see `autosync`) from LRCLIB's plain
    /// lyrics, because LRCLIB had no synced ones. Output only, like `stale`: it is added on the
    /// way out of [`fetch_lyrics`] and never stored in the LRCLIB cache, so a cached record can't
    /// pass generated timings off as LRCLIB's.
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// Set when these lyrics are a draft the speech engine wrote for a song LRCLIB has nothing
    /// for, and a person hasn't yet reviewed every line. Unlike `auto` it is accepted back: a
    /// saved edit of a half-reviewed draft keeps it, so the review carries on where it stopped
    /// and the draft can't be published until it is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftReview>,
    /// Why there is no draft for a song LRCLIB has nothing for: `noSinging` or `unclear`.
    /// Output only, like `stale`.
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub no_draft: Option<String>,
    /// What `synced` can't hold: where each word starts and where lines end, line by line
    /// (pauses included as lines with no text). From an LRCLIB record's Lyricsfile, or a local
    /// edit timed in the editor's timeline; `None` when `synced` says it all. See `lyricsfile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Vec<crate::lyricsfile::TimedLine>>,
}

/// What a reviewer needs to know about a draft, line by line (by index into its lines, blank
/// ones included).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftReview {
    /// The words of each line the engine was unsure of, by position in the line.
    pub doubt: Vec<Vec<usize>>,
    /// Which lines a person has checked.
    pub reviewed: Vec<bool>,
}

impl Lyrics {
    fn is_empty(&self) -> bool {
        self.synced.is_none() && self.plain.is_none()
    }

    /// Show a fresh draft: its timed lines, with nothing reviewed yet.
    fn show_draft(&mut self, draft: crate::autosync::Draft) {
        self.synced = Some(draft.lrc);
        self.source = "draft".into();
        self.timing = Some(draft.timing).filter(|t| crate::lyricsfile::beyond_lrc(t));
        self.draft = Some(DraftReview {
            reviewed: vec![false; draft.doubt.len()],
            doubt: draft.doubt,
        });
    }
}

/// How long a cached LRCLIB record is trusted before it is checked again. Past this it is
/// still shown — it is almost always still right — while a refresh runs behind it.
const HIT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long "LRCLIB has nothing for this track" is believed. Short, because lyrics do get
/// added, but long enough that replaying a track doesn't cost another slow lookup.
const MISS_TTL: Duration = Duration::from_secs(2 * 60 * 60);

/// One disk cache file: what LRCLIB answered, and when. A miss is stored as empty lyrics.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    /// Unix seconds. Absent in files written before entries expired, which therefore
    /// read as stale and pick up a timestamp on their next refresh.
    #[serde(default)]
    cached_at: Option<u64>,
    #[serde(flatten)]
    lyrics: Lyrics,
}

impl CacheEntry {
    fn new(lyrics: Lyrics) -> Self {
        Self {
            cached_at: Some(unix_now()),
            lyrics,
        }
    }

    /// Whether this entry is still within its TTL at `now`. A timestamp from the future
    /// (the clock went backwards) isn't trusted.
    fn is_fresh(&self, now: u64) -> bool {
        let ttl = if self.lyrics.is_empty() {
            MISS_TTL
        } else {
            HIT_TTL
        };
        match self.cached_at {
            Some(at) if at <= now => now - at < ttl.as_secs(),
            _ => false,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

/// The cache key for a track as Pandora names it, after the same clean-up every lookup applies.
pub(crate) fn key_for(artist: &str, track: &str, album: Option<&str>) -> String {
    track_key(
        &undouble(artist),
        &undouble(track),
        album.map(undouble).as_deref(),
    )
}

pub(crate) fn hashed_file(dir: std::path::PathBuf, key: &str) -> Option<std::path::PathBuf> {
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

/// Fetch lyrics: a local edit wins, then the disk cache, then LRCLIB.
///
/// A fresh cache entry is the answer, hit or miss. A stale hit is still returned at
/// once, marked `stale`, so the pane isn't held on a slow lookup for lyrics that almost
/// never change. The caller then asks again with `refresh` set, which skips the cache and
/// goes to LRCLIB. A stale miss has nothing worth showing, so it goes to LRCLIB right away.
#[tauri::command]
pub async fn fetch_lyrics(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
    refresh: Option<bool>,
) -> Result<Lyrics, String> {
    let key = key_for(&artist, &track, album.as_deref());
    let mut found = lookup(&app, artist, track, album, duration, refresh).await?;
    // Generated timings fill in only where LRCLIB has words but no timings, and never over
    // the listener's own edit.
    if found.synced.is_none() && !found.overridden {
        if let Some(plain) = found.plain.as_deref() {
            if let Some(lrc) = crate::autosync::stored_lrc(&app, &key, plain) {
                found.synced = Some(lrc);
                found.auto = true;
            }
        }
    }
    // A draft fills in only where LRCLIB has nothing at all.
    if found.is_empty() && !found.overridden {
        match crate::autosync::stored_draft(&app, &key) {
            Some(Ok(draft)) => found.show_draft(draft),
            Some(Err(why)) => found.no_draft = Some(why),
            None => {}
        }
    }
    Ok(found)
}

/// [`fetch_lyrics`] without the generated timings: what the listener's edit, their chosen
/// record, the cache and LRCLIB say. Auto-sync asks this to decide whether a track needs it.
pub(crate) async fn lookup(
    app: &tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
    refresh: Option<bool>,
) -> Result<Lyrics, String> {
    let app = app.clone();
    let artist = undouble(&artist);
    let track = undouble(&track);
    let album = album.map(|a| undouble(&a));

    let key = track_key(&artist, &track, album.as_deref());

    if let Some(mut edit) = read_override(&app, &key) {
        edit.overridden = true;
        return Ok(edit);
    }

    let mut pin = read_pin(&app, &key);

    let cache_file = lyrics_cache_path(&app, &key);
    let cached = cache_file
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<CacheEntry>(&bytes).ok())
        // Hits cached before the record fields existed can't be corrected — there is
        // nothing to identify the record. Treat them as absent so the cache heals one
        // track at a time instead of needing a wipe.
        .filter(|e| e.lyrics.is_empty() || e.lyrics.id.is_some())
        // With a record chosen, a cached copy of any other record isn't this track's
        // answer any more.
        .filter(|e| pin.map_or(true, |id| e.lyrics.id == Some(id)));

    if let Some(entry) = &cached {
        let fresh = entry.is_fresh(unix_now());
        if !refresh.unwrap_or(false) && (fresh || !entry.lyrics.is_empty()) {
            let mut hit = entry.lyrics.clone();
            hit.source = format!("{} (cached)", hit.source);
            hit.stale = !fresh;
            hit.pinned = pin.is_some();
            return Ok(hit);
        }
    }

    let found = match pin {
        Some(id) => match lrclib_record(id).await? {
            Some(l) => Some(l),
            // LRCLIB doesn't delete records, so this shouldn't happen. If it does, the
            // choice points at nothing; forget it and match as if it had never been made.
            None => {
                remove_pin(&app, &key);
                pin = None;
                lookup_lrclib(&artist, &track, album.as_deref(), duration).await?
            }
        },
        None => lookup_lrclib(&artist, &track, album.as_deref(), duration).await?,
    };

    let entry = match (found, cached) {
        (Some(l), _) => CacheEntry::new(l),
        // LRCLIB never deletes a record, so finding nothing where something was found
        // before says more about search than about the track. Keep the copy we have and
        // stop re-checking it for another TTL.
        (None, Some(prior)) if !prior.lyrics.is_empty() => {
            let renewed = CacheEntry::new(prior.lyrics);
            write_cache(&cache_file, &renewed);
            let mut hit = renewed.lyrics;
            hit.source = format!("{} (cached)", hit.source);
            return Ok(hit);
        }
        (None, _) => CacheEntry::new(Lyrics {
            source: "none".into(),
            ..Default::default()
        }),
    };
    write_cache(&cache_file, &entry);
    let mut out = entry.lyrics;
    out.pinned = pin.is_some();
    Ok(out)
}

/// One LRCLIB record by id: what a chosen record is refreshed from.
async fn lrclib_record(id: i64) -> Result<Option<Lyrics>, String> {
    let client = lrclib_client()?;
    let url = format!("https://lrclib.net/api/get/{id}");
    let none: [(&str, &str); 0] = [];
    Ok(lrclib_get(&client, &url, &none)
        .await?
        .map(|v| from_lrclib(&v, "lrclib/chosen")))
}

/// Ask LRCLIB: an exact `get`, then progressively looser `search`es, choosing the best
/// result by [`rank`]. `Ok(None)` is a definite miss; `Err` is no answer.
async fn lookup_lrclib(
    artist: &str,
    track: &str,
    album: Option<&str>,
    duration: Option<f64>,
) -> Result<Option<Lyrics>, String> {
    let client = lrclib_client()?;

    // 1) exact match via /get (needs duration)
    if let Some(dur) = duration {
        let mut q: Vec<(&str, String)> = vec![
            ("artist_name", artist.to_string()),
            ("track_name", track.to_string()),
            ("duration", (dur.round() as i64).to_string()),
        ];
        if let Some(al) = album.filter(|a| !a.is_empty()) {
            q.push(("album_name", al.to_string()));
        }
        if let Some(v) = lrclib_get(&client, "https://lrclib.net/api/get", &q).await? {
            return Ok(Some(from_lrclib(&v, "lrclib/get")));
        }
    }

    // 2) search — full title first, then simplified title
    let want = Want {
        track,
        album,
        duration,
    };
    for (label, t) in search_titles(track) {
        let q = [("track_name", t.as_str()), ("artist_name", artist)];
        if let Some(arr) = lrclib_get(&client, "https://lrclib.net/api/search", &q).await? {
            if let Some(best) = pick_best(&arr, &want) {
                return Ok(Some(from_lrclib(best, &format!("lrclib/search/{label}"))));
            }
        }
    }

    Ok(None)
}

/// The titles to search under, tightest first: as Pandora names it, then simplified.
fn search_titles(track: &str) -> Vec<(&'static str, String)> {
    let mut titles = vec![("full", track.to_string())];
    let simple = simplify_title(track);
    if simple != track && !simple.is_empty() {
        titles.push(("simple", simple));
    }
    titles
}

/// How long one attempt may take. LRCLIB is slow, not hung: a reply that has not
/// started in this long is not coming, and holding the pane on "Loading lyrics…" for
/// the rest of the song would be worse than trying again.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Pauses between attempts at one request. Two retries, so a request that fails for a
/// passing reason gets three tries in a little over half a minute at worst.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

fn lrclib_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// One LRCLIB lookup, retried while the failure is the kind that a retry can fix.
///
/// The three outcomes a caller has to tell apart:
///
/// - `Ok(Some(body))` — LRCLIB answered. For `/get` that is the record; for `/search`
///   it is an array, possibly empty.
/// - `Ok(None)` — LRCLIB answered "no such record" (a 404). Only `/get` does this;
///   `/search` says the same thing with an empty array.
/// - `Err` — no answer. A timeout, a connection error, a 5xx or a 429 is tried again
///   after each of [`RETRY_DELAYS`] before it becomes this; any other 4xx becomes this
///   at once, since asking the same question again would get the same refusal.
///
/// The distinction matters because a miss and an outage look identical from the pane
/// otherwise, and "No lyrics found" is a claim about the track, not about the network.
async fn lrclib_get<Q: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    url: &str,
    query: &Q,
) -> Result<Option<serde_json::Value>, String> {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        let failure = match client.get(url).query(query).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json().await {
                Ok(v) => return Ok(Some(v)),
                // A body that stopped short or came back as something other than JSON
                // is as transient as a dropped connection.
                Err(e) => format!("unreadable reply: {e}"),
            },
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => return Ok(None),
            Ok(resp)
                if resp.status().is_server_error()
                    || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS =>
            {
                format!("HTTP {}", resp.status())
            }
            Ok(resp) => {
                return Err(format!(
                    "LRCLIB refused the lookup (HTTP {})",
                    resp.status()
                ))
            }
            Err(e) => e.to_string(),
        };
        match delays.next() {
            Some(d) => tokio::time::sleep(*d).await,
            None => return Err(format!("Could not reach LRCLIB: {failure}")),
        }
    }
}

/// What's playing, as far as matching a search result goes.
struct Want<'a> {
    track: &'a str,
    album: Option<&'a str>,
    duration: Option<f64>,
}

/// Durations further apart than this are a different cut of the song (radio edit,
/// extended mix), and synced timings from one will drift against the other.
const SAME_CUT_SECS: f64 = 3.0;

/// How closely two names agree: 0 identical (ignoring case and spacing), 1 identical once
/// parentheticals and " - " suffixes are stripped, 2 otherwise.
fn name_distance(a: &str, b: &str) -> u8 {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    if norm(a) == norm(b) {
        0
    } else if norm(&simplify_title(a)) == norm(&simplify_title(b)) {
        1
    } else {
        2
    }
}

/// Sort key for one search result; lower is better.
///
/// Title comes first. A record whose title names a different version ("French Version",
/// "Split Brain Version") has the wrong words, which is worse than timings that drift
/// or no timings at all. Then same cut, then synced, then album, then the exact gap.
/// Without the title and album terms a song with many versions of one length (all
/// equally synced, all 165s) was decided by whatever order LRCLIB returned them in.
fn rank(v: &serde_json::Value, want: &Want) -> (u8, u8, u8, u8, i64) {
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let has_synced = !field("syncedLyrics").is_empty();
    let gap = match (want.duration, v.get("duration").and_then(|x| x.as_f64())) {
        (Some(w), Some(d)) if w > 0.0 && d > 0.0 => Some((d - w).abs()),
        _ => None,
    };
    let album = match want.album.filter(|a| !a.is_empty()) {
        Some(a) => name_distance(a, field("albumName")),
        None => 0,
    };
    (
        name_distance(want.track, field("trackName")),
        gap.map_or(1, |g| (g > SAME_CUT_SECS) as u8),
        !has_synced as u8,
        album,
        gap.map_or(i64::MAX, |g| (g * 1000.0) as i64),
    )
}

/// The best of a search-results array for what's playing. See [`rank`].
fn pick_best<'a>(arr: &'a serde_json::Value, want: &Want) -> Option<&'a serde_json::Value> {
    arr.as_array()?.iter().min_by_key(|v| rank(v, want))
}

/// Persist an LRCLIB answer, hit or miss. Best effort: a cache that can't be written
/// only costs a lookup next time.
fn write_cache(path: &Option<std::path::PathBuf>, entry: &CacheEntry) {
    if let Some(p) = path {
        if let Ok(json) = serde_json::to_vec(entry) {
            let _ = std::fs::write(p, json);
        }
    }
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
        stale: false,
        pinned: false,
        auto: false,
        draft: None,
        no_draft: None,
        timing: v
            .get("lyricsfile")
            .and_then(|x| x.as_str())
            .and_then(crate::lyricsfile::parse),
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

// ---- choosing among matches -----------------------------------------------------
//
// Search finds several records for most tracks: versions, languages, duplicate uploads.
// `rank` picks one, and when it picks wrong the listener can see the rest and choose.
// The choice is a pin on a record id, filed (like an edit) under Pandora's naming of the
// track, in the data dir: it is the user's decision, not something a cache wipe should undo.

#[derive(Serialize, Deserialize)]
struct Pin {
    id: i64,
}

fn lyrics_pin_path(app: &tauri::AppHandle, key: &str) -> Option<std::path::PathBuf> {
    hashed_file(app.path().app_data_dir().ok()?.join("lyrics-picks"), key)
}

fn read_pin(app: &tauri::AppHandle, key: &str) -> Option<i64> {
    let bytes = std::fs::read(lyrics_pin_path(app, key)?).ok()?;
    serde_json::from_slice::<Pin>(&bytes).ok().map(|p| p.id)
}

fn remove_pin(app: &tauri::AppHandle, key: &str) {
    if let Some(p) = lyrics_pin_path(app, key) {
        let _ = std::fs::remove_file(p);
    }
}

/// Every record LRCLIB's searches turn up for this track, best first by the same
/// [`rank`] the automatic choice uses, so the top entry is what would be picked.
///
/// Wider than the automatic lookup on purpose: it adds a free-text search, which also
/// finds records filed under a variant spelling of the artist or title. The searches run
/// at once (LRCLIB is slow), and one failing still leaves the others' results.
#[tauri::command]
pub async fn lyrics_candidates(
    artist: String,
    track: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<Vec<Lyrics>, String> {
    let artist = undouble(&artist);
    let track = undouble(&track);
    let album = album.map(|a| undouble(&a));

    let client = lrclib_client()?;
    let mut queries: Vec<Vec<(&str, String)>> = search_titles(&track)
        .into_iter()
        .map(|(_, t)| vec![("track_name", t), ("artist_name", artist.clone())])
        .collect();
    queries.push(vec![("q", format!("{artist} {}", simplify_title(&track)))]);
    let searches: Vec<_> = queries
        .into_iter()
        .map(|q| {
            let client = client.clone();
            tauri::async_runtime::spawn(async move {
                lrclib_get(&client, "https://lrclib.net/api/search", &q).await
            })
        })
        .collect();

    let mut found: Vec<serde_json::Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut failure = None;
    for search in searches {
        match search.await.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(Some(serde_json::Value::Array(items))) => {
                // The searches overlap heavily; keep the first copy of each record.
                found.extend(items.into_iter().filter(|v| {
                    v.get("id")
                        .and_then(|x| x.as_i64())
                        .is_some_and(|id| seen.insert(id))
                }));
            }
            Ok(_) => {}
            Err(e) => failure = Some(e),
        }
    }
    if found.is_empty() {
        if let Some(e) = failure {
            return Err(e);
        }
    }

    let want = Want {
        track: &track,
        album: album.as_deref(),
        duration,
    };
    // The free-text search isn't confined to the artist, so a different artist's song of
    // the same name sorts after every record that is plausibly this one.
    let other_artist = |v: &serde_json::Value| {
        let theirs = v.get("artistName").and_then(|x| x.as_str()).unwrap_or("");
        let (a, b) = (artist.to_lowercase(), theirs.to_lowercase());
        !(a.contains(&b) || b.contains(&a)) || b.is_empty()
    };
    found.sort_by_key(|v| (other_artist(v), rank(v, &want)));
    Ok(found
        .iter()
        // An instrumental record has no words to show; there is nothing to choose.
        .map(|v| from_lrclib(v, "lrclib/chosen"))
        .filter(|l| !l.is_empty())
        .collect())
}

/// Choose one of [`lyrics_candidates`] for this track from now on. The record arrives
/// whole (search results carry the lyrics), so this needs no request: it pins the id and
/// puts the record in the cache as the current answer.
#[tauri::command]
pub async fn choose_lyrics(
    app: tauri::AppHandle,
    artist: String,
    track: String,
    album: Option<String>,
    lyrics: Lyrics,
) -> Result<Lyrics, String> {
    let id = lyrics
        .id
        .ok_or("That record has no LRCLIB id to remember")?;
    if lyrics.is_empty() {
        return Err("That record has no lyrics".into());
    }
    let key = track_key(
        &undouble(&artist),
        &undouble(&track),
        album.as_deref().map(undouble).as_deref(),
    );
    let path = lyrics_pin_path(&app, &key).ok_or("No data directory")?;
    std::fs::write(
        &path,
        serde_json::to_vec(&Pin { id }).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let mut chosen = Lyrics {
        source: "lrclib/chosen".into(),
        overridden: false,
        ..lyrics
    };
    write_cache(
        &lyrics_cache_path(&app, &key),
        &CacheEntry::new(chosen.clone()),
    );
    chosen.pinned = true;
    Ok(chosen)
}

/// Forget a choice and go back to the automatic match. The cached copy is the chosen
/// record, so it goes too; the next fetch matches afresh.
#[tauri::command]
pub async fn clear_lyrics_choice(
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
    remove_pin(&app, &key);
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
    /// Word timings and line ends, when there are any: published as a Lyricsfile, from which
    /// LRCLIB derives the plain and synced lyrics older players read.
    #[serde(default)]
    pub timing: Option<Vec<crate::lyricsfile::TimedLine>>,
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
    // The editor saves before it publishes, and a saved draft stops being one only once every
    // line has been reviewed. The editor won't offer Publish before then; this makes sure.
    let key = track_key(
        &undouble(&publication.artist),
        &undouble(&publication.track),
        publication.album.as_deref().map(undouble).as_deref(),
    );
    if read_override(&app, &key).is_some_and(|l| l.draft.is_some()) {
        return Err("Every line of a draft has to be reviewed before it can be published".into());
    }

    // Built, and checked to read back as LRCLIB will read it, before any proof-of-work: a
    // Lyricsfile LRCLIB can't parse would leave the record with no lyrics for older players.
    let lyricsfile = match publication.timing.as_deref() {
        Some(lines) if crate::lyricsfile::beyond_lrc(lines) => Some(crate::lyricsfile::build(
            &crate::lyricsfile::Track {
                title: &publication.track_name,
                artist: &publication.artist_name,
                album: &publication.album_name,
                duration: publication.duration,
            },
            lines,
            publication.plain.as_deref(),
        )?),
        _ => None,
    };

    let client = lrclib_client()?;
    let token = publish_token(&client).await?;

    let mut body = serde_json::json!({
        "trackName": publication.track_name,
        "artistName": publication.artist_name,
        "albumName": publication.album_name,
        "duration": publication.duration,
        "plainLyrics": publication.plain.clone().unwrap_or_default(),
        "syncedLyrics": publication.synced.clone().unwrap_or_default(),
    });
    // With a Lyricsfile, LRCLIB derives the two above from it and ignores what was sent.
    if let Some(yaml) = lyricsfile {
        body["lyricsfile"] = serde_json::Value::String(yaml);
    }
    let resp = client
        .post("https://lrclib.net/api/publish")
        .header("X-Publish-Token", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach LRCLIB: {e}"))?;

    if !resp.status().is_success() {
        return Err(describe_failure(resp).await);
    }

    // What LRCLIB serves for this track has changed; don't keep handing out the old copy.
    if let Some(p) = lyrics_cache_path(&app, &key) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Report a track whose published lyrics are wrong — the right move when a record is
/// matched to the wrong song entirely and there is nothing worth correcting by hand.
#[tauri::command]
pub async fn flag_lyrics(
    app: tauri::AppHandle,
    track_id: i64,
    content: Option<String>,
    artist: String,
    track: String,
    album: Option<String>,
) -> Result<(), String> {
    let client = lrclib_client()?;
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

    // The flagged record may be pulled or replaced; look again next time rather than
    // keep showing it for the rest of its TTL.
    let key = track_key(
        &undouble(&artist),
        &undouble(&track),
        album.as_deref().map(undouble).as_deref(),
    );
    if let Some(p) = lyrics_cache_path(&app, &key) {
        let _ = std::fs::remove_file(p);
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

    fn entry(found: bool, cached_at: Option<u64>) -> CacheEntry {
        CacheEntry {
            cached_at,
            lyrics: Lyrics {
                plain: found.then(|| "words".to_string()),
                id: found.then_some(1),
                ..Default::default()
            },
        }
    }

    #[test]
    fn misses_expire_sooner_than_hits() {
        let now = 1_000_000;
        let three_hours_ago = Some(now - 3 * 60 * 60);
        assert!(entry(true, three_hours_ago).is_fresh(now));
        assert!(!entry(false, three_hours_ago).is_fresh(now));

        let an_hour_ago = Some(now - 60 * 60);
        assert!(entry(false, an_hour_ago).is_fresh(now));

        let two_days_ago = Some(now - 2 * 24 * 60 * 60);
        assert!(!entry(true, two_days_ago).is_fresh(now));
    }

    /// Files from before entries had timestamps, and timestamps from a clock that has
    /// since gone backwards, must not count as fresh, or they would never be rechecked.
    #[test]
    fn untimed_and_future_entries_are_stale() {
        let now = 1_000_000;
        assert!(!entry(true, None).is_fresh(now));
        assert!(!entry(true, Some(now + 60)).is_fresh(now));
    }

    /// A cache file written before `cachedAt` existed is a bare `Lyrics` object, and has
    /// to keep loading.
    #[test]
    fn reads_untimed_cache_files() {
        let old = br#"{"synced":null,"plain":"words","source":"lrclib/get","id":7}"#;
        let e: CacheEntry = serde_json::from_slice(old).expect("loads");
        assert_eq!(e.cached_at, None);
        assert_eq!(e.lyrics.id, Some(7));

        let round = serde_json::to_vec(&CacheEntry::new(e.lyrics)).unwrap();
        let back: CacheEntry = serde_json::from_slice(&round).unwrap();
        assert!(back.cached_at.is_some());
        assert_eq!(back.lyrics.plain.as_deref(), Some("words"));
    }

    /// A saved edit of a half-reviewed draft must stay a draft, or it could be published
    /// unreviewed; why there is no draft describes one answer, like `stale`.
    #[test]
    fn a_draft_survives_a_save_and_no_draft_does_not() {
        let l = Lyrics {
            synced: Some("[00:01.00] Heard words".into()),
            draft: Some(DraftReview {
                doubt: vec![vec![1]],
                reviewed: vec![false],
            }),
            no_draft: Some("unclear".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["draft"]["reviewed"], serde_json::json!([false]));
        assert_eq!(json["noDraft"], "unclear");
        let back: Lyrics = serde_json::from_value(json).unwrap();
        assert_eq!(back.draft, l.draft);
        assert_eq!(back.no_draft, None);
        // Lyrics that aren't a draft don't say so.
        assert!(serde_json::to_value(Lyrics::default())
            .unwrap()
            .get("draft")
            .is_none());
    }

    /// `stale` describes one answer, not the lyrics. It must not be written to disk or
    /// accepted back from the frontend (which hands `Lyrics` to `save_lyrics_override`).
    #[test]
    fn stale_is_output_only() {
        let l = Lyrics {
            stale: true,
            ..Default::default()
        };
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["stale"], true);
        let back: Lyrics = serde_json::from_value(json).unwrap();
        assert!(!back.stale);

        // `pinned` likewise: `choose_lyrics` is handed a `Lyrics` by the frontend.
        let back: Lyrics = serde_json::from_value(serde_json::json!({"pinned": true})).unwrap();
        assert!(!back.pinned);
        assert!(serde_json::to_value(Lyrics::default())
            .unwrap()
            .get("stale")
            .is_none());
    }

    fn result(id: i64, track: &str, album: &str, duration: f64) -> serde_json::Value {
        serde_json::json!({
            "id": id, "trackName": track, "albumName": album, "duration": duration,
            "syncedLyrics": "[00:00.22] words", "plainLyrics": "words",
        })
    }

    /// The shape of LRCLIB's answer for Madelline — "Dopamine": many versions, all synced,
    /// nearly all 165s, in an order that put a different version first. Duration alone
    /// can't separate them; title and album have to.
    #[test]
    fn picks_by_title_and_album_when_durations_tie() {
        let arr = serde_json::json!([
            result(
                33935652,
                "Dopamine",
                "Dopamine (Split Brain Version)",
                165.0
            ),
            result(
                36641358,
                "dopamine (Version Française)",
                "dopamine - Single",
                165.0
            ),
            result(2162555, "dopamine", "dopamine", 165.0),
            result(2162554, "Dopamine", "Dopamine - Single", 165.0),
            result(26107595, "dopamine", "dopamine", 181.5),
        ]);
        let id = |want: &Want| pick_best(&arr, want).unwrap()["id"].as_i64().unwrap();

        let want = |album| Want {
            track: "Dopamine",
            album,
            duration: Some(165.4),
        };
        assert_eq!(id(&want(Some("Dopamine - Single"))), 2162554);
        assert_eq!(id(&want(Some("dopamine"))), 2162555);

        // A version Pandora names is the version it gets.
        let split = Want {
            track: "Dopamine (Split Brain Version)",
            album: None,
            duration: Some(165.0),
        };
        let arr2 = serde_json::json!([
            result(2162555, "dopamine", "dopamine", 165.0),
            result(35090140, "Dopamine (Split Brain Version)", "x", 165.0),
        ]);
        assert_eq!(pick_best(&arr2, &split).unwrap()["id"], 35090140);
    }

    /// Among equally-named records, a different cut loses to synced-but-right-length,
    /// and synced beats plain.
    #[test]
    fn prefers_same_cut_then_synced() {
        let mut plain = result(3, "Song", "A", 200.0);
        plain["syncedLyrics"] = serde_json::Value::Null;
        let arr = serde_json::json!([
            result(1, "Song", "A", 240.0),
            plain,
            result(2, "Song", "B", 201.0)
        ]);
        let want = Want {
            track: "Song",
            album: Some("A"),
            duration: Some(200.0),
        };
        assert_eq!(pick_best(&arr, &want).unwrap()["id"], 2);
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
