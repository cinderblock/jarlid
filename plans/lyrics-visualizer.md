# Lyrics pane: carry-over fix and a GPU visualizer

## Goal

1. When a song ends and the next has no lyrics (or only a few rows), the last song's rows must
   leave the pane — scroll off the top — instead of staying half visible.
2. An optional, GPU-rendered music visualizer, in the manner of MilkDrop / Winamp / the old
   iTunes visualizer (the user's steer, 2026-11-23).
3. When a song has no lyrics, the pane shows the visualizer instead of "No lyrics found".

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
- [ ] Watched in the real app with real music (user)

## Findings

- Headless Chrome (`--enable-unsafe-webgpu`) renders the WebGPU pipeline fine, so a harness
  page with a stubbed `__TAURI_INTERNALS__` feeding synthetic frames is enough to judge the
  look without the app. Harness and screenshots were in the session scratchpad.
- Measured the bug in the harness: without the spacer the pane stopped at `scrollTop` 989 of
  the 1333 needed; with it, 1333.
- Tauri `Channel<InvokeResponseBody>` with `Raw` bytes arrives in JS as an `ArrayBuffer`
  (small payloads via eval, larger via the channel fetch command).
- `cargo clippy -D warnings` already fails on pre-existing code with the current toolchain;
  logged in `ISSUES.md`.
