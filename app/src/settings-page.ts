// The Settings page. Account only for now — signing out had no UI at all before
// this, despite `native_sign_out` existing since the native client landed.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSelect, type Option } from "./select";
import { setTheme, themePref, type ThemePref } from "./theme";
import { recentsOrder, setRecentsOrder, type RecentsOrder } from "./recents";
import { FLAGS_CHANGED, TAGS, describe, type Flag, type TrackFlags } from "./flags";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("settings-page");
const closeBtn = $<HTMLButtonElement>("set-close");
const accountEl = $("set-account");
const signOutBtn = $<HTMLButtonElement>("set-signout");
const policyRadios = () =>
  document.querySelectorAll<HTMLInputElement>('input[name="update-policy"]');
const themeRadios = () => document.querySelectorAll<HTMLInputElement>('input[name="theme"]');
const recentsRadios = () =>
  document.querySelectorAll<HTMLInputElement>('input[name="recents-order"]');
const timeInput = $<HTMLInputElement>("set-check-time");
const volumeInput = $<HTMLInputElement>("set-volume");
const volumeValue = $("set-volume-value");
const outputNow = $("set-output-now");
const blendRadios = () => document.querySelectorAll<HTMLInputElement>('input[name="blend"]');
const blendSeconds = $<HTMLInputElement>("set-blend-seconds");
const blendSecondsValue = $("set-blend-seconds-value");
const blendPull = $<HTMLInputElement>("set-blend-pull");
const blendPullValue = $("set-blend-pull-value");
const blendRestore = $<HTMLInputElement>("set-blend-restore");
const blendLengthItem = $("set-blend-length-item");
const blendPullItem = $("set-blend-pull-item");
const blendRestoreItem = $("set-blend-restore-item");
const cacheEnabled = $<HTMLInputElement>("set-cache-enabled");
const cacheSizeItem = $("set-cache-size-item");
const cacheUsageItem = $("set-cache-usage-item");
const cacheUsage = $("set-cache-usage");
const cacheClearBtn = $<HTMLButtonElement>("set-cache-clear");
const debugLog = $<HTMLInputElement>("set-debug-log");
const debugLogPath = $("set-debug-log-path");
const debugLogShow = $<HTMLButtonElement>("set-debug-log-show");
const autosyncSection = $("set-autosync-section");
const autosyncEnabled = $<HTMLInputElement>("set-autosync-enabled");
const autosyncStatusEl = $("set-autosync-status");
const autosyncProgress = $("set-autosync-progress");
const autosyncProgressFill = $("set-autosync-progress-fill");
const autosyncGpuItem = $("set-autosync-gpu-item");
const autosyncGpu = $<HTMLInputElement>("set-autosync-gpu");
const autosyncActionItem = $("set-autosync-remove-item");
const autosyncActionBtn = $<HTMLButtonElement>("set-autosync-remove");
const flagsStatus = $("set-flags-status");
const flagsList = $("set-flags-list");
const flagsShow = $<HTMLButtonElement>("set-flags-show");

type Policy = "instant" | "afterSong" | "manualInstall" | "notifyOnly";
type CheckSchedule =
  | { kind: "never" }
  | { kind: "every"; minutes: number }
  | { kind: "dailyAt"; time: string };
type BlendMode = "off" | "crossfade" | "beatMatched";
interface Blend {
  mode: BlendMode;
  seconds: number;
  /** A pitch-fader range. Percent, because ±6% is the same musical stretch at any tempo. */
  maxPullPercent: number;
  restoreTempo: boolean;
}
interface AudioCache {
  enabled: boolean;
  /** MiB. */
  maxMb: number;
}
/** Auto-synced lyrics; see `settings::AutoSync`. */
interface AutoSync {
  enabled: boolean;
  gpu: boolean;
}
/** Where the downloadable speech engine stands; see `autosync::Status`. */
type AutoSyncStatus =
  | { state: "unavailable" }
  | { state: "notInstalled" }
  | { state: "downloading"; done: number; total: number }
  | { state: "installed"; bytes: number; version: string }
  | { state: "failed"; message: string };
