//! Songs kept on disk, so a repeat is played from there instead of downloaded again.
//!
//! **Off unless the user turns it on.** Pandora's terms generally don't allow keeping their audio
//! outside their own offline feature, so this is opt-in and the user chooses the size.
//!
//! Keyed by `pandora_id`, which is stable. The signed `audioURL` changes on every play and is
//! useless as a key. One file per song, named from the id, with the extension the audio actually
//! is: Media Foundation picks a byte-stream handler by extension, so a wrong or missing one would
//! fail to open.
//!
//! The *next* track is downloaded whole while the current one plays, and then played from here.
//! That costs no extra bandwidth on a first play, because one complete download replaces the
//! stream it would otherwise have been. See `plans/audio-track-cache.md`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where the cache lives and how big it may get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    pub dir: PathBuf,
    pub max_bytes: u64,
}

/// How much is cached right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub bytes: u64,
    pub songs: u64,
}

/// Extensions a cached song can have. Anything else in the directory is not ours.
const EXTENSIONS: [&str; 3] = ["mp3", "aac", "m4a"];

/// Suffix of a download in progress. Never played, never counted, and removed if found later:
/// it is what a crash or a lost connection mid-download leaves behind.
const PARTIAL: &str = "part";

/// The file name stem for a `pandora_id`, or `None` if it cannot be one. Ids look like
/// `TR:12345`; anything outside a conservative alphabet is mapped to `_` so no id can name a
/// path outside the directory.
fn stem(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    Some(
        id.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
    )
}

/// The cached file for `id`, if there is one.
pub fn find(dir: &Path, id: &str) -> Option<PathBuf> {
    let stem = stem(id)?;
    EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// The cached file for `id`, marked as just played so eviction keeps it longest.
///
/// The mark is the file's modified time. Best effort: a cache that can't be touched still plays.
pub fn take(dir: &Path, id: &str) -> Option<PathBuf> {
    let path = find(dir, id)?;
    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(path)
}

/// What a file holds, from its first bytes, as the extension Media Foundation needs.
///
/// Sniffed rather than taken from `Content-Type`, which a CDN is free to get wrong and which says
/// nothing about the file once it is on disk. `None` means "not something we know how to
/// name", and such a download is not cached.
fn sniff(head: &[u8]) -> Option<&'static str> {
    match head {
        [b'I', b'D', b'3', ..] => Some("mp3"),
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => Some("m4a"),
        // Both are 0xFFF-ish sync words, told apart by the layer bits: ADTS (AAC) always has
        // layer 00, MPEG audio never does.
        [0xFF, b1, ..] if b1 & 0xF6 == 0xF0 => Some("aac"),
        [0xFF, b1, ..] if b1 & 0xE0 == 0xE0 && b1 & 0x06 != 0 => Some("mp3"),
        _ => None,
    }
}

/// Download `url` in full and store it as `id`. Returns the stored path.
///
/// Written to a `.part` file and renamed into place only once complete, so a half-downloaded
/// song can never be found and played.
pub async fn store(
    client: &reqwest::Client,
    dir: &Path,
    id: &str,
    url: &str,
) -> Result<PathBuf, String> {
    let stem = stem(id).ok_or("no id to cache under")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cache dir: {e}"))?;

    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download: {}", e.without_url()))?;
    let expected = response.content_length();

    let partial = dir.join(format!("{stem}.{PARTIAL}"));
    let result = async {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&partial).map_err(|e| format!("create {partial:?}: {e}"))?;
        let mut head = Vec::with_capacity(16);
        let mut written = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("download: {}", e.without_url()))?
        {
            if head.len() < 16 {
                let take = (16 - head.len()).min(chunk.len());
                head.extend_from_slice(&chunk[..take]);
            }
            file.write_all(&chunk).map_err(|e| format!("write: {e}"))?;
            written += chunk.len() as u64;
        }
        file.flush().map_err(|e| format!("write: {e}"))?;
        drop(file);

        // A connection that closes early ends the stream without an error. Without a length to
        // check against, a truncated song would be cached and replayed truncated forever.
        if let Some(expected) = expected {
            if written != expected {
                return Err(format!("download stopped at {written} of {expected} bytes"));
            }
        }
        let ext = sniff(&head).ok_or("not an audio format we can name")?;
        let path = dir.join(format!("{stem}.{ext}"));
        std::fs::rename(&partial, &path).map_err(|e| format!("rename: {e}"))?;
        Ok(path)
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Every cached song, with its size and when it was last played.
fn entries(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension()?.to_str()?;
            if !EXTENSIONS.contains(&ext) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    path,
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                )
            })
        })
        .collect()
}

