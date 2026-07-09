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

## Queued (user requests, not yet done)
- Title marquee: DONE (hover-scrub, round 3). VERIFY with user.
- Lighter GPU flag (`--use-angle=gl`) instead of full `--disable-gpu`.
- Confirm Windows SMTC works (media keys / volume flyout).
- Lyrics matching for parenthetical titles: VERIFY round-3 fix with user.
- Station full-collection list (scrape My Collection) if rail-only is insufficient.
- Q4 (engine-less design) answered: NO without abandoning ban-safe web-session approach — engine
  IS the auth+DRM+audio backend; hidden window ≈ idle WebView2 process (~150MB, ~0% CPU). The
  alternative (direct REST + own audio pipeline) = unofficial-client territory = ban risk (see
  "Strategy"). Don't reopen without new info (e.g. official API access granted).

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