/** What the song cache holds right now; see `engine::cache::CacheStats`. */
interface CacheStats {
  bytes: number;
  songs: number;
}
interface Settings {
  updatePolicy: Policy;
  checkSchedule: CheckSchedule;
  theme: ThemePref;
  /** Which end of the recently-played strip the newest song sits at. */
  recentsOrder: RecentsOrder;
  blend: Blend;
  /** 0-100. The taper from this to a gain lives in Rust; see `settings::Volume`. */
  volume: number;
  /** `null` means follow the Windows default, and keep following it. */
  outputDevice: string | null;
  audioCache: AudioCache;
  /** Write everything the app logs to a file; see `debug_log.rs`. */
  debugLog: boolean;
  autoSync: AutoSync;
}

/** The value used for "follow the Windows default", which is `null` on the wire. */
const FOLLOW_DEFAULT = "";

/** The offered intervals. `daily` is a wall-clock time and takes the time field. */
const SCHEDULES: Option[] = [
  { value: "never", label: "Never" },
  { value: "30", label: "Every 30 minutes" },
  { value: "240", label: "Every 4 hours" },
  { value: "1440", label: "Every 24 hours" },
  { value: "daily", label: "Once a day, at…" },
];

const scheduleSel = createSelect($("set-check-schedule"), SCHEDULES, {
  label: "How often to check for updates",
  onChange: () => void persist(),
});

// Populated from the backend on open — the list of endpoints is a fact about the
// machine right now, not a constant.
const outputSel = createSelect($("set-output-device"), [{ value: FOLLOW_DEFAULT, label: "Windows default" }], {
  label: "Audio output device",
  onChange: (value) => {
    // Heard before it is saved, like the volume: choosing a device is judged by
    // whether sound comes out of it.
    void invoke("native_set_output", { device: value === FOLLOW_DEFAULT ? null : value }).catch(
      () => {},
    );
    void persist();
    // The engine takes about a second to move the stream; ask again once it has.
    setTimeout(() => void refreshOutputNow(), 1400);
  },
});

/** Sizes offered for the song cache, in MiB. */
const CACHE_SIZES_MB = [256, 512, 1024, 2048, 5120, 10240];

/** For a settings file written before auto-sync existed. Matches `AutoSync::default`. */
const NO_AUTOSYNC: AutoSync = { enabled: false, gpu: true };
let autosyncStatus: AutoSyncStatus = { state: "unavailable" };

/** For a settings file written before the cache existed. Matches `AudioCache::default`. */
const NO_CACHE: AudioCache = { enabled: false, maxMb: 1024 };

/**
 * What one song takes on disk, until the cache holds enough of them to say for itself:
 * four minutes at 128 kbit/s, which is the stream Pandora usually grants.
 */
const NOMINAL_SONG_BYTES = 4 * 60 * (128_000 / 8);

/** Songs averaged before the real figure replaces the nominal one. */
const MEASURED_AFTER = 5;

let cacheStats: CacheStats = { bytes: 0, songs: 0 };

const cacheSizeSel = createSelect($("set-cache-size"), [], {
  label: "Space to use for saved songs",
  onChange: () => void persist(),
});

let current: Settings | null = null;

/// How long `.closing` stays on before the page is hidden. Not shorter than the closing
/// animation in styles.css, or the panel vanishes mid-retreat. Same shape as the Stations
/// panel, for the same reasons — including the timer rather than `animationend`, which is
/// dispatched on a frame that an off-screen window may not get.
const CLOSE_MS = 200;
let closeTimer: ReturnType<typeof setTimeout> | undefined;

export const isOpen = () => !page.hidden;

export function open() {
  clearTimeout(closeTimer);
  page.classList.remove("closing");
  page.hidden = false;
  page.classList.add("opening");
  page.addEventListener("animationend", () => page.classList.remove("opening"), { once: true });
  void refreshAccount();
  void refreshSettings();
  void refreshFlags();
}

/**
 * The endpoints present right now.
 *
 * A stored device that is *not* present still gets an entry. Dropping it would make the
 * list disagree with what is saved, and the next save would then quietly rewrite the
 * choice — so unplugging a DAC would lose the preference rather than suspend it.
 */
async function refreshOutputDevices() {
  let devices: string[] = [];
  try {
    devices = await invoke<string[]>("native_output_devices");
  } catch {
    // Can't enumerate; the stored choice still has to be shown truthfully.
  }
  const chosen = current?.outputDevice ?? null;
  if (chosen && !devices.includes(chosen)) devices = [...devices, `${chosen}`];
  outputSel.setOptions([
    { value: FOLLOW_DEFAULT, label: "Windows default" },
    ...devices.map((d) => ({ value: d, label: d })),
  ]);
  outputSel.value = chosen ?? FOLLOW_DEFAULT;
}

