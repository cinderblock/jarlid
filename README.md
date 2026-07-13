# Jarlid

**Jarlid** (the lid of the jar — in the original myth, Pandora opened a *pithos*) is an
unofficial desktop client for a **paid Pandora account**, built after Pandora discontinued their
official desktop app. It wraps Pandora's own authenticated web player (so login, DRM, ad-free
playback, and skips are all handled by Pandora) and overlays a redesigned UI that puts the
current song's **album art and synced lyrics front and center**.

## Features

- **Synced (karaoke-style) lyrics** from [LRCLIB](https://lrclib.net), with duration-aware
  version matching, a permanent on-disk cache, and per-track sync nudging (`[` / `]`, ±0.25 s).
- **Native Windows media integration**: hardware media keys and the volume-flyout / lock-screen
  media panel (title, artist, album art, live state) via a real SMTC session — not the
  browser's flaky MediaSession bridge.
- **Transport & stations**: play/pause (Space), skip, thumbs, replay, searchable station
  picker, recently-played art gallery with per-track detail modal.
- **Network player (UPnP/DLNA + WiiM) remote mode**: when local playback is idle and a
  renderer on the LAN is playing, Jarlid becomes its display — "Now playing on …" with art and
  synced lyrics. WiiM devices use the native LinkPlay API, so metadata works for the WiiM's own
  sources too, and Jarlid can start playback on the device from its preset list and control
  play/pause/skip.
- **Self-healing engine**: heartbeat watchdog auto-reloads a wedged Pandora page and
  auto-dismisses the "still listening?" prompt for all-day sessions.
- **Auto-updates**: checks GitHub Releases (startup + every 4 h) and installs signed updates
  with one click from an in-app banner.
- Full station collection searchable in the picker; Cover-Flow-style recently-played gallery;
  WiiM volume slider in remote mode; window position/size persist across launches (kill-safe).

## Why this approach

We deliberately wrap the real `pandora.com` web player instead of using the reverse-engineered
"partner API" (pianobar / Pithos / pydora). That keeps the account ban-safe and durable: it's a
normal browser session, not an impersonated official client. See `plans/pandora-desktop-app.md`
for the full rationale, the official-API investigation, and the living task list.

## Architecture

Two decoupled contexts inside one Tauri (Rust + WebView2) app:

- **Engine webview** → `pandora.com`. Handles auth/audio/transport. A single injected script,
  `app/src-tauri/bridge.js`, is the only code that touches Pandora's DOM: it scrapes now-playing
  state via Pandora's stable `data-qa` hooks, sets `navigator.mediaSession` (→ Windows SMTC), and
  exposes commands the host can call.
- **UI window** → our own Vite app (`app/src/`). Renders the art + synced-lyrics interface and
  transport. Lyrics come from [LRCLIB](https://lrclib.net) (free, time-synced) via a Rust command
  (`fetch_lyrics`) that sidesteps page CSP/CORS.

State flows engine → Rust events → UI. Controls flow UI → Rust → `eval` into the engine.

Two more Rust-side services complete the picture: a native Windows SMTC session (`souvlaki`)
fed by the same engine events, and a network-player watcher (`app/src-tauri/src/upnp.rs`) that
discovers a renderer via SSDP and reads it directly — LinkPlay/WiiM native HTTP API first,
generic DLNA AVTransport as fallback.

## Develop

```sh
cd app
bun install
bun run tauri dev
```

Requirements: Rust, Bun, and the WebView2 runtime (all already present on the dev machine).
First launch: log in to Pandora once in the engine window; the session persists (WebView2 user
data keyed to app id `com.camer.pandora-desktop`).

## Install

Grab `Jarlid_<version>_x64-setup.exe` from the
[latest release](https://github.com/cinderblock/jarlid/releases/latest) and run it. The app
keeps itself up to date from there. First launch: log in to Pandora once in the engine window
(reveal it with the globe button or `Ctrl+Shift+E`); the session persists across updates.

To build locally instead: `cd app && bun run tauri build` (requires Rust, Bun, WebView2;
releases are built by `.github/workflows/release.yml` on version tags).

## Status

Daily-driver ready. Remaining ideas are tracked in `plans/pandora-desktop-app.md`.

## Disclaimer

Jarlid is an unofficial, personal project. It is not affiliated with, endorsed by, or supported
by Pandora Media or SiriusXM; "Pandora" is used only to identify the service this client
connects to. It does not circumvent DRM or provide any content itself — it drives
Pandora's own authenticated web player in an embedded browser, so it requires your own valid
(paid) Pandora account. Use it in accordance with Pandora's Terms of Service.

## License

[MIT](LICENSE) © Cameron Tacklind
