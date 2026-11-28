# Lyrics pane: carry-over fix and a GPU visualizer

## Goal

1. When a song ends and the next has no lyrics (or only a few rows), the last song's rows must
   leave the pane — scroll off the top — instead of staying half visible.
2. An optional, GPU-rendered music visualizer, in the manner of MilkDrop / Winamp / the old
   iTunes visualizer (the user's steer, 2026-11-23).
3. When a song has no lyrics, the pane shows the visualizer instead of "No lyrics found".
4. (2026-11-23, round 2) An option to run the visualizer behind the whole main window.
5. An option to run it in its own window, which can be fullscreened.
6. In that window, optional lyrics one line at a time, like subtitles.

## Decisions

- **Bug cause**: the carry-over (`clearPane` in `app/src/main.ts`) keeps the last song's rows
  above the new ones and scrolls down to `songTop()`. The content past that point was only the
  new rows plus `--lyrics-pad` (32vh) of bottom padding, shorter than the pane, so the scroll
  clamped short and the old rows stayed on screen. Fix: while a carried-over block exists,
  `#lyrics::after` is a spacer a pane's content-height tall (`--carry-room`, measured in
  `clearPane`); it goes with the block.
- **Where the audio comes from**: the webview has no audio (Rust decodes and plays it), so the
  samples are tapped in the output callback, after mixing and before the volume gain.
  `audio::Scope` is a fixed ring of atomics: no allocation or locking on the audio thread.
  Reads end `device_audio` frames behind the write head so the picture keeps time with the
  speaker. Per player (`Player::scope`); the engine publishes the live one.
- **Analysis in Rust, drawing on the GPU**: `app/src-tauri/src/visualizer.rs` runs a 4096-point
  FFT (`rustfft`) at 60 Hz only while the visualizer is on screen, and streams 64 log bands,
  level, low end, an onset pulse, and 256 waveform points to the page over a Tauri `Channel`
  as raw bytes. Levels are relative to the song's recent peak (AGC).
- **Renderer**: WebGPU feedback loop (`app/src/visualizer.ts`): each frame warps the previous
  one (zoom, rotate, ripple, hue drift, decay), draws the waveform (line, ring or spiral)
  additively into it, then presents with an optional mirrored echo. Scene parameters are
  randomised and eased; a new scene comes on a strong beat after 14 s, or at 30 s.
- **Themes**: dark theme draws light over the pane (premultiplied alpha); light theme gets an
  opaque dark rounded screen, because light-on-light washed out to pastels.
- **Failure is loud**: no WebGPU → the pane says why instead of drawing nothing.
- **Remote (speaker) mode**: no local samples, so no visualizer; "No lyrics found" as before.
- **Setting**: Appearance → "Visualizer when a song has no lyrics", on by default.

- **Round 2 shape**: the renderer becomes a class (`visualizer-render.ts`) any canvas can
  host; one analysis channel per webview (Rust keys subscribers by webview label), shared by
  every view on that page. Background mode: a fixed canvas above the art scrim, dimmed, under
  the UI; it replaces the pane visualizer while on. Standalone window: `visualizer.html`, a
  second Vite entry, label `visualizer`, opened and closed by Rust from the `window` setting;
  closing it turns the setting off; double-click / F11 fullscreen, Esc leaves. Subtitles: the
  main window emits the current synced line (`lyrics://line`) on change; the window shows it.
  Settings: `visualizer { noLyrics, background, window, windowLyrics }` (renamed from
  `enabled`, never released).

## Progress

- [x] Carry-over fix (`23b6242`)
- [x] `audio::Scope` + tap in the output callback, tests
- [x] Engine publishes the live player's scope
- [x] `visualizer.rs`: FFT + waveform thread, `visualizer_start` / `visualizer_stop`, tests
- [x] `app/src/visualizer.ts`: WebGPU feedback renderer
- [x] Pane shows the visualizer for songs with no lyrics; setting
- [x] README feature line
- [x] Checks: `cargo test` (audio, engine, app), `bun run build`, `bun test`, clippy clean on
      the new code
- [x] Watched in the real app with real music (user: "looking good")
- [x] Round 2: renderer as a class (`visualizer-render.ts`), per-webview analysis
- [x] Background mode
- [x] Standalone window + fullscreen (`visualizer.html`, `visualizer-window.ts`)
- [x] Subtitle lyrics in the window: off / line / word (`subtitles.ts`, tested); `L` cycles
- [x] Settings (own Visualizer section), README, checks, commit
- [x] Released in v1.28.0 (https://github.com/cinderblock/jarlid/releases/tag/v1.28.0)
- [x] Bug #1 (https://github.com/cinderblock/jarlid/issues/1): stack overflow in scene changes, fixed
- [ ] Watched in the real app: background mode, the window, full screen, subtitles (user)

## Findings

- Headless Chrome (`--enable-unsafe-webgpu`) renders the WebGPU pipeline fine, so a harness
  page with a stubbed `__TAURI_INTERNALS__` feeding synthetic frames is enough to judge the
  look without the app. Harness and screenshots were in the session scratchpad.
- Measured the bug in the harness: without the spacer the pane stopped at `scrollTop` 989 of
  the 1333 needed; with it, 1333.
- Tauri `Channel<InvokeResponseBody>` with `Raw` bytes arrives in JS as an `ArrayBuffer`
  (small payloads via eval, larger via the channel fetch command).
- The window page was checked headless the same way: word mode and line mode both render over
  the picture with a stubbed event bridge. Background mode was not screenshotted.
- The visualizer window gets its own capability (`capabilities/visualizer.json`) for
  fullscreen and events; app commands need no capability.
- `cargo clippy -D warnings` already fails on pre-existing code with the current toolchain;
  logged in `ISSUES.md`.

- v1.28.0 bug, issue #1: `Uncaught RangeError: Maximum call stack size exceeded` from
  `visualizer-render`. `Scenes.now()` called itself for the outgoing scene, and past 30 s the inner
  call took the same branch forever. Its clock also started at page load, so a view first shown
  after 30 s crashed on frame one. Scenes now live in `visualizer-scenes.ts` with tests. A frame
  that throws now stops the view and shows the error, instead of throwing every frame.