/**
 * What is actually being played to, which is a different question from what was chosen:
 * "Windows default" doesn't say which device that is, and a chosen device that has gone
 * away falls back rather than going silent.
 */
async function refreshOutputNow() {
  let inUse: string | null = null;
  try {
    inUse = await invoke<string | null>("native_output_device");
  } catch {
    // Not signed in, so no engine and nothing open.
  }
  const chosen = current?.outputDevice ?? null;
  if (!inUse) {
    outputNow.textContent = "Nothing is playing, so no device is open right now.";
  } else if (!chosen) {
    outputNow.textContent = `Playing on ${inUse} — the current Windows default.`;
  } else if (chosen === inUse) {
    outputNow.textContent = `Playing on ${inUse}.`;
  } else {
    outputNow.textContent = `${chosen} isn't available — playing on ${inUse} until it is back.`;
  }
}

/**
 * Apply the stored look at startup, without showing the page.
 *
 * Both of these are painted from a `localStorage` cache before Rust can answer, so
 * this is the moment the file gets to correct the guess — which it has to do when
 * the settings were changed by another install or hand-edited between runs.
 */
export async function applyStoredAppearance() {
  try {
    const s = await invoke<Settings>("get_settings");
    if (s.theme && s.theme !== themePref()) setTheme(s.theme);
    if (s.recentsOrder && s.recentsOrder !== recentsOrder()) setRecentsOrder(s.recentsOrder);
  } catch {
    // Unreadable settings means the cached preferences stand.
  }
}

// Read from the backend rather than keeping a local copy: the update loop reads the same
// file, and two sources of truth is how they drift apart.
async function refreshSettings() {
  try {
    current = await invoke<Settings>("get_settings");
    render(current);
    setEnabled(true);
    // Both depend on `current`, so they follow the render rather than racing it.
    await refreshOutputDevices();
    await refreshOutputNow();
    await refreshCacheStats();
    await refreshDebugLogPath();
    await refreshAutosyncStatus();
  } catch {
    // Can't read them — don't show controls whose state would be a guess.
    setEnabled(false);
  }
}

function setEnabled(on: boolean) {
  policyRadios().forEach((r) => (r.disabled = !on));
  themeRadios().forEach((r) => (r.disabled = !on));
  recentsRadios().forEach((r) => (r.disabled = !on));
  scheduleSel.setDisabled(!on);
  outputSel.setDisabled(!on);
  timeInput.disabled = !on;
  volumeInput.disabled = !on;
  blendRadios().forEach((r) => (r.disabled = !on));
  blendSeconds.disabled = !on;
  blendPull.disabled = !on;
  blendRestore.disabled = !on;
  cacheEnabled.disabled = !on;
  cacheSizeSel.setDisabled(!on);
  debugLog.disabled = !on;
  autosyncEnabled.disabled = !on;
  autosyncGpu.disabled = !on;
}

/** Defaults for a settings file written before blending existed. */
const NO_BLEND: Blend = {
  mode: "off",
  seconds: 8,
  maxPullPercent: 6,
  restoreTempo: true,
};

function readBlend(): Blend {
  const picked = [...blendRadios()].find((r) => r.checked);
  return {
    mode: (picked?.value as BlendMode) ?? "off",
    seconds: Number(blendSeconds.value),
    maxPullPercent: Number(blendPull.value),
    restoreTempo: blendRestore.checked,
  };
}

/**
 * Show only the controls that currently do something.
 *
 * Overlap applies to both blending modes; the tempo pull and the return-to-native glide
 * only mean anything when we are matching beats. Leaving them visible but inert invites
 * the reasonable conclusion that they were set and ignored.
 */
