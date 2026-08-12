import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import * as stationsPage from "./stations-page";
import type { StationInfo } from "./stations-page";
import * as settingsPage from "./settings-page";
import * as lyricEditor from "./lyric-editor";
import type { Lyrics } from "./lyric-editor";

// ---- types -------------------------------------------------------------
interface NowPlaying {
  title: string;
  artist: string;
  album: string;
  station: string;
  /// On QuickMix, which contributing station this track came from. Empty on an ordinary station.
  sourceStation: string;
  art: string;
  artFallback: string;
  thumbUp: boolean;
  thumbDown: boolean;
}
interface Playhead {
  position: number;
  duration: number;
  paused: boolean;
  volume: number;
}
interface LyricLine {
  t: number;
  text: string;
}
interface RemoteState {
  device: string;
  playing: boolean;
  title: string;
  artist: string;
  album: string;
  art: string;
  position: number;
  duration: number;
  volume: number;
}

// ---- element helpers ---------------------------------------------------
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const player = $("player");
const loginHint = $("login-hint");
const bg = $("bg");
const artEl = $<HTMLImageElement>("art");
const titleEl = $("title");
const titleInner = $("title-inner");
const artistEl = $("artist");
const albumEl = $("album");
const stationBtn = $("station");
const stationPanel = $("station-panel");
const stationSearch = $<HTMLInputElement>("station-search");
const stationList = $("station-list");
const stationAllLink = $<HTMLButtonElement>("station-all-link");
const sourceEl = $("source-station");
const histEl = $("history");
const barEl = $("bar");
const tCur = $("t-cur");
const tDur = $("t-dur");
const playIcon = $("play-icon");
const pauseIcon = $("pause-icon");
const thumbUpBtn = $("thumbUp");
const thumbDownBtn = $("thumbDown");
const lyricsEl = $("lyrics");
const lyricsStatus = $("lyrics-status");
const lyricsEditBtn = $("lyrics-edit");

const fmt = (s: number) => {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
};

// ---- state -------------------------------------------------------------
let currentKey = "";
let syncedLines: LyricLine[] | null = null;
// What the lyrics pane is currently showing, and the track naming it was fetched under —
// both needed to edit, and to file the edit against the right track.
let lastLyrics: Lyrics | null = null;
let lastMeta = { title: "", artist: "", album: "" };
// remote (network player) mode
let remote: RemoteState | null = null;
let remoteAt = 0; // Date.now() when the last remote state arrived
let remoteMode = false;
let lastLocalPlayingAt = 0;
let lastLocalNp: NowPlaying | null = null;
/// `activeLineIdx` before anything has been highlighted. It cannot be -1, because -1 is
/// a real state now — "playing, but the first line hasn't come round yet" — and the
/// highlighter skips work when the index is unchanged.
const NO_LINE = -2;
let activeLineIdx = NO_LINE;
let lastPlayhead: Playhead = { position: 0, duration: 0, paused: true, volume: 1 };

// ---- LRC parsing -------------------------------------------------------
function parseLrc(lrc: string): LyricLine[] {
  const out: LyricLine[] = [];
  const re = /\[(\d+):(\d+)(?:[.:](\d+))?\]/g;
  for (const raw of lrc.split(/\r?\n/)) {
    const stamps: number[] = [];
    let m: RegExpExecArray | null;
    re.lastIndex = 0;
    while ((m = re.exec(raw)) !== null) {
      const min = parseInt(m[1], 10);
      const sec = parseInt(m[2], 10);
      const frac = m[3] ? parseInt(m[3].padEnd(3, "0").slice(0, 3), 10) / 1000 : 0;
      stamps.push(min * 60 + sec + frac);
    }
    const text = raw.replace(re, "").trim();
    for (const t of stamps) out.push({ t, text });
  }
  out.sort((a, b) => a.t - b.t);
  return out;
}

// Songs rarely start singing at 0:00, and until the first timestamp there is no line to
// highlight — so the pane had nothing to scroll to, sat at the top, and then jumped when
// the first line finally landed. This synthetic row is the scroll target for that gap,
// and it doubles as a countdown so the wait doesn't read as "the lyrics didn't load".
// It is deliberately NOT a `.line`: highlightLine indexes `.line` nodes positionally.
const INTRO_MIN = 2;
let introEl: HTMLElement | null = null;
let introBar: HTMLElement | null = null;

function renderSyncedLyrics(lines: LyricLine[]) {
  lyricsEl.innerHTML = "";
  lyricsEl.classList.add("synced");
  introEl = introBar = null;

  if ((lines[0]?.t ?? 0) >= INTRO_MIN) {
    introEl = document.createElement("div");
    introEl.className = "lyric-intro";
    introBar = document.createElement("i");
    introEl.appendChild(introBar);
    lyricsEl.appendChild(introEl);
  }

  lines.forEach((ln, i) => {
    const div = document.createElement("div");
    div.className = "line";
    div.dataset.idx = String(i);
    div.textContent = ln.text || " ";
    lyricsEl.appendChild(div);
  });
}

function renderPlainLyrics(text: string) {
  lyricsEl.classList.remove("synced");
  lyricsEl.innerHTML = "";
  introEl = introBar = null;
  for (const raw of text.split(/\r?\n/)) {
    const div = document.createElement("div");
    div.className = "line plain";
    div.textContent = raw || " ";
    lyricsEl.appendChild(div);
  }
}

