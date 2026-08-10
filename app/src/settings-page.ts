// The Settings page. Account only for now — signing out had no UI at all before
// this, despite `native_sign_out` existing since the native client landed.

import { invoke } from "@tauri-apps/api/core";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const page = $("settings-page");
const closeBtn = $<HTMLButtonElement>("set-close");
const accountEl = $("set-account");
const signOutBtn = $<HTMLButtonElement>("set-signout");
const autoUpdate = $<HTMLInputElement>("set-auto-update");

interface Settings {
  autoUpdate: boolean;
}

export function open() {
  page.hidden = false;
  void refreshAccount();
  void refreshSettings();
}

// Read from the backend rather than keeping a local copy: the update loop reads the same
// file, and two sources of truth for one checkbox is how they drift apart.
async function refreshSettings() {
  try {
    const s = await invoke<Settings>("get_settings");
    autoUpdate.checked = s.autoUpdate;
    autoUpdate.disabled = false;
  } catch {
    // Can't read it — don't show a checkbox whose state is a guess.
    autoUpdate.disabled = true;
  }
}

autoUpdate.addEventListener("change", async () => {
  const wanted = autoUpdate.checked;
  autoUpdate.disabled = true;
  try {
    const s = await invoke<Settings>("set_auto_update", { enabled: wanted });
    autoUpdate.checked = s.autoUpdate;
    // The version badge phrases itself differently depending on this, so tell it.
    window.dispatchEvent(new CustomEvent("jarlid:auto-update", { detail: s.autoUpdate }));
  } catch {
    autoUpdate.checked = !wanted; // roll back rather than lie about what was saved
  } finally {
    autoUpdate.disabled = false;
  }
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