function reflectBlend() {
  const blend = readBlend();
  blendLengthItem.hidden = blend.mode === "off";
  blendPullItem.hidden = blend.mode !== "beatMatched";
  blendRestoreItem.hidden = blend.mode !== "beatMatched";

  blendSeconds.style.setProperty("--fill", `${((blend.seconds - 2) / 18) * 100}%`);
  blendSecondsValue.textContent = `${blend.seconds}s`;

  blendPull.style.setProperty("--fill", `${(blend.maxPullPercent / 16) * 100}%`);
  // Say what the percentage means in the units the rest of the app shows tempo in.
  // "±6%" is the honest setting; "±7.7 BPM at 128" is the one you can picture.
  const atOneTwentyEight = (128 * blend.maxPullPercent) / 100;
  blendPullValue.textContent =
    blend.maxPullPercent === 0
      ? "none"
      : `±${blend.maxPullPercent}% · ±${atOneTwentyEight.toFixed(1)} BPM at 128`;
}

function formatMb(mb: number): string {
  if (mb < 1024) return `${mb} MB`;
  const gb = mb / 1024;
  return `${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`;
}

function formatBytes(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb < 1) return "under 1 MB";
  if (mb < 1024) return `${Math.round(mb)} MB`;
  return `${(mb / 1024).toFixed(1)} GB`;
}

/**
 * Roughly how many songs `mb` holds, from the songs actually saved once there are enough to
 * average. Rounded so it reads as the estimate it is: "about 260", not "about 263".
 */
function songsIn(mb: number): string {
  const perSong =
    cacheStats.songs >= MEASURED_AFTER ? cacheStats.bytes / cacheStats.songs : NOMINAL_SONG_BYTES;
  const n = (mb * 1024 * 1024) / perSong;
  const step = n < 100 ? 5 : n < 1000 ? 10 : 100;
  return (Math.round(n / step) * step).toLocaleString();
}

/**
 * The size list for a stored cap. A cap this build doesn't offer (hand-edited, or from a
 * newer version) gets its own entry rather than being snapped to a neighbour, for the same
 * reason as `scheduleOptions`.
 */
function cacheSizeOptions(stored: number): Option[] {
  const sizes = CACHE_SIZES_MB.includes(stored)
    ? CACHE_SIZES_MB
    : [...CACHE_SIZES_MB, stored].sort((a, b) => a - b);
  return sizes.map((mb) => ({
    value: String(mb),
    label: `${formatMb(mb)} · about ${songsIn(mb)} songs`,
  }));
}

function readCache(): AudioCache {
  return {
    enabled: cacheEnabled.checked,
    maxMb: Number(cacheSizeSel.value) || NO_CACHE.maxMb,
  };
}

/**
 * Show the size only while the cache is on, and the usage whenever there is anything to
 * report. Songs saved before the cache was turned off are still on disk, and saying so
 * (with the button to delete them) is the honest answer to turning it off.
 */
function reflectCache() {
  const cache = current?.audioCache ?? NO_CACHE;
  const enabled = cacheEnabled.checked;
  cacheSizeItem.hidden = !enabled;
  cacheSizeSel.setOptions(cacheSizeOptions(cache.maxMb));
  cacheSizeSel.value = String(cache.maxMb);

  const { bytes, songs } = cacheStats;
  cacheUsageItem.hidden = !enabled && songs === 0;
  cacheClearBtn.disabled = songs === 0;
  if (songs === 0) {
    // The song playing when it is switched on was streamed, so saving starts with the next.
    cacheUsage.textContent = "Nothing saved yet. Saving starts with the next song.";
  } else {
    const what = `${formatBytes(bytes)} for ${songs.toLocaleString()} song${songs === 1 ? "" : "s"}`;
    cacheUsage.textContent = enabled
      ? `${what}, of ${formatMb(cache.maxMb)}.`
      : `${what}, still on disk from before saving was turned off.`;
  }
}

async function refreshCacheStats() {
  try {
    cacheStats = await invoke<CacheStats>("audio_cache_stats");
  } catch {
    cacheStats = { bytes: 0, songs: 0 };
  }
  reflectCache();
}