// Manual lyric sync offset (seconds), per-track, persisted. Nudge with [ and ].
let syncOffset = 0;

function highlightLine(position: number) {
  // Edit mode owns the pane: rows may have no timestamp yet, so the ascending-order
  // assumption below does not hold, and the editor drives its own cursor instead.
  if (lyricEditor.isEditing()) return;
  if (!syncedLines || syncedLines.length === 0) return;
  const p = position + syncOffset;
  let idx = -1;
  for (let i = 0; i < syncedLines.length; i++) {
    if (syncedLines[i].t <= p + 0.15) idx = i;
    else break;
  }

  // The countdown has to move on every tick, so it is updated before the
  // nothing-changed shortcut below.
  if (introEl && introBar) {
    introEl.classList.toggle("past", idx >= 0);
    const until = syncedLines[0].t;
    introBar.style.transform = `scaleX(${Math.max(0, Math.min(1, p / until))})`;
  }

  if (idx === activeLineIdx) return;
  activeLineIdx = idx;
  const nodes = lyricsEl.querySelectorAll<HTMLElement>(".line");
  nodes.forEach((n) => {
    const i = Number(n.dataset.idx);
    n.classList.toggle("past", i < idx);
    n.classList.toggle("active", i === idx);
  });
  // Before the first line, the countdown row is what the pane centres on — otherwise
  // there is no target and the first line arrives with a jump.
  const active = idx < 0 ? introEl : nodes[idx];
  if (active) active.scrollIntoView({ block: "center", behavior: "smooth" });
}

// ---- now-playing -------------------------------------------------------
async function onNowPlaying(np: NowPlaying) {
  lastLocalNp = np;
  if (remoteMode) return; // remote overlay owns the screen; re-rendered on exit
  loginHint.hidden = true;
  player.hidden = false;

  titleInner.textContent = np.title || "—";
  titleInner.style.transform = "translateX(0)";
  // mark whether the title fits (hide the edge-fade hint if it does)
  requestAnimationFrame(() =>
    titleEl.classList.toggle("fits", titleInner.scrollWidth <= titleEl.clientWidth + 1)
  );
  artistEl.textContent = np.artist || "";
  albumEl.textContent = np.album || "";
  if (np.station) stationBtn.textContent = np.station;
  // QuickMix blends many stations; without this there's no way to tell which one is playing.
  sourceEl.textContent = np.sourceStation ? `from ${np.sourceStation}` : "";
  sourceEl.hidden = !np.sourceStation;
  setThumbs(np.thumbUp, np.thumbDown);

  setArt(np.art || np.artFallback, np.artFallback);

  // Key on title+artist only. Album flickers empty when Pandora collapses the
  // now-playing view during a long pause; including it would churn the key and
  // wipe the lyrics until the next song.
  const key = `${np.title}|${np.artist}`;
  if (key !== currentKey) {
    currentKey = key;
    syncedLines = null;
    activeLineIdx = NO_LINE;
    syncOffset = parseFloat(localStorage.getItem(`syncoff:${key}`) || "0") || 0;
    // An editor left open is still editing the previous track — which is right — but
    // the playhead it was timing against now belongs to this one.
    lyricEditor.notePlaybackMoved();
    pushHistory(np);
    await loadLyrics(np);
  }
}

// Wait briefly for the NEW track's duration before looking up lyrics — the
// fetch fires on title change, before the playhead reflects the new track,
// and without a duration LRCLIB version-matching can pick the wrong edit.
async function waitForDuration(key: string): Promise<number | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < 2500) {
    if (currentKey !== key) return null; // track changed under us
    const d = lastPlayhead.duration;
    if (d > 0 && lastPlayhead.position < d && lastPlayhead.position < 30) return d;
    await new Promise((r) => setTimeout(r, 200));
  }
  return lastPlayhead.duration > 0 ? lastPlayhead.duration : null;
}

// ---- recently-played gallery -------------------------------------------
interface HistItem {
  art: string;
  title: string;
  artist: string;
  album?: string;
  at?: number;
}
let history: HistItem[] = [];
try {
  history = JSON.parse(localStorage.getItem("history") || "[]");
} catch {
  history = [];
}
const histModal = $("hist-modal");
const hmArt = $<HTMLImageElement>("hm-art");
const hmTitle = $("hm-title");
const hmArtist = $("hm-artist");
const hmAlbum = $("hm-album");
const hmWhen = $("hm-when");

function openHistModal(h: HistItem) {
  hmArt.src = h.art;
  hmTitle.textContent = h.title;
  hmArtist.textContent = h.artist;
  hmAlbum.textContent = h.album || "";
  hmWhen.textContent = h.at ? `Played ${new Date(h.at).toLocaleString()}` : "";
  histModal.hidden = false;
}
histModal.addEventListener("click", (e) => {
  if (!(e.target as HTMLElement).closest(".hm-card")) histModal.hidden = true;
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") histModal.hidden = true;
});

// Custom tooltip (no native title= tooltips anywhere in this app).
const tooltip = $("tooltip");
function attachTip(el: HTMLElement, text: () => string) {
  el.addEventListener("mouseenter", () => {
    tooltip.textContent = text();
    tooltip.hidden = false;
    const r = el.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left + r.width / 2 - tooltip.offsetWidth / 2, innerWidth - tooltip.offsetWidth - 8));
    tooltip.style.left = `${x}px`;
    tooltip.style.top = `${Math.max(4, r.top - tooltip.offsetHeight - 10)}px`;
  });
  el.addEventListener("mouseleave", () => (tooltip.hidden = true));
}

