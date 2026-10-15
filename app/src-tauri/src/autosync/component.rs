//! The downloadable speech engine: what it is, whether it is here, and getting it.
//!
//! Nothing of it ships in the installer. Turning auto-sync on downloads it into the app's data
//! directory: `jarlid-autosync.exe` and the engine DLLs (about 45 MB) and the speech model
//! (669 MB). See `plans/auto-synced-lyrics.md` §5.
//!
//! **Trust comes from one constant.** [`MANIFEST_SHA256`] pins the manifest, and the manifest
//! gives the size and SHA-256 of every other file, so a file is only ever used once its hash has
//! been checked against it. The executable and DLLs are checked again before every start of the
//! sidecar (they are small); the model, which is data and not code, at download.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

/// Where the published manifest lives. Released from an `autosync-v*` tag by
/// `.github/workflows/autosync-sidecar.yml`.
const MANIFEST_URL: &str =
    "https://github.com/cinderblock/jarlid/releases/download/autosync-v0.1.0/autosync-manifest.json";

/// SHA-256 of that manifest, as `package.ps1` printed it in the release run. Moving to a new
/// engine release means changing the URL and this together; empty would leave auto-sync
/// unavailable in release builds.
const MANIFEST_SHA256: &str = "2e41a04a9cc001f7f36e94b83397e7906861fb1a96d4469daf133f20f8152f45";

/// The sidecar protocol this app speaks. Must match `crates/autosync-sidecar/src/protocol.rs`.
pub const PROTOCOL: u32 = 1;

/// Development only: a manifest to use instead of the pinned one (a path, or a `file:` or
/// `https:` URL), so the whole download can be exercised before an engine is released. Its hash
/// is not checked, but every file it lists still is. Ignored in release builds.
const DEV_MANIFEST_ENV: &str = "JARLID_AUTOSYNC_MANIFEST";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub protocol: u32,
    pub files: Vec<Entry>,
    pub model: Entry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl Manifest {
    /// Every byte the download fetches.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum::<u64>() + self.model.size
    }
}

/// An installed engine, ready to start.
#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: Manifest,
    pub engine_dir: PathBuf,
    pub model: PathBuf,
}

impl Installed {
    pub fn exe(&self) -> PathBuf {
        self.engine_dir.join("jarlid-autosync.exe")
    }

    /// Re-hash the executable and DLLs against the manifest. Cheap (about 45 MB), and done before
    /// every start, so a file swapped on disk since the download is never run.
    pub fn verify_code(&self) -> Result<(), String> {
        for f in &self.manifest.files {
            let path = self.engine_dir.join(&f.name);
            let got = sha256_file(&path).map_err(|e| format!("{}: {e}", f.name))?;
            if got != f.sha256 {
                return Err(format!("{} does not match its download; reinstall", f.name));
            }
        }
        Ok(())
    }
}

/// What the Settings page shows.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Not offered in this build: no engine release is pinned yet.
    Unavailable,
    NotInstalled,
    Downloading {
        done: u64,
        total: u64,
    },
    Installed {
        bytes: u64,
        version: String,
    },
    Failed {
        message: String,
    },
}

/// Everything auto-sync keeps, under the app's data dir: it is a deliberate download, not a
/// cache for Windows to clear.
pub fn root(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("autosync"))
}

fn dev_manifest() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(DEV_MANIFEST_ENV)
            .ok()
            .filter(|v| !v.is_empty())
    } else {
        None
    }
}

/// Whether this build can offer auto-sync at all.
pub fn available() -> bool {
    !MANIFEST_SHA256.is_empty() || dev_manifest().is_some()
}

