// The Settings page. Account only for now — signing out had no UI at all before
// this, despite `native_sign_out` existing since the native client landed.

import { invoke } from "@tauri-apps/api/core";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("settings-page");
const closeBtn = $<HTMLButtonElement>("set-close");
const accountEl = $("set-account");
const signOutBtn = $<HTMLButtonElement>("set-signout");
const policyRadios = () =>
  document.querySelectorAll<HTMLInputElement>('input[name="update-policy"]');
const scheduleSel = $<HTMLSelectElement>("set-check-schedule");
const timeInput = $<HTMLInputElement>("set-check-time");

type Policy = "instant" | "afterSong" | "manualInstall" | "notifyOnly";
type CheckSchedule =
  | { kind: "never" }
  | { kind: "every"; minutes: number }
  | { kind: "dailyAt"; time: string };
interface Settings {
  updatePolicy: Policy;
  checkSchedule: CheckSchedule;
}

let current: Settings | null = null;

export function open() {
  page.hidden = false;
  void refreshAccount();
  void refreshSettings();
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
  scheduleSel.disabled = !on;
  timeInput.disabled = !on;
}

function render(s: Settings) {
  policyRadios().forEach((r) => (r.checked = r.value === s.updatePolicy));
  const sched = s.checkSchedule;
  scheduleSel.value =
    sched.kind === "never" ? "never" : sched.kind === "dailyAt" ? "daily" : String(sched.minutes);
  // An interval this build doesn't offer (set by a newer version, or hand-edited) would
  // otherwise leave the select blank and silently rewrite itself on the next save.
  if (!scheduleSel.value || scheduleSel.selectedIndex < 0) {
    scheduleSel.value = "30";
  }
  timeInput.hidden = sched.kind !== "dailyAt";
  if (sched.kind === "dailyAt") timeInput.value = sched.time;
}

function readSchedule(): CheckSchedule {
  const v = scheduleSel.value;
  if (v === "never") return { kind: "never" };
  if (v === "daily") return { kind: "dailyAt", time: timeInput.value || "03:00" };
  return { kind: "every", minutes: Number(v) };
}

function readPolicy(): Policy {
  const picked = [...policyRadios()].find((r) => r.checked);
  return (picked?.value as Policy) ?? "afterSong";
}

async function persist() {
  const next: Settings = { updatePolicy: readPolicy(), checkSchedule: readSchedule() };
  timeInput.hidden = next.checkSchedule.kind !== "dailyAt";
  setEnabled(false);
  try {
    current = await invoke<Settings>("set_settings", { settings: next });
    render(current);
  } catch {
    // Roll back to what is actually stored rather than leaving the UI asserting
    // something that was never saved.
    if (current) render(current);
  } finally {
    setEnabled(true);
  }
}

$("set-policy").addEventListener("change", () => void persist());
scheduleSel.addEventListener("change", () => void persist());
timeInput.addEventListener("change", () => void persist());

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