function renderHistory() {
  histEl.innerHTML = "";
  for (const h of history) {
    const wrap = document.createElement("div");
    wrap.className = "hist-wrap";
    const img = new Image();
    img.src = h.art;
    img.className = "hist-item";
    img.loading = "lazy";
    img.addEventListener("click", () => openHistModal(h));
    attachTip(img, () => `${h.title} — ${h.artist}`);
    wrap.appendChild(img);
    histEl.appendChild(wrap);
  }
  requestAnimationFrame(coverflow);
}

// Cover-Flow-style perspective: items tilt away from the strip's center as
// they approach the edges (the wrap gets the scroll-driven transform so the
// image's own hover zoom still works).
function coverflow() {
  const rect = histEl.getBoundingClientRect();
  if (rect.width === 0) return;
  const cx = rect.left + rect.width / 2;
  for (const el of Array.from(histEl.children) as HTMLElement[]) {
    const r = el.getBoundingClientRect();
    const d = Math.max(-1, Math.min(1, (r.left + r.width / 2 - cx) / (rect.width / 2)));
    el.style.transform = `perspective(420px) rotateY(${(-d * 32).toFixed(1)}deg) scale(${(
      1 - Math.abs(d) * 0.16
    ).toFixed(3)})`;
    el.style.opacity = (1 - Math.abs(d) * 0.45).toFixed(2);
    el.style.zIndex = String(100 - Math.round(Math.abs(d) * 50));
  }
}
histEl.addEventListener("scroll", () => requestAnimationFrame(coverflow));
window.addEventListener("resize", () => requestAnimationFrame(coverflow));
function pushHistory(np: NowPlaying) {
  const art = np.artFallback || np.art;
  if (!art || !np.title) return;
  if (history[0] && history[0].title === np.title && history[0].artist === np.artist) return;
  history.unshift({ art, title: np.title, artist: np.artist, album: np.album, at: Date.now() });
  history = history.slice(0, 40);
  localStorage.setItem("history", JSON.stringify(history));
  renderHistory();
}
renderHistory();
// Cap accumulated per-track sync offsets (no timestamps to age by; a rare
// full reset beats unbounded growth).
{
  const offKeys = Object.keys(localStorage).filter((k) => k.startsWith("syncoff:"));
  if (offKeys.length > 500) offKeys.forEach((k) => localStorage.removeItem(k));
}
// vertical wheel scrolls the strip horizontally
histEl.addEventListener(
  "wheel",
  (e) => {
    if (e.deltaY) {
      histEl.scrollLeft += e.deltaY;
      e.preventDefault();
    }
  },
  { passive: false }
);

// Preload art off-screen, then fade it in — avoids broken-image flashes and
// softens the stale-art-then-correct-art swap Pandora's DOM produces on load.
let artToken = 0;
function setArt(url: string, fallback: string) {
  if (!url || artEl.src === url) return;
  const token = ++artToken;
  const img = new Image();
  img.onload = () => {
    if (token !== artToken) return; // newer art superseded this one
    artEl.style.opacity = "0";
    setTimeout(() => {
      if (token !== artToken) return;
      artEl.src = url;
      bg.style.backgroundImage = `url("${url}")`;
      artEl.style.opacity = "1";
    }, 180);
  };
  img.onerror = () => {
    if (token === artToken && fallback && fallback !== url) setArt(fallback, "");
  };
  img.src = url;
}

function setThumbs(up: boolean, down: boolean) {
  thumbUpBtn.classList.toggle("active", !!up);
  thumbDownBtn.classList.toggle("active", !!down);
  thumbUpBtn.setAttribute("aria-pressed", up ? "true" : "false");
  thumbDownBtn.setAttribute("aria-pressed", down ? "true" : "false");
}

async function loadLyrics(np: NowPlaying) {
  const key = `${np.title}|${np.artist}`;
  lyricsStatus.textContent = "Loading lyrics…";
  // Keep any existing lyrics on screen until replacements arrive (no blank flash).
  const duration = await waitForDuration(key);
  if (currentKey !== key) return;
  await loadLyricsFor({ title: np.title, artist: np.artist, album: np.album }, duration, key);
}

async function loadLyricsFor(
  meta: { title: string; artist: string; album: string },
  duration: number | null,
  key: string
) {
  lyricsStatus.textContent = "Loading lyrics…";
  try {
    const res = await invoke<Lyrics>("fetch_lyrics", {
      artist: meta.artist,
      track: meta.title,
      album: meta.album || null,
      duration,
    });
    // Ignore if the track changed while we were fetching.
    if (key !== currentKey) return;

    lastMeta = meta;
    applyLyrics(res);
  } catch (e) {
    // Don't leave the pencil pointing at the last track's lyrics: `lastMeta` was not
    // updated, so editing now would file the edit against the wrong song.
    lastLyrics = null;
    lyricsEditBtn.hidden = true;
    syncedLines = null;
    introEl = introBar = null;
    lyricsEl.innerHTML = `<div class="line empty">Lyrics unavailable</div>`;
    lyricsStatus.textContent = "Lyrics";
  }
}

