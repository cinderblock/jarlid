# Jarlid

**Jarlid** (the lid of the jar — in the original myth, Pandora opened a *pithos*) is an
unofficial desktop client for a **paid Pandora account**, built after Pandora discontinued their
official desktop app. It wraps Pandora's own authenticated web player (so login, DRM, ad-free
playback, and skips are all handled by Pandora) and overlays a redesigned UI that puts the
current song's **album art and synced lyrics front and center**, with full **Windows system
media controls** (media keys / volume-flyout / lock screen).

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

## Develop

```sh
cd app
bun install
bun run tauri dev
```

Requirements: Rust, Bun, and the WebView2 runtime (all already present on the dev machine).
First launch: log in to Pandora once in the engine window; the session persists (WebView2 user
data keyed to app id `com.camer.pandora-desktop`).

## Status

Early but working: the bridge, backend, SMTC, synced lyrics, and UI are wired up and the engine
runs hidden. Rough edges are tracked in `plans/pandora-desktop-app.md`.

## Disclaimer

Jarlid is an unofficial, personal project. It is not affiliated with, endorsed by, or supported
by Pandora Media or SiriusXM; "Pandora" is used only to identify the service this client
connects to. It does not circumvent DRM or provide any content itself — it drives
Pandora's own authenticated web player in an embedded browser, so it requires your own valid
(paid) Pandora account. Use it in accordance with Pandora's Terms of Service.

## License

[MIT](LICENSE) © Cameron Tacklind