pub fn stats(dir: &Path) -> CacheStats {
    let all = entries(dir);
    CacheStats {
        bytes: all.iter().map(|(_, size, _)| size).sum(),
        songs: all.len() as u64,
    }
}

/// Delete the least recently played songs until the cache fits in `max_bytes`, never touching
/// the ones in `keep` (playing now, or about to). Also sweeps up abandoned partial downloads
/// that are not in progress.
pub fn evict(dir: &Path, max_bytes: u64, keep: &[&str]) {
    let keep: Vec<String> = keep.iter().filter_map(|id| stem(id)).collect();
    let kept = |path: &Path| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| keep.iter().any(|k| k == s))
    };

    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == PARTIAL) && !kept(&path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    let mut all = entries(dir);
    let mut total: u64 = all.iter().map(|(_, size, _)| size).sum();
    all.sort_by_key(|(_, _, played)| *played);
    for (path, size, _) in all {
        if total <= max_bytes {
            break;
        }
        // A file that is playing may be open; Windows refuses to delete it, and that is fine.
        if kept(&path) || std::fs::remove_file(&path).is_err() {
            continue;
        }
        total -= size;
    }
}

/// Delete every cached song. Returns what is left: anything open right now (the song playing)
/// stays until it is closed.
pub fn clear(dir: &Path) -> CacheStats {
    evict(dir, 0, &[]);
    stats(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jarlid-cache-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, id: &str, ext: &str, size: usize, age_secs: u64) {
        let path = dir.join(format!("{}.{ext}", stem(id).unwrap()));
        std::fs::write(&path, vec![0u8; size]).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(age_secs))
            .unwrap();
    }

    #[test]
    fn ids_cannot_escape_the_directory() {
        assert_eq!(stem("TR:12345").as_deref(), Some("TR_12345"));
        assert_eq!(stem("..\\..\\evil").as_deref(), Some("______evil"));
        assert_eq!(stem(""), None);
    }

    #[test]
    fn formats_are_told_apart_by_their_first_bytes() {
        assert_eq!(sniff(b"ID3\x04\x00"), Some("mp3"));
        // MPEG-1 Layer III, 128 kbit/s.
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x64]), Some("mp3"));
        // ADTS AAC: same sync, layer bits 00.
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Some("aac"));
        assert_eq!(sniff(&[0xFF, 0xF9, 0x50, 0x80]), Some("aac"));
        assert_eq!(sniff(b"\x00\x00\x00\x20ftypM4A "), Some("m4a"));
        assert_eq!(
            sniff(b"<html>"),
            None,
            "an error page must not be cached as a song"
        );
        assert_eq!(sniff(&[]), None);
    }

    #[test]
    fn finds_a_song_whatever_its_extension() {
        let dir = scratch("find");
        put(&dir, "TR:1", "aac", 10, 0);
        assert_eq!(find(&dir, "TR:1"), Some(dir.join("TR_1.aac")));
        assert_eq!(find(&dir, "TR:2"), None);
    }

    #[test]
    fn evicts_the_least_recently_played_first() {
        let dir = scratch("evict");
        put(&dir, "TR:old", "mp3", 100, 300);
        put(&dir, "TR:mid", "mp3", 100, 200);
        put(&dir, "TR:new", "mp3", 100, 100);

        evict(&dir, 200, &[]);

        assert_eq!(find(&dir, "TR:old"), None);
        assert!(find(&dir, "TR:mid").is_some());
        assert!(find(&dir, "TR:new").is_some());
        assert_eq!(
            stats(&dir),
            CacheStats {
                bytes: 200,
                songs: 2
            }
        );
    }

    #[test]
    fn playing_a_song_moves_it_to_the_back_of_the_queue() {
        let dir = scratch("touch");
        put(&dir, "TR:a", "mp3", 100, 300);
        put(&dir, "TR:b", "mp3", 100, 200);

        assert!(take(&dir, "TR:a").is_some());
        evict(&dir, 100, &[]);

        assert!(
            find(&dir, "TR:a").is_some(),
            "the song just played was evicted"
        );
        assert_eq!(find(&dir, "TR:b"), None);
    }

    #[test]
    fn never_evicts_what_is_playing_or_next() {
        let dir = scratch("keep");
        put(&dir, "TR:now", "mp3", 100, 300);
        put(&dir, "TR:next", "mp3", 100, 200);
        put(&dir, "TR:other", "mp3", 100, 100);

        evict(&dir, 0, &["TR:now", "TR:next"]);

        assert!(find(&dir, "TR:now").is_some());
        assert!(find(&dir, "TR:next").is_some());
        assert_eq!(find(&dir, "TR:other"), None);
    }

    #[test]
    fn partial_downloads_are_never_songs_and_get_swept() {
        let dir = scratch("partial");
        put(&dir, "TR:half", PARTIAL, 100, 0);
        assert_eq!(find(&dir, "TR:half"), None);
        assert_eq!(stats(&dir), CacheStats::default());

        // One in progress is left alone.
        evict(&dir, 1000, &["TR:half"]);
        assert!(dir.join("TR_half.part").exists());
        evict(&dir, 1000, &[]);
        assert!(!dir.join("TR_half.part").exists());
    }

    #[test]
    fn clear_empties_it() {
        let dir = scratch("clear");
        put(&dir, "TR:a", "mp3", 100, 0);
        put(&dir, "TR:b", "m4a", 100, 0);
        assert_eq!(clear(&dir), CacheStats::default());
    }

    /// Serve one HTTP response on a local port and return its URL. `declared` is the
    /// Content-Length to claim, which may be more than `body` to simulate a connection that
    /// drops mid-download.
    async fn serve_once(body: Vec<u8>, declared: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {declared}\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });
        format!("http://{addr}/audio")
    }

    #[tokio::test]
    async fn a_complete_download_is_stored_under_its_real_format() {
        let dir = scratch("store");
        let mut body = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
        body.resize(64 * 1024, 0x55);
        let url = serve_once(body.clone(), body.len()).await;

        let path = store(&reqwest::Client::new(), &dir, "TR:9", &url)
            .await
            .expect("stored");

        assert_eq!(path, dir.join("TR_9.mp3"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(find(&dir, "TR:9"), Some(path));
        assert!(
            !dir.join("TR_9.part").exists(),
            "the partial file was left behind"
        );
    }

    /// The failure that matters most: a song cut off mid-download would otherwise be cached
    /// and replayed cut off, every time it came round.
    #[tokio::test]
    async fn a_truncated_download_is_not_cached() {
        let dir = scratch("truncated");
        let mut body = b"ID3\x04\x00".to_vec();
        body.resize(10_000, 0x55);
        let url = serve_once(body, 50_000).await;

        assert!(store(&reqwest::Client::new(), &dir, "TR:9", &url)
            .await
            .is_err());
        assert_eq!(find(&dir, "TR:9"), None);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "debris left behind"
        );
    }

    #[tokio::test]
    async fn something_that_is_not_audio_is_not_cached() {
        let dir = scratch("not-audio");
        let body = b"<html>Access denied</html>".to_vec();
        let url = serve_once(body.clone(), body.len()).await;

        assert!(store(&reqwest::Client::new(), &dir, "TR:9", &url)
            .await
            .is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn a_missing_directory_is_an_empty_cache() {
        let dir = std::env::temp_dir().join("jarlid-cache-test-never-created");
        assert_eq!(stats(&dir), CacheStats::default());
        evict(&dir, 0, &[]);
    }
}
