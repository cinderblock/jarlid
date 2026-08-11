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

type Policy = "instant" | "afterSong" | "manualInstall" | "notifyOnly";
type CheckSchedule =
  | { kind: "never" }
  | { kind: "every"; minutes: number }
  | { kind: "dailyAt"; time: string };
interface Settings {
  updatePolicy: Policy;
  checkSchedule: CheckSchedule;
  theme: ThemePref;
}

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

let current: Settings | null = null;

export function open() {
  page.hidden = false;
  void refreshAccount();
  void refreshSettings();
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
  } catch {
    // Can't read them — don't show controls whose state would be a guess.
    setEnabled(false);
  }
}

function setEnabled(on: boolean) {
  policyRadios().forEach((r) => (r.disabled = !on));
  themeRadios().forEach((r) => (r.disabled = !on));
  scheduleSel.setDisabled(!on);
  timeInput.disabled = !on;
}

function render(s: Settings) {
  policyRadios().forEach((r) => (r.checked = r.value === s.updatePolicy));
  themeRadios().forEach((r) => (r.checked = r.value === (s.theme ?? "system")));
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

async function persist() {
  const next: Settings = {
    updatePolicy: readPolicy(),
    checkSchedule: readSchedule(),
    theme: readTheme(),
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
    // something that was never saved — including the theme, which has already
    // been applied optimistically.
    if (current) {
      render(current);
      setTheme(current.theme ?? "system");
    }
  } finally {
    setEnabled(true);
    if (focused?.isConnected) focused.focus({ preventScroll: true });
  }
}

$("set-policy").addEventListener("change", () => void persist());
timeInput.addEventListener("change", () => void persist());

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
    accountEl.textContent = "Signed out. Restart Jarlid to sign in again.";
    signOutBtn.hidden = true;
  } catch (e) {
    accountEl.textContent = `Could not sign out: ${e}`;
  }
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !page.hidden) close();
});