function render(s: Settings) {
  policyRadios().forEach((r) => (r.checked = r.value === s.updatePolicy));
  themeRadios().forEach((r) => (r.checked = r.value === (s.theme ?? "system")));
  recentsRadios().forEach((r) => (r.checked = r.value === (s.recentsOrder ?? "newestRight")));
  volumeInput.value = String(s.volume ?? 100);
  outputSel.value = s.outputDevice ?? FOLLOW_DEFAULT;
  reflectVolume();
  const blend = s.blend ?? NO_BLEND;
  blendRadios().forEach((r) => (r.checked = r.value === blend.mode));
  blendSeconds.value = String(blend.seconds);
  blendPull.value = String(blend.maxPullPercent);
  blendRestore.checked = blend.restoreTempo;
  reflectBlend();
  cacheEnabled.checked = (s.audioCache ?? NO_CACHE).enabled;
  reflectCache();
  debugLog.checked = s.debugLog ?? false;
  const autoSync = s.autoSync ?? NO_AUTOSYNC;
  autosyncEnabled.checked = autoSync.enabled;
  autosyncGpu.checked = autoSync.gpu;
  reflectAutosync();
  const sched = s.checkSchedule;
  scheduleSel.setOptions(scheduleOptions(sched));
  scheduleSel.value =
    sched.kind === "never" ? "never" : sched.kind === "dailyAt" ? "daily" : String(sched.minutes);
  timeInput.hidden = sched.kind !== "dailyAt";
  if (sched.kind === "dailyAt") timeInput.value = sched.time;
}

/**
 * The list to show for a given stored schedule.
 *
 * An interval this build doesn't offer — written by a newer version, or edited by
 * hand — gets an entry of its own rather than being snapped to the nearest one we
 * do know. Showing "Every 3 hours" and leaving it alone beats quietly rewriting a
 * setting the user never touched.
 */
function scheduleOptions(sched: CheckSchedule): Option[] {
  if (sched.kind !== "every" || SCHEDULES.some((o) => o.value === String(sched.minutes))) {
    return SCHEDULES;
  }
  const extra = { value: String(sched.minutes), label: everyLabel(sched.minutes) };
  return [...SCHEDULES.slice(0, -1), extra, SCHEDULES[SCHEDULES.length - 1]];
}

function everyLabel(minutes: number): string {
  const plural = (n: number, unit: string) => `Every ${n} ${unit}${n === 1 ? "" : "s"}`;
  if (minutes % 1440 === 0) return plural(minutes / 1440, "day");
  if (minutes % 60 === 0) return plural(minutes / 60, "hour");
  return plural(minutes, "minute");
}

function readSchedule(): CheckSchedule {
  const v = scheduleSel.value;
  if (v === "never") return { kind: "never" };
  if (v === "daily") return { kind: "dailyAt", time: timeInput.value || "03:00" };
  return { kind: "every", minutes: Number(v) };
}

function readTheme(): ThemePref {
  const picked = [...themeRadios()].find((r) => r.checked);
  return (picked?.value as ThemePref) ?? "system";
}

function readRecentsOrder(): RecentsOrder {
  const picked = [...recentsRadios()].find((r) => r.checked);
  return (picked?.value as RecentsOrder) ?? "newestRight";
}

function readPolicy(): Policy {
  const picked = [...policyRadios()].find((r) => r.checked);
  return (picked?.value as Policy) ?? "afterSong";
}

function readVolume(): number {
  return Number(volumeInput.value);
}

function readOutput(): string | null {
  const v = outputSel.value;
  return v === FOLLOW_DEFAULT ? null : v;
}

/** Paint the filled part of the track and the read-out from the slider's position. */
function reflectVolume() {
  const v = readVolume();
  volumeInput.style.setProperty("--fill", `${v}%`);
  volumeValue.textContent = `${v}%`;
}

/** Apply a level without saving it. */
function applyVolume(percent: number) {
  // Not signed in means there is no engine to talk to yet; the level is still saved,
  // and `attach()` applies it as soon as one exists.
  void invoke("native_volume", { percent }).catch(() => {});
}

