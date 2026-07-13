import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";

// ---- types -------------------------------------------------------------
interface NowPlaying {
  title: string;
  artist: string;
  album: string;
  station: string;
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
interface Lyrics {
  synced: string | null;
  plain: string | null;
  source: string;
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

const fmt = (s: number) => {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
};

// ---- state -------------------------------------------------------------
let currentKey = "";
let everPlayed = false;
let syncedLines: LyricLine[] | null = null;
// remote (network player) mode
let remote: RemoteState | null = null;
let remoteAt = 0; // Date.now() when the last remote state arrived
let remoteMode = false;
let lastLocalPlayingAt = 0;
let lastLocalNp: NowPlaying | null = null;
let activeLineIdx = -1;
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

function renderSyncedLyrics(lines: LyricLine[]) {
  lyricsEl.innerHTML = "";
  lyricsEl.classList.add("synced");
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
  if (!syncedLines || syncedLines.length === 0) return;
  const p = position + syncOffset;
  let idx = -1;
  for (let i = 0; i < syncedLines.length; i++) {
    if (syncedLines[i].t <= p + 0.15) idx = i;
    else break;
  }
  if (idx === activeLineIdx) return;
  activeLineIdx = idx;
  const nodes = lyricsEl.querySelectorAll<HTMLElement>(".line");
  nodes.forEach((n) => {
    const i = Number(n.dataset.idx);
    n.classList.toggle("past", i < idx);
    n.classList.toggle("active", i === idx);
  });
  const active = nodes[idx];
  if (active) active.scrollIntoView({ block: "center", behavior: "smooth" });
}

// ---- now-playing -------------------------------------------------------
async function onNowPlaying(np: NowPlaying) {
  everPlayed = true;
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
  setThumbs(np.thumbUp, np.thumbDown);

  setArt(np.art || np.artFallback, np.artFallback);

  const key = `${np.title}|${np.artist}|${np.album}`;
  if (key !== currentKey) {
    currentKey = key;
    syncedLines = null;
    activeLineIdx = -1;
    syncOffset = parseFloat(localStorage.getItem(`syncoff:${key}`) || "0") || 0;
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
}

async function loadLyrics(np: NowPlaying) {
  const key = `${np.title}|${np.artist}|${np.album}`;
  lyricsStatus.textContent = "Loading lyrics…";
  lyricsEl.innerHTML = "";
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
  if (lyricsEl.innerHTML === "" || currentKey === key) lyricsEl.innerHTML = "";
  try {
    const res = await invoke<Lyrics>("fetch_lyrics", {
      artist: meta.artist,
      track: meta.title,
      album: meta.album || null,
      duration,
    });
    // Ignore if the track changed while we were fetching.
    if (key !== currentKey) return;

    if (res.synced) {
      syncedLines = parseLrc(res.synced);
      renderSyncedLyrics(syncedLines);
      lyricsStatus.textContent = "Synced lyrics";
      activeLineIdx = -1;
      highlightLine(lastPlayhead.position);
    } else if (res.plain) {
      syncedLines = null;
      renderPlainLyrics(res.plain);
      lyricsStatus.textContent = "Lyrics";
    } else {
      syncedLines = null;
      lyricsEl.innerHTML = `<div class="line empty">No lyrics found</div>`;
      lyricsStatus.textContent = "Lyrics";
    }
  } catch (e) {
    lyricsEl.innerHTML = `<div class="line empty">Lyrics unavailable</div>`;
    lyricsStatus.textContent = "Lyrics";
  }
}

// ---- playhead ----------------------------------------------------------
// Playing/paused is derived from whether the position is actually moving —
// Pandora's DOM and <audio> elements both misreport paused state, but a
// advancing playhead cannot lie.
let lastPos = -1;
let lastMoveAt = 0;
function onPlayhead(ph: Playhead) {
  lastPlayhead = ph;
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
getVersion().then((v) => ($("version").textContent = `v${v}`)).catch(() => {});
$("skip").addEventListener("click", () =>
  remoteMode ? invoke("remote_cmd", { cmd: "skip" }).catch(() => {}) : cmd("skip")
);
$("replay").addEventListener("click", () => cmd("replay"));
thumbUpBtn.addEventListener("click", () => cmd("thumbUp"));
thumbDownBtn.addEventListener("click", () => cmd("thumbDown"));
$("open-engine").addEventListener("click", () =>
  invoke("show_engine", { visible: true }).catch(() => {})
);
$("engine-btn").addEventListener("click", () =>
  invoke("toggle_engine").catch(() => {})
);
// Keyboard escape hatch for the same toggle.
window.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "e") {
    invoke("toggle_engine").catch(() => {});
  }
});

// ---- station switching (searchable picker) -------------------------------
// With ids (full collection via Pandora's web API) selection navigates to the
// station directly; without (rail fallback) it clicks the nth rail item.
let stationNames: string[] = [];
let stationIds: string[] | null = null;
let activeStation = "";

function renderStationList(filter = "") {
  const f = filter.trim().toLowerCase();
  stationList.innerHTML = "";
  stationNames.forEach((name, i) => {
    if (f && !name.toLowerCase().includes(f)) return;
    const item = document.createElement("div");
    item.className = "station-item" + (name === activeStation ? " active" : "");
    item.textContent = name;
    item.addEventListener("click", () => {
      const id = stationIds?.[i];
      cmd(id ? `playStation:${id}` : `station:${i}`);
      stationBtn.textContent = name;
      stationPanel.hidden = true;
    });
    stationList.appendChild(item);
  });
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
window.addEventListener("click", (e) => {
  if (!stationPanel.hidden && !(e.target as HTMLElement).closest("#station-wrap")) {
    stationPanel.hidden = true;
  }
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") stationPanel.hidden = true;
});
listen<{ stations: string[]; ids: string[] | null; active: string }>(
  "engine://stations",
  (e) => {
    const { stations, ids, active } = e.payload;
    if (!stations.length) return;
    // Never downgrade from the full collection back to the rail list.
    if (stationIds && !ids) return;
    stationNames = stations;
    stationIds = ids;
    activeStation = active;
    if (active) stationBtn.textContent = active;
    if (!stationPanel.hidden) renderStationList(stationSearch.value);
  }
);

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
  activeLineIdx = -1; // force re-highlight
  highlightLine(lastPlayhead.position);
  flashStatus(
    syncOffset === 0
      ? "Synced lyrics · offset cleared"
      : `Synced lyrics · offset ${syncOffset > 0 ? "+" : ""}${syncOffset.toFixed(2)}s`
  );
}
window.addEventListener("keydown", (e) => {
  if ((e.target as HTMLElement).tagName === "INPUT") return;
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
  activeLineIdx = -1;
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

// ---- update banner ---------------------------------------------------------
const updateBanner = $("update-banner");
const updateText = $("update-text");
const updateBtn = $("update-btn");
listen<string>("app://update-available", (e) => {
  updateText.textContent = `Jarlid v${e.payload} is available`;
  updateBanner.hidden = false;
});
updateBtn.addEventListener("click", () => {
  updateBtn.textContent = "Installing…";
  (updateBtn as HTMLButtonElement).disabled = true;
  invoke("install_update").catch((err) => {
    updateText.textContent = `Update failed: ${err}`;
    updateBtn.textContent = "Retry";
    (updateBtn as HTMLButtonElement).disabled = false;
  });
});

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
  // Once a track has ever played, ignore transient login signals (they fire during
  // Pandora's initial "/" load before redirecting to the player).
  if (everPlayed) return;
  player.hidden = true;
  loginHint.hidden = false;
});
