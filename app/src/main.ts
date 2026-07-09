import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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

function highlightLine(position: number) {
  if (!syncedLines || syncedLines.length === 0) return;
  let idx = -1;
  for (let i = 0; i < syncedLines.length; i++) {
    if (syncedLines[i].t <= position + 0.15) idx = i;
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

  const art = np.art || np.artFallback;
  if (art) {
    artEl.src = art;
    artEl.onerror = () => {
      if (np.artFallback && artEl.src !== np.artFallback) artEl.src = np.artFallback;
    };
    bg.style.backgroundImage = `url("${art}")`;
  }

  const key = `${np.title}|${np.artist}|${np.album}`;
  if (key !== currentKey) {
    currentKey = key;
    syncedLines = null;
    activeLineIdx = -1;
    pushHistory(np);
    await loadLyrics(np);
  }
}

// ---- recently-played gallery -------------------------------------------
interface HistItem {
  art: string;
  title: string;
  artist: string;
}
let history: HistItem[] = [];
try {
  history = JSON.parse(localStorage.getItem("history") || "[]");
} catch {
  history = [];
}
function renderHistory() {
  histEl.innerHTML = "";
  for (const h of history) {
    const img = new Image();
    img.src = h.art;
    img.title = `${h.title} — ${h.artist}`;
    img.className = "hist-item";
    img.loading = "lazy";
    histEl.appendChild(img);
  }
}
function pushHistory(np: NowPlaying) {
  const art = np.artFallback || np.art;
  if (!art || !np.title) return;
  if (history[0] && history[0].title === np.title && history[0].artist === np.artist) return;
  history.unshift({ art, title: np.title, artist: np.artist });
  history = history.slice(0, 40);
  localStorage.setItem("history", JSON.stringify(history));
  renderHistory();
}
renderHistory();
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

function setThumbs(up: boolean, down: boolean) {
  thumbUpBtn.classList.toggle("active", !!up);
  thumbDownBtn.classList.toggle("active", !!down);
}

async function loadLyrics(np: NowPlaying) {
  lyricsStatus.textContent = "Loading lyrics…";
  lyricsEl.innerHTML = "";
  try {
    const res = await invoke<Lyrics>("fetch_lyrics", {
      artist: np.artist,
      track: np.title,
      album: np.album || null,
      duration: lastPlayhead.duration || null,
    });
    // Ignore if the track changed while we were fetching.
    if (`${np.title}|${np.artist}|${np.album}` !== currentKey) return;

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
function onPlayhead(ph: Playhead) {
  lastPlayhead = ph;
  const pct = ph.duration > 0 ? (ph.position / ph.duration) * 100 : 0;
  barEl.style.width = `${Math.min(100, pct)}%`;
  tCur.textContent = fmt(ph.position);
  tDur.textContent = fmt(ph.duration);
  playIcon.hidden = !ph.paused;
  pauseIcon.hidden = ph.paused;
  highlightLine(ph.position);
}

// ---- controls ----------------------------------------------------------
const cmd = (c: string) => invoke("player_cmd", { cmd: c }).catch(() => {});
$("play").addEventListener("click", () => {
  // optimistic flip for instant feedback; playhead events correct it if wrong
  const wasPaused = !playIcon.hidden;
  playIcon.hidden = wasPaused;
  pauseIcon.hidden = !wasPaused;
  cmd("toggle");
});
$("skip").addEventListener("click", () => cmd("skip"));
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
let stationNames: string[] = [];
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
      cmd(`station:${i}`);
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
listen<{ stations: string[]; active: string }>("engine://stations", (e) => {
  const { stations, active } = e.payload;
  if (!stations.length) return;
  stationNames = stations;
  activeStation = active;
  if (active) stationBtn.textContent = active;
  if (!stationPanel.hidden) renderStationList(stationSearch.value);
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