async function persist() {
  const next: Settings = {
    updatePolicy: readPolicy(),
    checkSchedule: readSchedule(),
    theme: readTheme(),
    recentsOrder: readRecentsOrder(),
    volume: readVolume(),
    blend: readBlend(),
    outputDevice: readOutput(),
    audioCache: readCache(),
    debugLog: debugLog.checked,
    autoSync: { enabled: autosyncEnabled.checked, gpu: autosyncGpu.checked },
  };
  timeInput.hidden = next.checkSchedule.kind !== "dailyAt";
  // Disabling a control takes focus with it, so a keyboard user would be dropped
  // back to the top of the page by every change they made.
  const focused = document.activeElement as HTMLElement | null;
  setEnabled(false);
  try {
    current = await invoke<Settings>("set_settings", { settings: next });
    render(current);
    // A smaller cap evicts at once, so the usage line may have changed.
    void refreshCacheStats();
  } catch {
    // Roll back to what is actually stored rather than leaving the UI asserting
    // something that was never saved — including the theme and the volume, both of
    // which have already been applied optimistically.
    if (current) {
      render(current);
      setTheme(current.theme ?? "system");
      setRecentsOrder(current.recentsOrder ?? "newestRight");
      applyVolume(current.volume ?? 100);
      // The device was moved optimistically too, so put playback back where the
      // stored settings say it belongs.
      void invoke("native_set_output", { device: current.outputDevice ?? null }).catch(() => {});
    }
  } finally {
    setEnabled(true);
    if (focused?.isConnected) focused.focus({ preventScroll: true });
  }
}

$("set-policy").addEventListener("change", () => void persist());
timeInput.addEventListener("change", () => void persist());

// Volume is judged by ear, so it follows the handle rather than the write: every
// `input` is heard immediately, and only the value it is let go on reaches the disk.
// `change` on a range fires on release, which is exactly the moment worth saving.
volumeInput.addEventListener("input", () => {
  reflectVolume();
  applyVolume(readVolume());
});
volumeInput.addEventListener("change", () => void persist());

// The filled part of the track is painted from `--fill`, which only script can set, so
// until this runs the markup's own `value` is drawn as an empty track with the thumb at
// the far end. `render()` would fix it — but not on the path where the settings can't be
// read at all, which is exactly when a control lying about its value is worst.
reflectVolume();

// Changing the mode reveals or hides the controls under it, so the layout has to move on
// the click rather than waiting for the write to come back.
$("set-blend-mode").addEventListener("change", () => {
  reflectBlend();
  void persist();
});
blendRestore.addEventListener("change", () => void persist());

// The two sliders follow the handle for the read-out and only save on release, like the
// volume fader — but unlike volume there is nothing to apply optimistically, since a blend
// setting is not heard until the next time a song ends.
for (const slider of [blendSeconds, blendPull]) {
  slider.addEventListener("input", reflectBlend);
  slider.addEventListener("change", () => void persist());
}

// Same reason as `reflectVolume()` below: `--fill` is script-only, so without this the
// tracks paint empty until the first render — and on the path where settings cannot be
// read at all, that is exactly when a control lying about its value is worst.
reflectBlend();

// The theme is the one setting you judge by looking at it, so it changes as you
// click rather than after the write comes back.
$("set-theme").addEventListener("change", () => {
  setTheme(readTheme());
  void persist();
});

// Same again: the strip is behind this page, and the only way to judge which way
// round you want it is to see it turn.
$("set-recents-order").addEventListener("change", () => {
  setRecentsOrder(readRecentsOrder());
  void persist();
});

// The size item appears with the switch, so the layout moves on the click.
cacheEnabled.addEventListener("change", () => {
  cacheSizeItem.hidden = !cacheEnabled.checked;
  void persist();
});

debugLog.addEventListener("change", () => void persist());

// Turning it on is the consent to download: the backend starts as soon as it is saved.
autosyncEnabled.addEventListener("change", () => {
  reflectAutosync();
  void persist();
});
autosyncGpu.addEventListener("change", () => void persist());

/**
 * Show the section only where this build can offer it, then say where the engine stands.
 * The one button changes job with the state — stop a download, retry a failed one, or
 * remove what is installed — so there is never a control that can't do anything.
 */
function reflectAutosync() {
  const st = autosyncStatus;
  autosyncSection.hidden = st.state === "unavailable";
  const on = autosyncEnabled.checked;
  autosyncGpuItem.hidden = !on;
  autosyncProgress.hidden = st.state !== "downloading";

  let text = "";
  let action: "" | "cancel" | "retry" | "remove" | "download" = "";
  switch (st.state) {
    case "notInstalled":
      text = on ? "Not downloaded yet." : "";
      action = on ? "download" : "";
      break;
    case "downloading": {
      const pct = st.total ? Math.floor((st.done / st.total) * 100) : 0;
      autosyncProgressFill.style.width = `${pct}%`;
      text = `Downloading the speech engine… ${formatBytes(st.done)} of ${formatBytes(st.total)} (${pct}%)`;
      action = "cancel";
      break;
    }
    case "installed":
      text = `Speech engine installed, ${formatBytes(st.bytes)}.${on ? "" : " Not in use while this is off."}`;
      action = "remove";
      break;
    case "failed":
      text = `The download didn't finish: ${st.message}`;
      action = "retry";
      break;
  }
  autosyncStatusEl.textContent = text;
  autosyncActionItem.hidden = action === "";
  autosyncActionBtn.dataset.action = action;
  autosyncActionBtn.dataset.armed = "";
  autosyncActionBtn.textContent = { "": "", cancel: "Stop download", retry: "Try again", remove: "Remove", download: "Download" }[action];
}

