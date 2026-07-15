# Pandora Desktop App — Living Plan

Plan path: `plans/pandora-desktop-app.md`

## Goal
Recreate a desktop Pandora client for the user's **paid** Pandora account (their official
desktop app access was disabled). Redesign the UI to put **current song art + synced lyrics
front and center**. Do it the "best" (durable, ban-safe) way, not the fastest.

## Strategy (decided)
Wrap Pandora's **real authenticated `pandora.com` web player** inside a desktop shell and
overlay a custom UI. This uses Pandora's own web client (normal login, DRM, ad-free paid tier,
skips, stream URLs all handled by them) — so it won't get the account banned and survives
Pandora changes far better than the reverse-engineered partner-API route (pianobar/Pithos),
which is fragile and ban-prone. **We are NOT using the reverse-engineered partner API.**

## Architecture (decided 2026-07-08)
Decoupled two-context design (chosen over injecting an overlay onto pandora.com, which fights
Pandora's CSP for our fonts/styles/lyrics fetch and their obfuscated React DOM):
- **Engine webview** → `pandora.com`. Handles auth, audio, transport, DRM. Hidden once logged in
  (shown for login). An injected `bridge.js` (Tauri initialization script) is the ONLY thing that
  touches Pandora's DOM: it extracts now-playing state and executes transport commands.
- **UI window** → our own Vite app. Full creative control, no CSP constraints. Renders big art +
  synced lyrics + transport. Fetches lyrics via a Rust command (bypasses CSP/CORS).
- **Bridge:** bridge.js -> Rust (events) -> UI for state; UI -> Rust -> `engine.eval(js)` for control.
- **Lyrics fetch** goes through a Rust `fetch_lyrics` command (HTTP to LRCLIB) to dodge CSP/CORS.

## System media controls (CRITICAL — user requirement)
Windows SMTC (media-key overlay, lock screen, hardware keys) is a must-have, not nice-to-have.
- **Primary approach:** set `navigator.mediaSession` metadata + action handlers inside the engine
  webview. WebView2 is Edge/Chromium, which bridges MediaSession → Windows SMTC automatically when
  a real media element is playing. Handlers (play/pause/nexttrack) drive Pandora's transport.
  Note: Pandora radio has no "previous track"; skips may be limited by tier.
- **Fallback if WebView2 doesn't bridge MediaSession:** the `souvlaki` Rust crate (SMTC via
  windows-rs) driven from the Rust side using state from the bridge. Verify primary first.