/// Paint a set of lyrics into the pane. Separate from fetching because the editor hands
/// back replacements that never went near the network.
function applyLyrics(res: Lyrics) {
  lastLyrics = res;
  // Even "nothing found" is worth an edit button — that is exactly when contributing
  // the words is most useful.
  lyricsEditBtn.hidden = false;
  const edited = res.overridden ? " · edited" : "";

  // Edit mode owns the pane's contents. Repainting under it would throw away the rows
  // and the caret mid-edit, so keep the state current and leave the DOM alone.
  if (lyricEditor.isEditing()) {
    syncedLines = res.synced ? parseLrc(res.synced) : null;
    lyricsStatus.textContent = `${res.synced ? "Synced lyrics" : "Lyrics"}${edited}`;
    return;
  }

  if (res.synced) {
    syncedLines = parseLrc(res.synced);
    renderSyncedLyrics(syncedLines);
    lyricsStatus.textContent = `Synced lyrics${edited}`;
    activeLineIdx = NO_LINE;
    highlightLine(lastPlayhead.position);
  } else if (res.plain) {
    syncedLines = null;
    renderPlainLyrics(res.plain);
    lyricsStatus.textContent = `Lyrics${edited}`;
  } else {
    syncedLines = null;
    introEl = introBar = null;
    lyricsEl.innerHTML = `<div class="line empty">No lyrics found</div>`;
    lyricsStatus.textContent = "Lyrics";
  }
}

lyricsEditBtn.addEventListener("click", () => {
  if (lyricEditor.isEditing()) {
    lyricEditor.end();
    return;
  }
  if (!lastLyrics) return;
  lyricsEditBtn.classList.add("on");
  lyricEditor.begin({
    meta: lastMeta,
    lyrics: lastLyrics,
    playhead: () => lastPlayhead,
    transport: (c) => void cmd(c),
    seek: (position) => void invoke("player_seek", { position }).catch(() => {}),
    canTransport: !remoteMode,
    onApplied: applyLyrics,
    // Repaint the pane the ordinary way and let the highlighter take over again.
    onExit: () => {
      lyricsEditBtn.classList.remove("on");
      activeLineIdx = NO_LINE;
      if (lastLyrics) applyLyrics(lastLyrics);
    },
  });
});
attachTip(lyricsEditBtn, () =>
  lyricEditor.isEditing()
    ? "Stop editing"
    : lastLyrics?.overridden
      ? "Edit lyrics (showing your local edit)"
      : "Fix or time these lyrics"
);

// ---- playhead ----------------------------------------------------------
// Playing/paused is derived from whether the position is actually moving —
// Pandora's DOM and <audio> elements both misreport paused state, but a
// advancing playhead cannot lie.
let lastPos = -1;
let lastMoveAt = 0;
function onPlayhead(ph: Playhead) {
  const wasPaused = lastPlayhead.paused;
  lastPlayhead = ph;
  // A staged update says "after this song" or "while paused" depending on this, so the
  // badge has to follow it.
  if (wasPaused !== ph.paused && status.armed) renderVersion();
  const now = Date.now();
  const moved = Math.abs(ph.position - lastPos) > 0.05;
  if (moved) {
    lastPos = ph.position;
    lastMoveAt = now;
    lastLocalPlayingAt = now; // local playback active — used for mode switching
    updateMode();
  }
  if (remoteMode) return; // remote overlay owns progress/icon/highlight

  const pct = ph.duration > 0 ? (ph.position / ph.duration) * 100 : 0;
  barEl.style.width = `${Math.min(100, pct)}%`;
  tCur.textContent = fmt(ph.position);
  tDur.textContent = fmt(ph.duration);
  if (moved) {
    if (now >= optimisticUntil) setPlayingIcon(true);
  } else if (now - lastMoveAt > 1600 && now >= optimisticUntil) {
    setPlayingIcon(false);
  }
  highlightLine(ph.position);
}

// ---- controls ----------------------------------------------------------
const cmd = (c: string) => invoke("player_cmd", { cmd: c }).catch(() => {});
// The icons are SVG elements: the `.hidden` PROPERTY does not exist on
// SVGElement (it's HTMLElement-only), so it must be the attribute.
let uiPlaying = false;
function setPlayingIcon(playing: boolean) {
  uiPlaying = playing;
  if (playing) {
    playIcon.setAttribute("hidden", "");
    pauseIcon.removeAttribute("hidden");
  } else {
    pauseIcon.setAttribute("hidden", "");
    playIcon.removeAttribute("hidden");
  }
}

// After a click, in-flight playhead ticks still carry pre-toggle motion; hold
// the optimistic state briefly so the icon doesn't flicker before settling.
let optimisticUntil = 0;
function togglePlayback() {
  const desired = !uiPlaying;
  setPlayingIcon(desired);
  optimisticUntil = Date.now() + 2000;
  if (remoteMode) {
    invoke("remote_cmd", { cmd: desired ? "play" : "pause" }).catch(() => {});
  } else {
    cmd("toggle");
  }
}
$("play").addEventListener("click", togglePlayback);
// Space toggles playback (unless typing in the station search)
window.addEventListener("keydown", (e) => {
  if (e.code === "Space" && (e.target as HTMLElement).tagName !== "INPUT") {
    e.preventDefault();
    togglePlayback();
  }
});
// The version badge is the entire (unobtrusive) update UI: shows the running version,
// then what is about to happen to it. Clicking walks the known -> staged -> armed -> now
// ladder one step per click; how far the app walks on its own is the Settings policy.
const versionEl = $("version");
let baseVersion = "";
let versionBusy = false;