/// The installed engine, if every file it needs is present at the size the manifest says.
pub fn installed(app: &AppHandle) -> Option<Installed> {
    let root = root(app)?;
    let bytes = std::fs::read(root.join("manifest.json")).ok()?;
    // The copy on disk is only trusted if it is still the pinned one (or, in development, the
    // override): a manifest edited in place must not be able to vouch for different files.
    if dev_manifest().is_none() && hex(&Sha256::digest(&bytes)) != MANIFEST_SHA256 {
        return None;
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.protocol != PROTOCOL {
        return None;
    }
    let engine_dir = root.join(format!("engine-{}", manifest.version));
    let model = root.join("models").join(&manifest.model.name);
    let sized = |p: &Path, size: u64| p.metadata().map(|m| m.len() == size).unwrap_or(false);
    let complete = manifest
        .files
        .iter()
        .all(|f| sized(&engine_dir.join(&f.name), f.size))
        && sized(&model, manifest.model.size);
    complete.then_some(Installed {
        manifest,
        engine_dir,
        model,
    })
}

/// The status, without touching the network.
pub fn status(app: &AppHandle) -> Status {
    if !available() {
        return Status::Unavailable;
    }
    match installed(app) {
        Some(i) => Status::Installed {
            bytes: i.manifest.total_bytes(),
            version: i.manifest.version,
        },
        None => Status::NotInstalled,
    }
}

/// Fetch and check the manifest.
async fn fetch_manifest(http: &reqwest::Client) -> Result<(Vec<u8>, Manifest), String> {
    let (bytes, pinned) = match dev_manifest() {
        Some(src) => {
            eprintln!("[autosync] using development manifest {src}; its own hash is not checked");
            (read_source(http, &src).await?, false)
        }
        None => (read_source(http, MANIFEST_URL).await?, true),
    };
    if pinned && hex(&Sha256::digest(&bytes)) != MANIFEST_SHA256 {
        return Err("the engine manifest does not match the one this version expects".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("engine manifest: {e}"))?;
    if manifest.protocol != PROTOCOL {
        return Err(format!(
            "engine speaks protocol {}, this version needs {PROTOCOL}",
            manifest.protocol
        ));
    }
    Ok((bytes, manifest))
}

/// A whole small file, from a URL or (development only) a local path.
async fn read_source(http: &reqwest::Client, src: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = local_path(src) {
        return std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()));
    }
    let r = http
        .get(src)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    Ok(r.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// A local file named by `src`, in development builds only. Release builds download over HTTPS
/// and nothing else.
fn local_path(src: &str) -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    if let Some(rest) = src.strip_prefix("file:///") {
        return Some(PathBuf::from(rest));
    }
    (!src.starts_with("http://") && !src.starts_with("https://")).then(|| PathBuf::from(src))
}

/// Download and verify everything the manifest lists, reporting progress through `progress`.
///
/// Files already present with the right hash are kept, and a partial download is resumed where
/// the server allows it, so a retry after a lost connection doesn't start the 669 MB model over.
/// `stop` is polled between chunks; setting it abandons the install (the partial files stay, for
/// a later resume).
pub async fn install(
    app: &AppHandle,
    stop: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Installed, String> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("Jarlid/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let (manifest_bytes, manifest) = fetch_manifest(&http).await?;
    let root = root(app).ok_or("no data directory")?;
    let engine_dir = root.join(format!("engine-{}", manifest.version));
    let models = root.join("models");
    std::fs::create_dir_all(&engine_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&models).map_err(|e| e.to_string())?;

    let total = manifest.total_bytes();
    let mut done = 0u64;
    let jobs = manifest
        .files
        .iter()
        .map(|f| (f, engine_dir.join(&f.name)))
        .chain(std::iter::once((
            &manifest.model,
            models.join(&manifest.model.name),
        )));
    for (entry, dest) in jobs {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let base = done;
        fetch(&http, entry, &dest, stop, |n| progress(base + n, total)).await?;
        done += entry.size;
        progress(done, total);
    }

    // Only now does the install exist: `installed` looks for this file.
    std::fs::write(root.join("manifest.json"), &manifest_bytes).map_err(|e| e.to_string())?;
    remove_others(&root, &engine_dir, &models.join(&manifest.model.name));
    installed(app).ok_or_else(|| "installed files did not check out".into())
}

/// One file, verified, at `dest`. Skips the download if `dest` already holds the right bytes.
async fn fetch(
    http: &reqwest::Client,
    entry: &Entry,
    dest: &Path,
    stop: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    if dest
        .metadata()
        .map(|m| m.len() == entry.size)
        .unwrap_or(false)
        && hash_off_thread(dest).await.ok().as_deref() == Some(entry.sha256.as_str())
    {
        return Ok(());
    }
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));

    if let Some(src) = local_path(&entry.url) {
        std::fs::copy(&src, &part).map_err(|e| format!("{}: {e}", src.display()))?;
    } else {
        // Resume from what an earlier attempt left, if the server agrees to send the rest.
        let have = part.metadata().map(|m| m.len()).unwrap_or(0);
        let mut request = http.get(&entry.url);
        if have > 0 && have < entry.size {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{}: {e}", entry.name))?;
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .map_err(|e| format!("{}: {e}", part.display()))?;
        let mut written = if resumed { have } else { 0 };
        progress(written);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("{}: {e}", entry.name))?
        {
            if stop.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            written += chunk.len() as u64;
            if written > entry.size {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err(format!("{} is larger than expected", entry.name));
            }
            progress(written);
        }
    }

    let got = hash_off_thread(&part).await?;
    if got != entry.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{} failed its integrity check", entry.name));
    }
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Delete engine versions and models other than the ones just installed.
fn remove_others(root: &Path, engine_dir: &Path, model: &Path) {
    let Ok(dir) = std::fs::read_dir(root) else {
        return;
    };
    for e in dir.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("engine-") && p != engine_dir {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
    if let Ok(models) = std::fs::read_dir(root.join("models")) {
        for e in models.flatten() {
            if e.path() != model && !e.path().to_string_lossy().ends_with(".part") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Delete everything auto-sync downloaded. The sidecar must already be stopped: Windows won't
/// delete a running executable.
pub fn remove(app: &AppHandle) -> Result<(), String> {
    let Some(root) = root(app) else {
        return Ok(());
    };
    match std::fs::remove_dir_all(&root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Tell the Settings page.
pub fn emit(app: &AppHandle, status: &Status) {
    let _ = app.emit("autosync://status", status);
}

/// [`sha256_file`] on a blocking thread: the model takes seconds to hash, which is too long to
/// hold an async worker.
async fn hash_off_thread(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_what_package_ps1_writes() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.1.0","protocol":1,
                "files":[{"name":"jarlid-autosync.exe","url":"u","sha256":"ab","size":10}],
                "model":{"name":"m.bin","url":"v","sha256":"cd","size":90}}"#,
        )
        .unwrap();
        assert_eq!(m.total_bytes(), 100);
        assert_eq!(m.files[0].name, "jarlid-autosync.exe");
    }

    /// Against the real release: the pinned manifest checks out, and a file it lists downloads
    /// and verifies, resuming from a partial `.part` left by an interrupted attempt.
    /// `cargo test -- --ignored pinned_release`.
    #[test]
    #[ignore = "downloads from GitHub"]
    fn pinned_release_downloads_and_verifies() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let http = reqwest::Client::new();
            let (_, manifest) = fetch_manifest(&http).await.expect("pinned manifest");
            assert_eq!(manifest.protocol, PROTOCOL);
            let entry = manifest
                .files
                .iter()
                .find(|f| f.name == "ggml.dll")
                .expect("ggml.dll listed");

            let dir = std::env::temp_dir().join("jarlid-autosync-fetch-test");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dest = dir.join(&entry.name);
            // Half a file from an earlier, interrupted attempt.
            let whole = http
                .get(&entry.url)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            std::fs::write(dir.join("ggml.dll.part"), &whole[..whole.len() / 2]).unwrap();

            let stop = AtomicBool::new(false);
            let mut seen = 0;
            fetch(&http, entry, &dest, &stop, |n| seen = n)
                .await
                .expect("fetch");
            assert_eq!(seen, entry.size);
            assert_eq!(sha256_file(&dest).unwrap(), entry.sha256);
            // Present and right: a second fetch downloads nothing.
            fetch(&http, entry, &dest, &stop, |_| panic!("re-downloaded"))
                .await
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn hashes_files() {
        let dir = std::env::temp_dir().join("jarlid-autosync-hash-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abc.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