async function refreshAutosyncStatus() {
  try {
    autosyncStatus = await invoke<AutoSyncStatus>("autosync_status");
  } catch {
    autosyncStatus = { state: "unavailable" };
  }
  reflectAutosync();
}

void listen<AutoSyncStatus>("autosync://status", (e) => {
  autosyncStatus = e.payload;
  reflectAutosync();
});

autosyncActionBtn.addEventListener("click", async () => {
  const action = autosyncActionBtn.dataset.action;
  if (action === "retry" || action === "download") {
    autosyncStatus = await invoke<AutoSyncStatus>("autosync_install").catch(() => autosyncStatus);
    reflectAutosync();
    return;
  }
  // Removing throws away a 700 MB download, so it takes two clicks, like signing out.
  if (action === "remove" && autosyncActionBtn.dataset.armed !== "1") {
    autosyncActionBtn.dataset.armed = "1";
    autosyncActionBtn.textContent = "Really remove it?";
    setTimeout(() => {
      if (autosyncActionBtn.dataset.armed === "1") reflectAutosync();
    }, 4000);
    return;
  }
  // Stopping or removing both mean "not now": turn the setting off too, or the next launch
  // would start downloading again.
  autosyncEnabled.checked = false;
  try {
    autosyncStatus = await invoke<AutoSyncStatus>("autosync_remove");
  } catch {
    await refreshAutosyncStatus();
  }
  reflectAutosync();
  void persist();
});

/** Say where the log is, in full: the folder is three levels deep in AppData. */
async function refreshDebugLogPath() {
  try {
    debugLogPath.textContent = (await invoke<string | null>("debug_log_path")) ?? "";
  } catch {
    debugLogPath.textContent = "";
  }
}

debugLogShow.addEventListener("click", () => {
  invoke("debug_log_reveal").catch((err) => {
    debugLogPath.textContent = `Could not open the log folder: ${err}`;
  });
});

cacheClearBtn.addEventListener("click", async () => {
  // Two-step, like signing out: a stray click would cost every saved song a download.
  if (cacheClearBtn.dataset.armed !== "1") {
    cacheClearBtn.dataset.armed = "1";
    cacheClearBtn.textContent = "Really delete them?";
    setTimeout(() => {
      cacheClearBtn.dataset.armed = "";
      cacheClearBtn.textContent = "Delete saved songs";
    }, 4000);
    return;
  }
  cacheClearBtn.dataset.armed = "";
  cacheClearBtn.textContent = "Delete saved songs";
  try {
    cacheStats = await invoke<CacheStats>("audio_cache_clear");
  } catch {
    await refreshCacheStats();
    return;
  }
  reflectCache();
});

// ---- flagged lyrics -------------------------------------------------------------
// Pandora won't play a song on request, so this list is the only way back to a flag whose
// song hasn't come round again.

async function refreshFlags() {
  let all: TrackFlags[];
  try {
    all = await invoke<TrackFlags[]>("all_lyric_flags");
  } catch (e) {
    flagsStatus.textContent = `Couldn't read the flags: ${e}`;
    flagsList.replaceChildren();
    return;
  }
  flagsStatus.textContent =
    all.length === 0
      ? "Nothing flagged."
      : `${all.length} ${all.length === 1 ? "song" : "songs"} flagged, most recent first.`;
  flagsList.replaceChildren(...all.map(flaggedSong));
}

/// Where the flagged lyrics came from, in a few words.
function origin(f: Flag): string {
  const s = f.shown;
  if (s.overridden) return "your edit";
  if (s.auto) return "auto-synced";
  const from = s.id !== null ? `LRCLIB #${s.id}` : s.source;
  return s.pinned ? `${from}, your pick` : from;
}

