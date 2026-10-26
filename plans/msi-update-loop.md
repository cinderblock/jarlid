# MSI update loop — Living Plan

Plan path: `plans/msi-update-loop.md`

## Goal

v1.15.0 was released 2026-10-26 12:57 PM PDT (19:57Z). The local v1.14.1 install restarts to
update and never comes back. Make it update, and make sure no install can get stuck this way
again: an update that fails must not kill the app on every song boundary.

## Environment / context

- Machine: this Windows 11 box, user `camer`. Install dir `C:\Users\camer\AppData\Local\Jarlid`.
- `tauri-plugin-updater` 2.10.1, `tauri-utils` 2.9.3. Release built by `tauri-apps/tauri-action@v0`
  in `.github/workflows/release.yml`, `bundle.targets: "all"` (NSIS setup.exe **and** MSI).
- Two uninstall entries for Jarlid exist on this machine:
  - NSIS, DisplayVersion 1.14.0, `uninstall.exe` (installed 2026-10-19 9:34 PM PDT)
  - MSI, DisplayVersion 1.14.1, ProductCode `{9E0E5BEA-BE5A-4E62-88A1-D3EA8BA877AC}`

## Findings

1. **What failed.** Windows Installer event log (Application, source MsiInstaller): both
   attempts ran `Jarlid-1.15.0-installer.msi` and failed with status 1603 —
   `Error 1730. You must be an Administrator to remove this application.` —
   at 12:59:57 PM PDT (19:59Z) and 1:20:50 PM PDT (20:20Z). The MSI's major upgrade has to
   remove the 1.14.1 MSI product and can't without elevation.
2. **Why the app never comes back.** For an MSI the plugin runs
   `msiexec /i <msi> /quiet /promptrestart AUTOLAUNCHAPP=True` via `ShellExecuteW` and then
   `std::process::exit(0)` (`updater.rs:817-865`). Relaunch is the MSI's job; a failed MSI
   relaunches nothing. `/quiet` also means no UAC prompt is ever shown.
3. **It loops.** On the next launch the old version finds 1.15.0 again, stages it, and
   installs at the next track boundary — exiting again. Nothing records that the last
   attempt failed.
4. **How this machine ended up on the MSI.** The updater picks the `latest.json` entry from
   a marker the bundler patches into the exe (`__TAURI_BUNDLE_TYPE_VAR_NSS/_MSI/_UNK`), trying
   `windows-x86_64-<type>` then `windows-x86_64`. tauri-action points the generic entry at the
   **MSI**. Markers found:
   - `jarlid.exe.prev` (2026-10-19 10:20 PM PDT): `_UNK` — a raw `target/release` build had
     been copied over the NSIS install.
   - `jarlid.exe` (now): `_MSI` — that unmarked build updated itself through the generic
     entry, i.e. the MSI, on 2026-10-20 9:45 AM PDT (succeeded, status 0; it was a fresh MSI
     install with nothing to remove).
5. **Install type is sniffed from bytes**, not the marker (`extract_exe`, `updater.rs:920`):
   whatever file the chosen entry points at is what runs.

## Plan / steps

1. [x] Diagnose (above).
2. [x] Code (`5783a85`): `build_updater` always asks for `windows-x86_64-nsis` on Windows, whatever the
   exe's marker says. Per-user NSIS needs no admin and relaunches the app itself (`/UPDATE`).
3. [x] Code (`5783a85`; badge text swept into peer commit `dda07e3`): failed-install guard. Record the version being installed before `install`; on
   launch, if that marker names a version newer than the one running, the install failed:
   don't auto-install that version again, say so in the badge (manual click can still try).
4. [x] Release config (`5783a85`): stop shipping the MSI (`bundle.targets: ["nsis"]`), so `latest.json` only
   has NSIS entries. MSI-marked 1.14.1 then falls back to generic `windows-x86_64` → NSIS.
   This is what un-sticks already-installed MSI copies without touching them.
5. [x] This machine (2026-10-26 2:24 PM PDT): data backed up to
   `%TEMP%\jarlid-reinstall\data-backup`; `msiexec /x {9E0E5BEA-…}` elevated (exit 0); NSIS
   1.15.0 setup `/S` (exit 0). Now one registration (NSIS 1.15.0), exe marker `_NSS`, running.
   `jarlid.exe.prev` (the Oct 19 unmarked build) left in the install dir untouched.
6. [x] Released v1.16.0 (`54eb108`), carrying 2–4 plus a peer's draft-lyrics feature.
7. [ ] Confirm: CI result, `latest.json` lists only the NSIS setup, and the running 1.15.0
   updates itself to 1.16.0 at a song end and comes back.

## Findings during the fix

- C: ran out of space mid-way (0 bytes free; `~/.cargo/build` was 269 GB, 127 GB of it
  `debug\incremental`). Symptoms: `LNK1318 Unexpected PDB error; LIMIT (12)`, then
  `os error 112` / `LNK1180`. The user freed space themselves.

## Things not to do

- Don't copy a raw `target/release/jarlid.exe` over an install: it has no bundle marker, and
  an unmarked exe updates itself onto the MSI track.
- Don't hand-edit `latest.json` on a published release without the user's say-so.