## Official partner API — investigated 2026-07-08 (parallel lottery ticket, not the plan)
An official `developer.pandora.com` GraphQL API exists and DOES real playback (audio URLs,
skip/pause/play/shuffle/replay, thumbs, search, collections, honors Free/Plus/Premium tiers) —
the sanctioned version of pydora's hack. **If attainable it would be strictly better** (drop the
webview, pure native UI on an official API, no ban risk). BUT it's a **B2B/OEM partner program**,
not self-serve: the access form wants company name/address/URL/size/countries/distribution/target
audience/**projected sales**. A Feb 2025 community note says Pandora is **not exploring new
partnerships**. Verdict: low odds for an individual. Plan: optionally submit an honest access
request (Claude can DRAFT, user must SUBMIT — no account creation/form submission on user's behalf).
If granted → pivot architecture to official API. If not (likely) → web-wrapper is the working
baseline, needs no approval. **Web-wrapper build continues regardless.**

## Decisions already made (don't re-ask)
- **Shell:** Tauri (Rust + OS webview = WebView2 on Windows). Small binary, low memory.
- **Integration:** Hybrid — Claude picks per feature. Likely DOM/JS injection for controls
  (click real buttons / drive the player) and Pandora's internal REST API (auth token lifted
  from the logged-in session) for clean now-playing metadata where that's cleaner.
- **Lyrics:** Synced (karaoke) via **LRCLIB** (free, no API key, time-synced with plain-text
  fallback). Genius/Musixmatch only considered later if coverage is poor.
- **Album art:** Read the high-res art URL from player state; no separate provider.

## Environment / context
- Primary dir: `C:\Users\camer\git\Personal Projects\Pandora` (was empty; not yet a git repo).
- Toolchain (verified 2026-07-08): cargo/rustc 1.94.1, node v24.11.1, bun 1.3.0, pnpm 10.29.3,
  npm 11.6.2, tauri-cli 2.10.1, WebView2 runtime 131.0.2903.99.
- Frontend package manager: **bun** (user default). Use `bun run` scripts.

## Biggest open risk (validate FIRST, before building UI)
**Does Pandora's web player actually PLAY AUDIO inside WebView2?**
- Pandora web may use EME/Widevine encrypted media. WebView2 doesn't always ship Widevine
  enabled. If playback fails, options: enable Widevine component in WebView2, use a different
  webview, or reconsider approach. This spike gates everything else.
- Secondary: does login (incl. any captcha/2FA) work inside the embedded webview and persist?

## Plan / steps
1. [current] Scaffold Tauri app (bun + vanilla/framework TBD) that opens a window to pandora.com.
2. **Spike:** log in, confirm audio plays, confirm session persists across restarts. Document result.
3. Inspect the web player: find DOM selectors + internal API calls for now-playing
   (title/artist/album/art), transport (play/pause/skip/thumbs), station list. Capture via devtools/network.
4. Build the state bridge: JS injected into the webview posts now-playing + player state out to the
   Rust/host side (or a second window) on change.
5. Build the custom UI (separate Tauri window or overlay): big art, synced lyrics, minimal transport.
   Hide/background the real web player window.
6. Wire LRCLIB lyrics fetch + time-sync to playback position.
7. Controls from custom UI -> drive the web player (DOM clicks or internal API).
8. Polish: global media keys, mini-player, remember window state, art color theming.

## Findings / gotchas
- WebView2 present at 131.x — Chromium-based, so modern JS/CSS fine. Widevine status TBD (see risk).
- 2026-07-08 Widevine risk looks LOW: Pandora's own docs describe web audio only by bitrate
  (64kbps AAC+ free, up to 192kbps Plus/Premium) with no DRM mention. Historically Pandora served
  token-authenticated but *unencrypted* AAC (that's how pianobar/Pithos got direct URLs). If still
  true, no EME → WebView2 plays natively. Running spike is the definitive check.
- Scaffolded into `app/` subdir (Tauri vanilla-ts). Main window (label `pandora`) points directly at
  https://www.pandora.com with devtools enabled — first run IS the spike.
- 2026-07-08 First integration run: cross-webview event bridge WORKS (engine's injected script
  successfully emitted engine://needs-login to the UI → proves remote-page Tauri IPC via the
  `engine` capability's `remote.urls` is functioning). Good sign for the whole design.
- 2026-07-08 The engine window came up NOT logged in (spike session didn't carry to this run).
  Verified on logged-in pandora.com (Chrome): a logged-in/playing page has ZERO password fields
  and `[data-qa=playing_track_title]` IS present. So login detection must be TITLE-FIRST: if a
  title exists it's the player (never show login card); only emit needs-login when no title AND
  (password field present OR path looks login-ish). Fixed in bridge.js snapshot().
- TODO after next run: confirm login persists across app restarts (WebView2 profile is app-wide
  under target/debug/app.exe; if Pandora cookie is session-only, may need "Keep me signed in").

## Progress log
- [x] 2026-07-08 Feasibility assessed; approach + stack decided with user.
- [x] 2026-07-08 Toolchain verified — all present, no install needed.
- [x] 2026-07-08 Scaffold Tauri app (vanilla-ts) into `app/`.
- [x] 2026-07-08 **SPIKE PASSED** — user confirmed music PLAYS in WebView2, login works.
      No Widevine/EME wall. Core approach validated. Proceeding to real build.
- [x] 2026-07-08 Inspected live Pandora player (via Chrome): Pandora does NOT set MediaSession
      (so SMTC needs us), and exposes clean `data-qa` hooks (playing_track_title/artist/album,
      station_active_name, album_active_image, play/skip/replay/thumbs buttons, volume_slider).
      Art URL encodes size (_500W_500H) → upscale to _1080W_1080H.
- [x] 2026-07-08 Built v1: `bridge.js` (scrape + MediaSession + commands), Rust backend
      (`fetch_lyrics` LRCLIB, `player_cmd` eval, `show_engine`, engine webview w/ autoplay arg),
      capabilities (engine remote emit), UI (index.html/main.ts/styles.css — hero art + synced
      lyrics + transport). `cargo check` and `tsc --noEmit` both clean.
- [x] 2026-07-08 Integration WORKS after GPU-disable: engine loads+plays, event bridge fires,
      **synced lyrics render and highlight in time** in the redesigned window (confirmed via
      screenshot — "Ordinary" by Alex Warren, current line highlighted). Core design validated.
- [x] 2026-07-08 Fixed stuck "Sign in" card (transient needs-login during initial "/" load was
      re-covering the working player). Guard: main.ts `everPlayed` + bridge `everHadTitle` — never
      show login card once a real track has played.
- [ ] VERIFY next run: (1) card gone, (2) album ART displays (left column looked dark — check),
      (3) controls (play/skip/thumbs) work from redesigned window, (4) **Windows SMTC**.
- [ ] Then: switch to lighter GPU flag (try `--use-angle=gl`), set engine `visible(false)` +
      re-enable nowplaying auto-hide for the real single-window experience.

## Session/window lifecycle (2026-07-08)
- Engine window: `visible(false)` by default; shown on `engine://needs-login`, re-hidden on
  `engine://nowplaying`. Reveal manually with **Ctrl+Shift+E** (or the login-card button).
- Engine CloseRequested → `prevent_close()` + hide (closing it would destroy WebView2 + kill audio).
- Main window CloseRequested → `handle.exit(0)` (else hidden engine keeps a zombie process alive).
- OPEN: with autoplay flag removed + engine hidden, cold-start playback needs a gesture. Does the
  UI play button start audio when engine is hidden? If WebView2 blocks programmatic play(), options:
  auto-reveal engine on failed start, or find a safe autoplay enable. NEEDS USER TEST.

## Git
- Repo initialized at project root 2026-07-08. First commit b8e1627. target/, node_modules/, dist/,
  *.log ignored; lockfiles committed. Commit at each logical step going forward.

## Fixes applied 2026-07-08 (post-first-run)
- `[hidden]{display:none!important}` — our explicit `display` was overriding the attribute, so the
  sign-in card (and player toggling) never actually hid. THIS was the stuck-card cause (not logic).
- Layout: `#player` never scrolls; art/meta/controls is a fixed panel; only `#lyrics` scrolls
  (min-height:0 chain). Narrow/portrait stacks art-on-top fixed, lyrics scroll below.
- Times-broken-on-skip: bridge `audioEl()` now picks the actively-playing `<audio>` (Pandora
  preloads next track as a 2nd element; we were reading the old/ended one).

## Fixes applied 2026-07-08 (round 2)
- Lyrics not loading: title was DOUBLED ("X...X...") because Pandora's `.Marquee` CLONES
  `.Marquee__wrapper__content` for long titles. `txt()` now reads the first
  `.Marquee__wrapper__content` → clean single title → LRCLIB matches. (Short titles weren't
  cloned, which is why earlier tracks worked.)
- Album art lagging behind: art element updates a beat after the title, so a title-only change key
  left stale art. Bridge change-key now includes `data.art`; main.ts still gates lyric fetch on
  title|artist|album so art-only re-emits refresh art without refetching lyrics.
- Play button dead / autoplay: CORRECTED earlier misattribution — the STATUS_ACCESS_VIOLATION crash
  was GPU, NOT the autoplay flag (removing autoplay alone didn't fix it; disabling GPU did). So
  re-added `--autoplay-policy=no-user-gesture-required` ALONGSIDE `--disable-gpu` so the UI play
  button can start audio in the hidden engine. NEEDS USER TEST: does play now start cold, no crash?

## Fixes applied 2026-07-08 (round 3)
- Lyrics still failing on titles with parentheticals ("Somebody to Someone (I Just Wanna Fall in
  Love)") even though display title was clean → it's LRCLIB MATCHING, not doubling. Rewrote
  `fetch_lyrics`: `undouble()` (halve exactly-doubled strings, bulletproof vs any marquee leak),
  search full title THEN `simplify_title()` (strip "(...)" and " - ..."), and `pick_best()` chooses
  synced + closest-duration among results instead of blindly taking the first.
- Play button dead + wrong icon while playing: switched play/pause/toggle to control the `<audio>`
  element directly (a.play()/a.pause()) instead of clicking Pandora's button in a hidden window
  (unreliable). Added `[bridge] cmd:` + `v3` version logging to confirm which bridge runs and
  whether commands arrive. NEEDS USER TEST + engine console check.

## Round 4 (2026-07-09): user feedback → features
- Play/pause WORKS (confirmed) but icon didn't flip on click → optimistic icon flip in click
  handler (playhead events correct it within 500ms if wrong).
- Station picker: #station label is now a <select>; bridge collectStations() scrapes
  `[data-qa=now_playing_station_list_station]` rail, emits engine://stations {stations, active};
  UI onchange → cmd "station:<idx>" → bridge clicks nth rail item. (Full station browse/search
  still available in the engine window itself.)
- Engine show/hide button: globe button top-right (#engine-btn) + Ctrl+Shift+E both call new Rust
  `toggle_engine` command (uses w.is_visible()).
- Release build kicked off (`bun run tauri build`) so user gets a double-clickable exe/installer:
  binary at app/src-tauri/target/release/app.exe, installers under
  app/src-tauri/target/release/bundle/ (nsis/msi). VERIFY build succeeds + record paths.

## Round 5 (2026-07-09)
- Play/pause icon STILL wrong after optimistic flip → root cause hypothesis: `<audio>.paused` lies
  during Pandora's crossfade/preload. New source of truth: Pandora play button's aria-label
  ("Pause" while playing / "Play" while paused) via `isPausedUi()` — used for playhead events AND
  mediaSession playbackState. Also audioEl() now prefers paused-mid-track (currentTime>0) over
  preloaded-next. VERIFY with user.
- Recently-played gallery: horizontal scrollable art strip under controls (localStorage
  "history", cap 40, lazy imgs, wheel→horizontal, hover zoom). iTunes-CoverFlow-ish; flat not 3D.
- Station picker v2: button + panel with SEARCH input filtering the scraped station list.
  LIMITATION: list = Pandora's Now-Playing rail (recents, ~6); full collection would need
  scraping My Collection page — future work if user wants.
- Gotcha: `taskkill app.exe` does NOT kill the orphaned Vite dev server → next launch dies with
  "Port 1420 is already in use". Fix: netstat -ano | find 1420 LISTENING pid → taskkill //PID.

## Round 6 (2026-07-09)
- Icon STILL broken after aria fix → probe showed play_button aria-label stays "Play" even while
  audio plays (Pandora a11y bug/quirk), so isPausedUi() always said paused. STOPPED trusting
  Pandora's DOM & <audio> for state: UI now derives playing/paused from PLAYHEAD MOTION
  (position advanced >0.05s between ticks → playing; static >1.6s → paused). Cannot lie.
  bridge isPausedUi still feeds SMTC playbackState — fix similarly if SMTC state looks wrong.
- Gallery thumbnails clickable → modal (art 180px, title/artist/album, played-at). History now
  stores album + at:Date.now() (older entries lack them, handled).
- IMPORTANT context: user now runs the INSTALLED release copy (dev instance died with old
  session; no vite on :1420). Frontend changes require `bun run tauri build` + reinstall to
  reach them. Dev-mode tip: taskkill app.exe does NOT kill vite (port 1420) or vice versa.

## Round 7 (2026-07-09): ICON BUG SOLVED + Jarlid rename
- **Play/pause icon root cause (after 5 failed backend fixes): the glyphs are SVG elements and
  SVGElement has NO `.hidden` property — `playIcon.hidden = x` was a silent no-op expando.**
  The state pipeline was fine all along (proven via dev-only Rust playhead tracing: positions
  advance while playing, freeze when paused). Fix: toggle the `hidden` ATTRIBUTE
  (setAttribute/removeAttribute) — attribute selectors apply to any element. LESSON: when a UI
  element refuses to change across multiple "correct" fixes, verify the DOM write actually works.
- App renamed **Jarlid** (user chose; trademark safety). productName/window title/index title/
  README. **Identifier deliberately kept** `com.camer.pandora-desktop` → preserves WebView2
  login + localStorage. Repo left as `pandora-desktop` (descriptive; rename optional).
- v0.2.0; version badge bottom-right of UI (know which build is running); Space toggles playback.
- Env gotchas: PowerShell tool calls + screen-capture script hang in this sandbox (no PNG ever
  written; 2min timeouts) — use `powershell.exe -NoProfile` via Bash for quick things, and don't
  rely on screenshots; use Rust-side event tracing + user screenshots instead.

## Round 8 (2026-07-09)
- Icon fix CONFIRMED by user ("finally kinda works") → polish: 2s optimistic grace kills the
  pause flicker; art preload+crossfade kills the stale-art flash (v0.2.1, user confirms fixed).
- Old Pandora 0.1.0 uninstalled for user (reg entry + files gone). GOTCHA: NSIS uninstaller
  killed the RUNNING JARLID — both binaries were the template's `app.exe` and Tauri NSIS
  uninstallers kill processes by binary name. Fixed permanently in v0.2.2: crate/lib/binary
  renamed to `jarlid` (jarlid.exe in Task Manager). Uninstall trick that worked in sandbox:
  `cmd //c start //wait "" uninstall.exe //S "_?=<installdir>"` (plain /S spawn never ran).
- User already on installed Jarlid (dev instances keep dying with sessions — that's fine).

## Round 9 (2026-07-09): reliability
- User had to manually refresh a wedged engine page ("just had to refresh... to get my player
  back"). v0.2.3: bridge emits engine://heartbeat every 5s; Rust watchdog thread reloads
  pandora.com in the engine if silent >30s (w.navigate). Bridge also auto-clicks Pandora's
  "still listening?" long-session prompt (text-match scan every 4s).
- Cause of the wedge unknown (renderer crash vs Pandora stall) — if reload-loops appear in the
  wild, next step is logging what state the page was in before reload.
- Tooltips: all native title= removed per user rule (saved to memory: no-native-title-tooltips);
  custom attachTip() tooltip on history thumbs.

## Round 10 (2026-07-10): native SMTC — v0.3.0
- User confirmed SMTC does NOT work via the engine's MediaSession (WebView2 never bridges it to
  Windows). Implemented the planned fallback: `souvlaki` 0.8.3 native SMTC from Rust.
  - hwnd from main window; metadata from engine://nowplaying (cover_url = artFallback 500px);
    playback state motion-derived from engine://playhead (same logic as UI icon), throttled to
    state changes + 5s progress refresh; SMTC events → engine_cmd (play/pause/toggle/skip/replay).
  - Added --disable-features=HardwareMediaKeyHandling to WebView2 args so media keys route to
    our session, not the webview.
  - player_cmd refactored to engine_cmd() helper shared by command + SMTC callback.
- NEEDS USER TEST (v0.3.0): media keys, volume-flyout panel (title/artist/art), lock screen.
- User asked for "play-pause buttons on the start menu" — the Win11 media panel (quick settings /
  volume flyout / lock screen) IS this SMTC integration; there is no separate start-menu media API.

## Round 11 (2026-07-10): v0.3.1 — four user-reported bugs
- **Stale-track resurrection** (skip→pause→wait→play resumed the OLD track): direct
  `<audio>.play()` was hitting a leftover element from before the skips. REVERTED play/pause to
  clicking Pandora's play_button (their logic targets the current track; the original reason for
  abandoning clicks was the SVG icon bug, not the clicks). audioEl() now prefers NEWEST paused
  element (reverse iteration) so position/duration don't read stale tracks either.
- **Tripled titles** ("Red Wine Supernova"×3): marquee clones content 2-4x while ACTIVELY
  SCROLLING (single .Marquee__wrapper__content query wasn't enough). bridge derepeat() collapses
  k-times-repeated strings (k=2..4, recursive); Rust undouble() generalized the same way. This
  also broke lyric lookup → user perceived it as "play/pause broke lyrics highlighting".
- **SMTC 2-3s dead zone** after button press: Windows greys controls until the app reports its
  new state; motion-derived reporting took ~2s. SMTC callback now reports expected state
  IMMEDIATELY + 2s grace vs motion (mirror of the UI icon fix).
- **Window position memory**: tauri-plugin-window-state 2.4.1, denylist ["engine"].

## Round 12 (2026-07-10): v0.3.2 — pause dead while playing
- v0.3.1's click-the-button pause failed because **Pandora swaps the button's data-qa**:
  `play_button` exists only while PAUSED, `pause_button` only while PLAYING. clicking qa("play")
  during playback found nothing. Fix: playPauseButton() = qa(play) || qa(pause); isPausedUi() is
  now simply "does pause_button exist" (cleanest state signal yet — supersedes aria/audio checks).
- This also RESOLVES the old mystery: first-session probe saw play_button+aria"Play" while
  "playing" — that snapshot was during BUFFERING. All state signals reconciled now.

## Round 13 (2026-07-10): v0.3.3 — lyrics quality + SMTC↔UI latency
- UI lagged behind media-key presses → SMTC callback now emits `player://optimistic`
  {playing}; main.ts flips icon instantly + 2s grace (third use of the optimistic pattern).
- Lyrics cache: disk cache under app_cache_dir/lyrics/<fnv64>.json keyed artist|track|album
  (lowercased). Hits marked "(cached)". Misses NOT cached (retry later).
- Out-of-sync lyrics root cause: lookup fired before the new track's duration was known
  (playhead still on old track) → duration matching skipped → wrong song EDIT chosen from
  LRCLIB. loadLyrics now awaits waitForDuration (≤2.5s, needs pos<30s+pos<dur) before querying.
- Manual sync nudge: [ / ] = ±0.25s, persisted per track (localStorage syncoff:<key>), offset
  shown in lyrics-status. User asked "am I overthinking?" — no: version-selection was real
  (fixed above); nudge covers LRCLIB's own timing variance.

## Round 14 (2026-07-10): v0.3.4 — nudge "doesn't work"
- Handler code was correct; problem is FEEDBACK: ±0.25s rarely moves the active line instantly,
  status text tiny, and songs with only plain lyrics hit a SILENT `if (!syncedLines) return`.
- v0.3.4: every [ / ] press flashes #lyrics-status (scale+color animation), including explicit
  "No synced lyrics to nudge"; keys also matched by e.code Bracket{Left,Right}.
- If user STILL sees no flash on 0.3.4 → handler truly not firing; next step devtools/console
  in the main window (Ctrl+Shift+I) or check version badge for stale build.

## Round 15 (2026-07-12): v0.4.0 — UPnP/DLNA remote mode
- User rejected HA-mediated overlay: "read straight from the media player ourselves." Built
  direct UPnP client (`src-tauri/src/upnp.rs`): SSDP M-SEARCH for MediaRenderer, description
  probe (friendlyName + AVTransport controlURL, prefers names containing wiim/speaker), 1s
  SOAP polls of GetTransportInfo + GetPositionInfo (DIDL-Lite → title/artist/album/albumArtURI),
  emits `remote://state` on change; 5 consecutive failures → rediscover.
- UI remote mode: local idle >3s + remote playing → overlay ("NOW PLAYING ON <device>" badge,
  accent color); local playback resuming flips back ≤1s. Lyrics pipeline fully reused (cache,
  duration matching, [ ] nudge, remote position interpolated between polls). Play/pause/skip →
  `remote_cmd` (AVTransport Play/Pause/Next) = first slice of "control the WiiM" feature.
  Thumbs/replay/station/history hidden via body.remote CSS.
- HA recon (before user redirected): WiiM = media_player.tom_sawyer_labs_warehouse_wiim
  ("Speakers"), exposes title/artist/album/duration/position — confirmed the metadata exists.
- RISK (untested): whether WiiM's AVTransport reports metadata/position for NATIVE sources
  (WiiM-app Pandora etc.), not just DLNA-pushed streams. HA's wiim entity may use the LinkPlay
  HTTP API. If AVTransport comes up empty for native sources → fallback: LinkPlay API
  (http://IP/httpapi.asp?command=getPlayerStatus, hex-encoded fields).
- Queued NEW: fuller WiiM control (volume, source pick?) as future feature per user.

## Round 16 (2026-07-12): v0.4.1 — LinkPlay native API preferred
- User: "definitely use the better API if available." Discovery now probes each SSDP renderer
  for the LinkPlay HTTP API (getPlayerStatus JSON; https→http, self-signed certs accepted via
  danger_accept_invalid_certs). If present → Target::LinkPlay (reports native WiiM sources);
  else Target::Upnp AVTransport fallback.
- LinkPlay specifics (verified via wiimhome.com PDF + forums): getPlayerStatus {status:
  play/pause/stop/loading, curpos/totlen in MS, Title/Artist/Album HEX-encoded}; getMetaInfo →
  metaData{title,artist,album,albumArtURI} clean text (newer firmware); commands
  setPlayerCmd:resume|pause|next. hex_decode() passes non-hex through; "unknow(n)" placeholders
  blanked.
- UI unchanged from 0.4.0 (same remote://state contract).

## Round 17 (2026-07-12): v0.5.0 — start music on WiiM from Jarlid
- Speaker button (top-right, appears when a network player is discovered; hidden otherwise)
  → "Play on <device>" panel listing WiiM presets via `getPresetInfo` (number/name/source/
  picurl; LinkPlay strings-as-numbers handled by jnum()). Click → `MCUKeyShortClick:N` (fires
  like the hardware preset button, N=1-12). remote_cmd gained "preset:N"; new `remote_presets`
  command. Presets are the sanctioned "start playback" surface — full catalog browse lives in
  WiiM's cloud app, not the local API.
- User flow: click speaker → pick "Dove Cameron Radio" preset → WiiM starts it → remote mode
  takes the screen with lyrics. Presets are configured in the WiiM Home app (up to 12).

## Round 18 (2026-07-12): v0.5.1 — window-state save timing
- User caught it: tauri-plugin-window-state writes only on clean exit → kills (installer
  upgrade taskkill, crash) lose geometry. Added debounced saver: Moved/Resized events mark
  dirty; background thread saves via AppHandleExt::save_window_state ~800ms after the last
  event. Presets Q&A: presets ARE required to *start* WiiM playback from Jarlid (12-slot local
  API limit); remote display/lyrics work for any source; audio-capture streamer remains the
  only path past the ceiling (future).

## Round 19 (2026-07-12): v0.6.0 — "all of it" batch
- Repo renamed → github.com/cinderblock/jarlid (gh auto-updated local remote; old URL redirects).
- GPU: `--use-angle=d3d11` replaces `--disable-gpu --disable-gpu-compositing`. WATCH for
  STATUS_ACCESS_VIOLATION return → revert to disable-gpu.
- Stations: bridge refreshStationList() POSTs /api/v1/station/getStations (X-CsrfToken from
  cookie, same-origin creds; tries {"pageSize":500} then {}); emits ids alongside names;
  UI picker uses `playStation:<id>` (location.assign /station/play/<id>) when ids exist,
  rail-click fallback otherwise. UNVERIFIED against live session (Chrome tab was logged out —
  400); bridge logs "[bridge] getStations ..." in engine console for diagnosis.
- WiiM volume: RemoteState.volume (-1 = unknown/DLNA), slider in remote mode (debounced
  vol:N), hidden when volume unreported. "prev" added (LinkPlay + AVTransport Previous).
- SMTC remote routing: remote://state listener in SMTC setup owns metadata+playback when
  remote_active (= remote playing && title && local motion >3s ago); callback routes buttons
  to upnp::command (Previous→prev) vs engine_cmd; local playhead/nowplaying listeners stand
  down while remote_active.
- Updater: tauri-plugin-updater + process; startup check (release builds, 10s delay) emits
  app://update-available → UI banner → install_update command (re-check, download_and_install,
  restart). Signing: minisign keypair at ~/.tauri/jarlid.key(.pub) (NO password; PRIVATE KEY
  ONLY THERE + gh secret TAURI_SIGNING_PRIVATE_KEY — do not lose/leak). Pubkey + endpoint
  (releases/latest/download/latest.json) in tauri.conf plugins.updater;
  bundle.createUpdaterArtifacts=true. CI: .github/workflows/release.yml on v* tags
  (tauri-action, windows-latest, rust-cache). Tag v0.6.0 pushed → run 29211394389.

## Round 20 (2026-07-12): v0.6.1 — deep-page detection
- User parked engine on a deep Pandora page → detection broke: bridge only scraped Now Playing
  page elements. QA map now lists selector candidates in preference order; falls back to the
  mini player / tuner bar (mini_track_title / mini_track_artist_name / mini_track_image) that
  exists on every page. Album/station absent in mini player → reuse last-known for same
  title|artist (lastMeta) so change-keys don't churn.
- Updater: startup-only check → loop every 4h (long-running instances see new releases).
- v0.6.1 tagged → CI release → THIS IS THE FIRST AUTO-UPDATE TEST: user's 0.6.0 should show
  the banner (within ~10s of a fresh launch, or ≤4h if left running... 0.6.0 has startup-only
  check, so user must restart the app once to see it).

## Round 21 (2026-07-12): v0.6.2 — startup engine-window flash
- **AUTO-UPDATE CONFIRMED WORKING** (0.6.0 → 0.6.1 via banner).
- Engine window popped up on every cold start: "/" matched the loginish regex while the SPA
  was still redirecting → needs-login → Rust showed the window. Fix: 8s grace after injection
  (injectedAt), then only real evidence (password field, /account/sign path, or an
  a[href*=/account/sign-in] link). v0.6.2 tagged → CI.
- New release loop: edit → tag vX.Y.Z → CI publishes → user's app offers the update itself
  (startup or ≤4h). No more local installer handoffs.

## Round 23 (2026-07-12): v0.6.4 — badge-as-update-UI
- User: version click should check; update control should be unobtrusive AT the badge.
- Center banner REMOVED. #version (now a <button>) is the whole update UI: click → check
  ("checking…" → "up to date" flash or accent "update to vX"); click pending → "installing…" →
  restart. Auto-checks (4h loop) feed the same badge via app://update-available. New Rust
  `check_update` command returns Option<version>.

## Queue (updated 2026-07-12, round 22 — everything earlier DONE)
Remaining / awaiting-verification:
- VERIFY (user): full-station search works on live session (bridge getStations was unverifiable
  from a logged-out probe); SMTC album art in the volume flyout; GPU stability with
  --use-angle=d3d11 (watch for STATUS_ACCESS_VIOLATION → revert to --disable-gpu).
- Future (explicitly deferred): audio-capture → DLNA streamer (play ANY station on WiiM from
  Jarlid, past the 12-preset ceiling); WiiM source picking; official-API pivot if Pandora ever
  grants partner access; proper app icon (still default Tauri art).
- Settled questions — don't reopen: engine-less design (NO — engine is the ban-safe
  auth/DRM/audio backend); HA-mediated overlay (user chose direct device reads).

## Publish (2026-07-08)
- Target: https://github.com/cinderblock/pandora-desktop, MIT license, public.
- LICENSE added (MIT, Cameron Tacklind, 2026). README has Disclaimer (unofficial, no DRM
  circumvention, needs user's own paid account) + License section.
- gh authed as cinderblock (ssh). Repo did not exist → create + push.

## Key files
- `app/src-tauri/bridge.js` — injected into Pandora webview (the only DOM-coupled code).
- `app/src-tauri/src/lib.rs` — commands + engine window + event relay.
- `app/src-tauri/capabilities/engine.json` — grants remote pandora.com event emit/listen.
- `app/src/main.ts` — UI logic, LRC parsing, synced highlight, controls.
- Run: `cd app && bun run tauri dev`. Same app identifier `com.camer.pandora-desktop` reuses
  the spike's logged-in WebView2 session (shouldn't need to re-login).

## Open questions for the user
- (none blocking yet — spike result may raise one)

## Crash: WebView2 STATUS_ACCESS_VIOLATION on engine (2026-07-08)
Engine window crashed loading pandora.com ("This page is having a problem",
STATUS_ACCESS_VIOLATION) — a native renderer crash. Spike ran pandora.com fine, so cause was
something added since: the `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--autoplay-policy=
no-user-gesture-required` flag (destabilizes WebView2 media pipeline). REMOVED it. Also lightened
bridge.js MutationObserver (dropped characterData, 400ms debounce). Confirmed the bridge itself
works — before crashing it emitted playhead (UI play button flipped to pause glyph) + needs-login.
Trade-off: without autoplay flag, user presses play once (real gesture); radio auto-advances after.
UPDATE: removing autoplay flag did NOT fix the crash — still STATUS_ACCESS_VIOLATION. Next levers
applied: (a) `--disable-gpu --disable-gpu-compositing` (canonical WebView2 native-crash fix),
(b) bridge now only runs in the TOP frame (was injecting into every cross-origin iframe),
(c) added `[bridge]` console logging (injected/started/__TAURI__-present/emit-errors).
If it STILL crashes with GPU disabled, next isolation step = temporarily remove
`.initialization_script(bridge)` to determine bridge-vs-environment as the cause.
CONFIRMED 2026-07-08: `--disable-gpu --disable-gpu-compositing` STOPS the crash — engine window
loads pandora.com. So the crash was GPU-acceleration in WebView2. Full GPU-disable is heavy-handed
(not the long-term answer). TODO after functional verification: try lighter flags to keep GPU while
avoiding the crash — candidates in likely order: `--use-angle=gl`, `--use-angle=d3d11`,
`--disable-gpu-sandbox`, `--use-angle=swiftshader` (software = ~= disable-gpu, last resort).
Also: engine window DevTools showed nothing (even network) during/pre-load; recheck once loaded.

## Things not to do
- Do NOT set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--autoplay-policy=... — crashes the renderer.
- Do NOT use the reverse-engineered partner API (pianobar/Pithos style) — ban risk + fragile.
- Do NOT touch infrastructure (Cloudflare/DNS/servers) — out of scope, needs per-change auth.
- Do NOT commit until the spike proves the core approach works.