type Policy = "instant" | "afterSong" | "manualInstall" | "notifyOnly";
interface UpdateStatus {
  available: string | null;
  staged: boolean;
  armed: boolean;
  policy: Policy;
}
let status: UpdateStatus = { available: null, staged: false, armed: false, policy: "afterSong" };

function renderVersion() {
  const v = status.available;
  versionEl.classList.toggle("update", !!v);
  if (!v) {
    versionEl.textContent = baseVersion;
    return;
  }
  if (!status.staged) {
    // Known about but not downloaded (notify-only, or a download that hasn't run yet).
    versionEl.textContent = `v${v} available`;
    return;
  }
  if (!status.armed) {
    // Downloaded and waiting to be asked.
    versionEl.textContent = `v${v} ready to install`;
    return;
  }
  // Armed: say when. Paused is the moment the updater prefers — it installs within about a
  // minute and comes back paused — so "after this song" would be a lie there.
  versionEl.textContent = lastPlayhead.paused
    ? `updating to v${v} while paused`
    : `updating to v${v} after this song`;
}

attachTip(versionEl, () => {
  const v = status.available;
  if (!v) return "Click to check for updates";
  if (!status.staged) return `Click to download v${v}`;
  if (!status.armed) return "Click to install after this song";
  return "Click to install now instead of waiting";
});

getVersion()
  .then((v) => {
    baseVersion = `v${v}`;
    renderVersion();
  })
  .catch(() => {});

invoke<UpdateStatus>("update_status")
  .then((s) => {
    status = s;
    renderVersion();
  })
  .catch(() => {});

listen<UpdateStatus>("app://update-status", (e) => {
  status = e.payload;
  renderVersion();
});

// The last thing painted before the process is replaced, so it should explain the
// silence that follows.
listen<string>("app://update-installing", (e) => {
  versionBusy = true;
  versionEl.classList.add("update");
  versionEl.textContent = `updating to v${e.payload}…`;
});
listen<string>("app://update-failed", () => {
  versionBusy = false;
  versionEl.textContent = "update failed";
  setTimeout(renderVersion, 2500);
});
// Playback started or stopped in the moment between the notice and the install, so the
// updater backed out. Nothing went wrong and it will try again — just undo the notice.
listen("app://update-stood-down", () => {
  versionBusy = false;
  renderVersion();
});

versionEl.addEventListener("click", async () => {
  if (versionBusy) return;
  versionBusy = true;
  const wasKnown = !!status.available;
  versionEl.textContent = status.staged ? "installing…" : "checking…";
  try {
    status = await invoke<UpdateStatus>("update_action");
    versionBusy = false;
    if (!status.available && !wasKnown) {
      versionEl.textContent = "up to date";
      setTimeout(renderVersion, 2200);
    } else {
      renderVersion();
    }
  } catch {
    versionBusy = false;
    versionEl.textContent = "check failed";
    setTimeout(renderVersion, 2200);
  }
});
$("skip").addEventListener("click", () =>
  remoteMode ? invoke("remote_cmd", { cmd: "skip" }).catch(() => {}) : cmd("skip")
);
$("replay").addEventListener("click", () => cmd("replay"));
thumbUpBtn.addEventListener("click", () => cmd("thumbUp"));
thumbDownBtn.addEventListener("click", () => cmd("thumbDown"));
// Sign in directly — there is no Pandora webview to log into any more. The password goes
// straight to the Rust side, which only persists it once Pandora has actually accepted it.
$("login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const user = $("login-user") as HTMLInputElement;
  const pass = $("login-pass") as HTMLInputElement;
  const submit = $("login-submit") as HTMLButtonElement;
  const error = $("login-error");

  error.hidden = true;
  submit.disabled = true;
  submit.textContent = "Signing in…";

  try {
    await invoke("native_sign_in", { username: user.value, password: pass.value });
    // Don't leave the password sitting in the DOM once it has been handed over.
    pass.value = "";
    loginHint.hidden = true;
  } catch (err) {
    error.textContent = String(err);
    error.hidden = false;
  } finally {
    submit.disabled = false;
    submit.textContent = "Sign in";
  }
});
// ---- station switching (searchable picker) -------------------------------
// The picker is a quick jump-to-station list only. Selecting stations for export
// lives on the Stations page, which has room for a long run's progress.
// A station is identified by its tuner token: the name is not unique.
let stations: StationInfo[] = [];
let activeStation = "";

function renderStationList(filter = "") {
  const f = filter.trim().toLowerCase();
  stationList.innerHTML = "";
  for (const st of stations) {
    if (f && !st.name.toLowerCase().includes(f)) continue;
    const item = document.createElement("div");
    item.className = "station-item" + (st.name === activeStation ? " active" : "");
    item.textContent = st.name;
    item.addEventListener("click", () => {
      invoke("native_play_station", { name: st.name, token: st.token }).catch(() => {});
      activeStation = st.name;
      stationBtn.textContent = st.name;
      stationPanel.hidden = true;
    });
    stationList.appendChild(item);
  }
}

stationBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  stationPanel.hidden = !stationPanel.hidden;
  if (!stationPanel.hidden) {
    stationSearch.value = "";
    renderStationList();
    stationSearch.focus();
  }
});
stationSearch.addEventListener("input", () => renderStationList(stationSearch.value));
stationAllLink.addEventListener("click", (e) => {
  e.stopPropagation();
  stationPanel.hidden = true;
  stationsPage.open();
});
window.addEventListener("click", (e) => {
  if (!stationPanel.hidden && !(e.target as HTMLElement).closest("#station-wrap")) {
    stationPanel.hidden = true;
  }
});
window.addEventListener("keydown", (e) => {
  // The pages own Escape while they are up.
  if (e.key === "Escape" && !stationsPage.isOpen()) stationPanel.hidden = true;
});

listen<{ stations: StationInfo[] }>("engine://stations", (e) => {
  const next = e.payload.stations;
  if (!next?.length) return;
  stations = next;
  stationsPage.setStations(next, activeStation);
  // The station list arriving means we know what's playing, so its modes are fetchable.
  void refreshModes();
  if (!stationPanel.hidden) renderStationList(stationSearch.value);
});

// ---- top-right pages -----------------------------------------------------
$("stations-btn").addEventListener("click", (e) => {
  e.stopPropagation();
  stationsPage.open();
});
$("settings-btn").addEventListener("click", (e) => {
  e.stopPropagation();
  settingsPage.open();
});
// index.html paints from a cached preference before the first frame; this is the
// authoritative answer arriving a moment later.
void settingsPage.applyStoredTheme();
attachTip($("stations-btn"), () => "All stations — browse, export");
attachTip($("settings-btn"), () => "Settings");

// ---- station modes (My Station / Crowd Faves / Discovery / Deep Cuts …) ----
// Modes are set over Pandora's REST API but the tuner playlist honours them, so switching here
// really does change what plays next. It affects newly generated playlists, not the current
// track — the engine clears its queue so the change is audible within a song or two.
type Mode = {
  modeId: number;
  modeName: string;
  modeButtonText: string;
  modeDescription: string;
  isInitialMode: boolean;
};

const modeWrap = $("mode-wrap");
const modeBtn = $("mode-btn");
const modePanel = $("mode-panel");
const modeList = $("mode-list");
let modes: Mode[] = [];
let activeMode = "";

function renderModes() {
  modeList.innerHTML = "";
  modes.forEach((mode) => {
    const label = mode.modeButtonText || mode.modeName;
    const item = document.createElement("div");
    item.className = "mode-item" + (label === activeMode ? " active" : "");

    const name = document.createElement("div");
    name.className = "mode-name";
    name.textContent = label;
    item.appendChild(name);

    // Pandora's own one-liner. Shown inline rather than on hover — the names alone don't
    // explain themselves, and hover text is invisible on a touchscreen.
    if (mode.modeDescription) {
      const desc = document.createElement("div");
      desc.className = "mode-desc";
      desc.textContent = mode.modeDescription;
      item.appendChild(desc);
    }

    item.addEventListener("click", () => {
      activeMode = label;
      modeBtn.textContent = label;
      modePanel.hidden = true;
      invoke("native_set_mode", { modeId: mode.modeId }).catch(() => {
        // Roll back if Pandora rejected it, rather than showing a mode that isn't set.
        void refreshModes();
      });
      renderModes();
    });
    modeList.appendChild(item);
  });
}

async function refreshModes() {
  try {
    modes = await invoke<Mode[]>("native_modes");
  } catch {
    modes = [];
  }
  // A station with only the default mode has nothing worth choosing between.
  modeWrap.hidden = modes.length < 2;
  if (!modes.length) return;
  if (!activeMode) {
    const initial = modes.find((m) => m.isInitialMode) ?? modes[0];
    activeMode = initial.modeButtonText || initial.modeName;
  }
  modeBtn.textContent = activeMode;
  renderModes();
}

modeBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  modePanel.hidden = !modePanel.hidden;
  if (!modePanel.hidden) renderModes();
});
window.addEventListener("click", (e) => {
  if (!modePanel.hidden && !(e.target as HTMLElement).closest("#mode-wrap")) {
    modePanel.hidden = true;
  }
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") modePanel.hidden = true;
});
listen<{ mode: string }>("engine://mode", (e) => {
  if (e.payload.mode) {
    activeMode = e.payload.mode;
    modeBtn.textContent = activeMode;
    renderModes();
  }
});

// ---- title marquee: hover to scrub a long title with the mouse x-position ----
titleEl.addEventListener("mousemove", (e) => {
  const overflow = titleInner.scrollWidth - titleEl.clientWidth;
  if (overflow <= 0) {
    titleInner.style.transform = "translateX(0)";
    return;
  }
  const rect = titleEl.getBoundingClientRect();
  const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
  titleInner.style.transform = `translateX(${-ratio * overflow}px)`;
});
titleEl.addEventListener("mouseleave", () => {
  titleInner.style.transform = "translateX(0)";
});

// ---- lyric sync nudge: [ = earlier, ] = later (0.25s steps, per-track) ----
function flashStatus(text: string) {
  lyricsStatus.textContent = text;
  lyricsStatus.classList.remove("flash");
  void lyricsStatus.offsetWidth; // restart the animation
  lyricsStatus.classList.add("flash");
}
function nudgeSync(delta: number) {
  if (!syncedLines) {
    flashStatus("No synced lyrics to nudge");
    return;
  }
  syncOffset = Math.round((syncOffset + delta) * 100) / 100;
  localStorage.setItem(`syncoff:${currentKey}`, String(syncOffset));
  activeLineIdx = NO_LINE; // force re-highlight
  highlightLine(lastPlayhead.position);
  flashStatus(
    syncOffset === 0
      ? "Synced lyrics · offset cleared"
      : `Synced lyrics · offset ${syncOffset > 0 ? "+" : ""}${syncOffset.toFixed(2)}s`
  );
}
window.addEventListener("keydown", (e) => {
  const el = e.target as HTMLElement;
  if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) return;
  // Edit mode binds Space, and a whole-file offset means nothing while individual lines
  // are being retimed — the per-line times are the thing being fixed.
  if (lyricEditor.isEditing()) return;
  if (e.key === "[" || e.code === "BracketLeft") nudgeSync(-0.25);
  else if (e.key === "]" || e.code === "BracketRight") nudgeSync(0.25);
});

