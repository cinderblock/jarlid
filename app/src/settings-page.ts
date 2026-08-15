// The Settings page. Account only for now — signing out had no UI at all before
// this, despite `native_sign_out` existing since the native client landed.

import { invoke } from "@tauri-apps/api/core";
import { createSelect, type Option } from "./select";
import { setTheme, themePref, type ThemePref } from "./theme";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("settings-page");
const closeBtn = $<HTMLButtonElement>("set-close");
const accountEl = $("set-account");
const signOutBtn = $<HTMLButtonElement>("set-signout");
const policyRadios = () =>
  document.querySelectorAll<HTMLInputElement>('input[name="update-policy"]');
const themeRadios = () => document.querySelectorAll<HTMLInputElement>('input[name="theme"]');
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
interface Settings {
  updatePolicy: Policy;
  checkSchedule: CheckSchedule;
  theme: ThemePref;
  blend: Blend;
  /** 0-100. The taper from this to a gain lives in Rust; see `settings::Volume`. */
  volume: number;
  /** `null` means follow the Windows default, and keep following it. */
  outputDevice: string | null;
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

let current: Settings | null = null;

export function open() {
  page.hidden = false;
  void refreshAccount();
  void refreshSettings();
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

/** Apply the stored theme at startup, without showing the page. */
export async function applyStoredTheme() {
  try {
    const s = await invoke<Settings>("get_settings");
    // The webview's cache already painted something; only correct it if the file
    // disagrees, which happens when the settings were changed by another install
    // or hand-edited between runs.
    if (s.theme && s.theme !== themePref()) setTheme(s.theme);
  } catch {
    // Unreadable settings means the cached preference stands.
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
  } catch {
    // Can't read them — don't show controls whose state would be a guess.
    setEnabled(false);
  }
}

function setEnabled(on: boolean) {
  policyRadios().forEach((r) => (r.disabled = !on));
  themeRadios().forEach((r) => (r.disabled = !on));
  scheduleSel.setDisabled(!on);
  outputSel.setDisabled(!on);
  timeInput.disabled = !on;
  volumeInput.disabled = !on;
  blendRadios().forEach((r) => (r.disabled = !on));
  blendSeconds.disabled = !on;
  blendPull.disabled = !on;
  blendRestore.disabled = !on;
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

function render(s: Settings) {
  policyRadios().forEach((r) => (r.checked = r.value === s.updatePolicy));
  themeRadios().forEach((r) => (r.checked = r.value === (s.theme ?? "system")));
  volumeInput.value = String(s.volume ?? 100);
  outputSel.value = s.outputDevice ?? FOLLOW_DEFAULT;
  reflectVolume();
  const blend = s.blend ?? NO_BLEND;
  blendRadios().forEach((r) => (r.checked = r.value === blend.mode));
  blendSeconds.value = String(blend.seconds);
  blendPull.value = String(blend.maxPullPercent);
  blendRestore.checked = blend.restoreTempo;
  reflectBlend();
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
    volume: readVolume(),
    blend: readBlend(),
    outputDevice: readOutput(),
  };
  timeInput.hidden = next.checkSchedule.kind !== "dailyAt";
  // Disabling a control takes focus with it, so a keyboard user would be dropped
  // back to the top of the page by every change they made.
  const focused = document.activeElement as HTMLElement | null;
  setEnabled(false);
  try {
    current = await invoke<Settings>("set_settings", { settings: next });
    render(current);
  } catch {
    // Roll back to what is actually stored rather than leaving the UI asserting
    // something that was never saved — including the theme and the volume, both of
    // which have already been applied optimistically.
    if (current) {
      render(current);
      setTheme(current.theme ?? "system");
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

function close() {
  page.hidden = true;
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