/// One flag as a line of prose: where, what kind, the words it starts on, and the note.
function flagLine(f: Flag): string {
  const bits = [describe(f)];
  const tags = f.tags.map((t) => TAGS.find((x) => x.id === t)?.label ?? t);
  if (tags.length) bits.push(tags.join(", "));
  const words = f.span?.text.find((t) => t.trim());
  if (words) bits.push(`“${words.length > 60 ? `${words.slice(0, 59)}…` : words}”`);
  bits.push(origin(f));
  return bits.join(" · ") + (f.note ? `. ${f.note}` : "");
}

function flaggedSong(t: TrackFlags): HTMLElement {
  const item = document.createElement("div");
  item.className = "set-item";

  const title = document.createElement("b");
  title.className = "set-item-title";
  title.textContent = `${t.track} · ${t.artist}`;

  const text = document.createElement("div");
  text.className = "set-item-text";
  const flags = [...t.flags].sort((a, b) => a.created - b.created);
  for (const f of flags) {
    const p = document.createElement("p");
    p.textContent = flagLine(f);
    text.append(p);
  }
  const when = document.createElement("p");
  when.className = "set-item-status";
  const newest = Math.max(...flags.map((f) => f.created));
  when.textContent = `${t.album ? `${t.album} · ` : ""}Flagged ${new Date(newest).toLocaleDateString()}`;
  text.append(when);

  const ctl = document.createElement("div");
  ctl.className = "set-item-ctl";
  const remove = document.createElement("button");
  remove.className = "btn";
  remove.textContent = "Remove";
  remove.addEventListener("click", async () => {
    // Two-step, like deleting saved songs: what a flag kept (the lyrics, what auto-sync heard)
    // can't be made again once the song has moved on.
    if (remove.dataset.armed !== "1") {
      remove.dataset.armed = "1";
      remove.textContent = "Really remove?";
      setTimeout(() => {
        remove.dataset.armed = "";
        remove.textContent = "Remove";
      }, 4000);
      return;
    }
    remove.disabled = true;
    try {
      await invoke("save_lyric_flags", {
        artist: t.artist,
        track: t.track,
        album: t.album,
        duration: t.duration,
        flags: [],
      });
      window.dispatchEvent(new Event(FLAGS_CHANGED));
    } catch (e) {
      flagsStatus.textContent = `Couldn't remove them: ${e}`;
    }
    await refreshFlags();
  });
  ctl.append(remove);

  item.append(title, text, ctl);
  return item;
}

flagsShow.addEventListener("click", () => {
  invoke("lyric_flags_reveal").catch((err) => {
    flagsStatus.textContent = `Could not open the folder: ${err}`;
  });
});

function close() {
  page.classList.remove("opening");
  page.classList.add("closing");
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    page.classList.remove("closing");
    page.hidden = true;
  }, CLOSE_MS);
}

async function refreshAccount() {
  try {
    const who = await invoke<string | null>("native_account");
    accountEl.textContent = who ? `Signed in as ${who}` : "Not signed in";
    signOutBtn.hidden = !who;
  } catch {
    accountEl.textContent = "Not signed in";
    signOutBtn.hidden = true;
  }
}

closeBtn.addEventListener("click", close);

signOutBtn.addEventListener("click", async () => {
  // Two-step rather than a confirm dialog: signing out clears the saved password,
  // so it costs a re-login, and a stray click shouldn't do that silently.
  if (signOutBtn.dataset.armed !== "1") {
    signOutBtn.dataset.armed = "1";
    signOutBtn.textContent = "Really sign out?";
    setTimeout(() => {
      signOutBtn.dataset.armed = "";
      signOutBtn.textContent = "Sign out";
    }, 4000);
    return;
  }
  signOutBtn.dataset.armed = "";
  signOutBtn.textContent = "Sign out";
  try {
    await invoke("native_sign_out");
    accountEl.textContent = "Not signed in";
    signOutBtn.hidden = true;
    // Signing out puts the sign-in card back up, but it sits behind this page — so get out of
    // its way. It used to say "restart Jarlid to sign in again", which was only true because
    // the card never appeared on its own.
    close();
  } catch (e) {
    accountEl.textContent = `Could not sign out: ${e}`;
  }
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !page.hidden) close();
});