// SMTC (media keys / Windows panel) pressed: reflect the state immediately
// instead of waiting for motion confirmation.
listen<{ playing: boolean }>("player://optimistic", (e) => {
  setPlayingIcon(e.payload.playing);
  optimisticUntil = Date.now() + 2000;
});

// ---- remote (network player) mode ----------------------------------------
// When the local engine is idle and a UPnP/DLNA renderer on the LAN is
// playing, the UI becomes a display for it: art, metadata, synced lyrics.
const remoteBadge = $("remote-badge");

const remoteKey = (r: RemoteState) => `R|${r.title}|${r.artist}|${r.album}`;

function renderRemote(r: RemoteState) {
  const key = remoteKey(r);
  if (key === currentKey) return;
  titleInner.textContent = r.title || "—";
  titleInner.style.transform = "translateX(0)";
  requestAnimationFrame(() =>
    titleEl.classList.toggle("fits", titleInner.scrollWidth <= titleEl.clientWidth + 1)
  );
  artistEl.textContent = r.artist || "";
  albumEl.textContent = r.album || "";
  remoteBadge.textContent = `Now playing on ${r.device}`;
  if (r.art) setArt(r.art, "");
  currentKey = key;
  syncedLines = null;
  activeLineIdx = NO_LINE;
  syncOffset = parseFloat(localStorage.getItem(`syncoff:${key}`) || "0") || 0;
  loadLyricsFor({ title: r.title, artist: r.artist, album: r.album }, r.duration || null, key);
}

function updateMode() {
  const localRecent = Date.now() - lastLocalPlayingAt < 3000;
  const want = !!remote && remote.playing && !!remote.title && !localRecent;
  if (want === remoteMode) {
    if (remoteMode && remote) renderRemote(remote); // track change within remote mode
    return;
  }
  remoteMode = want;
  document.body.classList.toggle("remote", remoteMode);
  remoteBadge.hidden = !remoteMode;
  currentKey = ""; // force full re-render for the new source
  if (remoteMode && remote) {
    loginHint.hidden = true;
    player.hidden = false;
    renderRemote(remote);
    setPlayingIcon(remote.playing);
  } else if (lastLocalNp) {
    onNowPlaying(lastLocalNp);
  }
}

let remoteDevice = "";
listen<RemoteState>("remote://state", (e) => {
  const st = e.payload;
  remoteDevice = st?.device || "";
  speakersBtn.hidden = !remoteDevice;
  remote = st && st.title ? st : null;
  remoteAt = Date.now();
  updateMode();
  reflectRemoteVolume();
});

// ---- "Play on Speakers": the network player's presets --------------------
interface Preset {
  number: number;
  name: string;
  source: string;
  art: string;
}
const speakersBtn = $("speakers-btn");
const speakersPanel = $("speakers-panel");
const speakersHead = $("speakers-head");
const speakersList = $("speakers-list");

speakersBtn.addEventListener("click", async (ev) => {
  ev.stopPropagation();
  if (!speakersPanel.hidden) {
    speakersPanel.hidden = true;
    return;
  }
  speakersPanel.hidden = false;
  speakersHead.textContent = `Play on ${remoteDevice || "speakers"}`;
  speakersList.innerHTML = `<div class="sp-empty">Loading presets…</div>`;
  try {
    const presets = await invoke<Preset[]>("remote_presets");
    speakersList.innerHTML = "";
    if (!presets.length) {
      speakersList.innerHTML = `<div class="sp-empty">No presets configured — add them in the WiiM Home app.</div>`;
      return;
    }
    for (const p of presets) {
      const item = document.createElement("div");
      item.className = "preset-item";
      if (p.art) {
        const img = new Image();
        img.src = p.art;
        img.className = "preset-art";
        img.onerror = () => img.remove();
        item.appendChild(img);
      }
      const text = document.createElement("div");
      const name = document.createElement("div");
      name.className = "preset-name";
      name.textContent = p.name;
      text.appendChild(name);
      if (p.source) {
        const src = document.createElement("div");
        src.className = "preset-source";
        src.textContent = p.source;
        text.appendChild(src);
      }
      item.appendChild(text);
      item.addEventListener("click", () => {
        invoke("remote_cmd", { cmd: `preset:${p.number}` }).catch(() => {});
        speakersPanel.hidden = true;
      });
      speakersList.appendChild(item);
    }
  } catch (err) {
    speakersList.innerHTML = `<div class="sp-empty">${String(err)}</div>`;
  }
});
window.addEventListener("click", (e) => {
  if (!speakersPanel.hidden && !(e.target as HTMLElement).closest("#speakers-panel, #speakers-btn")) {
    speakersPanel.hidden = true;
  }
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") speakersPanel.hidden = true;
});

// ---- remote volume slider -------------------------------------------------
const remoteVol = $<HTMLInputElement>("remote-vol");
let volDragging = false;
let volSendTimer: number | undefined;
remoteVol.addEventListener("pointerdown", () => (volDragging = true));
remoteVol.addEventListener("pointerup", () => (volDragging = false));
remoteVol.addEventListener("input", () => {
  clearTimeout(volSendTimer);
  volSendTimer = window.setTimeout(() => {
    invoke("remote_cmd", { cmd: `vol:${remoteVol.value}` }).catch(() => {});
  }, 150);
});
function reflectRemoteVolume() {
  if (!remote || volDragging) return;
  if (remote.volume >= 0) remoteVol.value = String(Math.round(remote.volume));
  remoteVol.parentElement!.style.visibility = remote.volume >= 0 ? "visible" : "hidden";
}

// Interpolate the remote position between the 1s device polls.
setInterval(() => {
  if (!remoteMode || !remote) return;
  let pos = remote.position + (remote.playing ? (Date.now() - remoteAt) / 1000 : 0);
  if (remote.duration > 0) pos = Math.min(pos, remote.duration);
  const pct = remote.duration > 0 ? (pos / remote.duration) * 100 : 0;
  barEl.style.width = `${Math.min(100, pct)}%`;
  tCur.textContent = fmt(pos);
  tDur.textContent = fmt(remote.duration);
  if (Date.now() >= optimisticUntil) setPlayingIcon(remote.playing);
  highlightLine(pos);
}, 400);

// ---- events from the engine bridge ------------------------------------
listen<NowPlaying>("engine://nowplaying", (e) => onNowPlaying(e.payload));
listen<Playhead>("engine://playhead", (e) => onPlayhead(e.payload));
listen<{ thumbUp: boolean; thumbDown: boolean }>("engine://thumbs", (e) =>
  setThumbs(e.payload.thumbUp, e.payload.thumbDown)
);
listen("engine://needs-login", () => {
  // Authoritative now: the native engine emits this only when the credential store is empty or
  // the saved credentials were rejected. The old `everPlayed` guard existed because Pandora's
  // page fired spurious login signals during its initial load; there is no page any more.
  player.hidden = true;
  loginHint.hidden = false;
});

// ---- toast --------------------------------------------------------------
// Engine problems used to be written into #login-error, which lives inside the sign-in card and
// is hidden the moment you are signed in — so a failure during playback went nowhere at all.
const toast = $("toast");
const toastMsg = $("toast-msg");
const toastAction = $<HTMLButtonElement>("toast-action");
let toastTimer: number | undefined;

function showToast(
  message: string,
  action?: { label: string; run: () => void },
  autoHideMs = 8000
) {
  toastMsg.textContent = message;
  toastAction.hidden = !action;
  if (action) {
    toastAction.textContent = action.label;
    toastAction.onclick = () => {
      hideToast();
      action.run();
    };
  }
  toast.hidden = false;
  window.clearTimeout(toastTimer);
  // Sticky when there is something to do about it; a toast that vanishes before you can click
  // its button is worse than none.
  if (!action) toastTimer = window.setTimeout(hideToast, autoHideMs);
}

function hideToast() {
  toast.hidden = true;
  window.clearTimeout(toastTimer);
}
$("toast-close").addEventListener("click", hideToast);

// ---- problem reporting --------------------------------------------------
// Everything sent to GitHub is redacted backend-side (see diagnostics.rs) and lands in GitHub's
// editor for review — nothing is ever submitted on the user's behalf.
function reportIssue(note = "") {
  invoke("native_report_issue", {
    context: {
      userAgent: navigator.userAgent,
      station: stationBtn.textContent ?? "",
      sourceStation: sourceEl.textContent ?? "",
      mode: modeBtn.textContent ?? "",
      remote: remoteMode,
      note,
    },
  }).catch((err) => showToast(`Could not open the issue page: ${err}`));
}

/** Record a problem for the next bug report, whether or not one is filed now. */
function recordIncident(source: string, message: string) {
  invoke("native_record_incident", { source, message }).catch(() => {});
}

const REPORT = { label: "Report issue", run: () => reportIssue() };

listen<{ message: string }>("engine://error", (e) =>
  showToast(e.payload.message, REPORT)
);

// A thrown error in the UI used to leave the interface subtly wrong with nothing said about it.
window.addEventListener("error", (e) => {
  const where = e.filename ? ` (${e.filename}:${e.lineno})` : "";
  recordIncident("ui", `${e.message}${where}`);
  showToast("Something went wrong in the interface.", REPORT);
});

// Unhandled rejections are the common shape here: every invoke() returns a promise.
window.addEventListener("unhandledrejection", (e) => {
  recordIncident("ui", `unhandled rejection: ${e.reason}`);
  showToast("Something went wrong in the interface.", REPORT);
});

// Another device holds the account's single stream. Recoverable, and the engine keeps retrying
// on its own — so say so, and offer to claim it now rather than waiting.
listen<{ message: string }>("engine://stream-taken", (e) => {
  showToast(e.payload.message, {
    label: "Play here",
    run: () => {
      invoke("native_take_over")
        .then(() => showToast("Playing here now.", undefined, 3000))
        .catch(() => {
          recordIncident("engine", "take over failed");
          showToast(
            "Could not take over — the other device still has it.",
            REPORT
          );
        });
    },
  });
});
