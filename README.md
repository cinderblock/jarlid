# Jarlid

**Jarlid** (the lid of the jar — in the original myth, Pandora opened a *pithos*) is an
unofficial desktop client for a **paid Pandora account**, built after Pandora discontinued their
official desktop app. It is a **reimplementation of Pandora's client protocol**: it speaks
Pandora's own APIs directly, decodes and plays the audio itself, and puts the current song's
**album art and synced lyrics front and center**. There is no browser and no embedded web player
anywhere in it.

## Features

- **Synced (karaoke-style) lyrics** from [LRCLIB](https://lrclib.net), with duration-aware
  version matching, a permanent on-disk cache, and per-track sync nudging (`[` / `]`, ±0.25 s).
- **Native Windows media integration**: hardware media keys and the volume-flyout / lock-screen
  media panel (title, artist, album art, live state) via a real SMTC session, fed from the player
  itself rather than inferred from a page. Plus a **taskbar thumbnail toolbar** (thumbs down ·
  replay · play/pause · skip · thumbs up) under the taskbar hover preview, drawn with the same
  artwork as the in-app transport and following the system light/dark theme.
- **Transport & stations**: play/pause (Space), skip, thumbs, replay, searchable station
  picker, recently-played art gallery with per-track detail modal.
- **Station Modes**: switch a station between My Station, Crowd Faves, Discovery, Deep Cuts,
  Newly Released, Artist Only, Energy Boost and Relax, with Pandora's own description of each
  shown inline. Switching clears the queued tracks so the change is audible within a song or two.
- **QuickMix shows its source**: QuickMix blends dozens of your stations, and each track says
  which one it came from — otherwise there's no way to tell what you're actually listening to.
- **Network player (UPnP/DLNA + WiiM) remote mode**: when local playback is idle and a
  renderer on the LAN is playing, Jarlid becomes its display — "Now playing on …" with art and
  synced lyrics. WiiM devices use the native LinkPlay API, so metadata works for the WiiM's own
  sources too, and Jarlid can start playback on the device from its preset list and control
  play/pause/skip.
- **Native audio**: 128 kbit/s MP3 (double Pandora's 64 kbit/s default), decoded and resampled to
  the output device's rate, streamed with a lock-free ring buffer. Playback position is measured
  from frames actually delivered to the device, which is what keeps synced lyrics honest — decoding
  runs several seconds ahead of what you hear.
- **Playback that survives being left alone**: a long pause releases the stream and the audio
  device rather than sitting on a connection that the far end will quietly drop, and pressing play
  re-opens the track and seeks back to the second you paused on. The same watchdog covers a stream
  that stalls mid-song or an output device that disappears (headphones unplugged, monitor asleep):
  it rebuilds where you were instead of going silent until you hit skip. Because it watches the
  *decoder* rather than the playhead, a stalled read is caught with the whole ring buffer still in
  hand — the re-open usually finishes before there is anything to hear.
- **Stations page**: the whole collection on one screen — search it, click to play, or switch to
  Select mode to pick stations for export. QuickMix, Thumbprint and genre stations are tagged,
  which matters because a QuickMix is a shuffle *over* other stations and so has no seeds or
  thumbs of its own.
- **Export your station preferences**: save any or all stations' thumbs up/down, seed artists &
  songs, and per-station settings to a single versioned JSON file. Your listening history lives
  only in your Pandora account and has no official export; this gives you a copy you own. The
  walk is deliberately serial and paced, and stops rather than retrying if Pandora pushes back.
  Re-importing is not built yet — and note Pandora only accepts a thumb for a track it has just
  served you, so thumbs will only ever be restorable opportunistically; seeds and stations
  restore cleanly.
- **Settings**: which account is signed in and a way to sign out (which clears the saved
  password from the Windows Credential Manager), plus how updates should arrive.
- **Updates that land between songs**: the installer is downloaded and signature-verified in
  the background, then installed in the gap *after* a track rather than through the middle
  of one. Jarlid restarts itself and picks the station back up, so an update costs a
  slightly longer pause rather than an interruption. You choose the behaviour — install
  after the current song (default), install immediately, download but ask first, or just be
  told a version exists — and how often to look, from every 30 minutes to once a day at a
  wall-clock hour you are not listening at. It will not restart during a station export,
  while a network player owns playback, or while paused.
- Full station collection searchable in the picker; Cover-Flow-style recently-played gallery;
  WiiM volume slider in remote mode; window position/size persist across launches (kill-safe).

## Why this approach

Jarlid began as a wrapper around Pandora's web player in an embedded WebView2, scraping its DOM
for metadata and clicking its buttons to control playback. That worked, but it meant shipping a
whole browser to play a 128 kbit/s stream, and every piece of state was inferred from obfuscated
React markup that could change at any time.

It is now a genuine client. The protocol was reverse-engineered from Pandora's own traffic and
verified against a live account:

- **No DRM to contend with.** The audio is plain MP4/AAC or MP3 over signed, expiring HTTPS URLs.
  Not one of `pssh`/`sinf`/`schm`/`tenc` appears in the container — verified structurally, not
  assumed.
- **Better audio than the web player's default.** The standard stream is 64 kbit/s HE-AAC;
  requesting `HTTP_128_MP3` yields **128 kbit/s**, measured from the file rather than read off a
  label.
- **Authoritative state.** Because we own the player, "paused" and playback position are facts
  rather than inferences from DOM motion.

The trade-off, stated plainly: this is against Pandora's Terms of Use, which prohibit third-party
clients. It requires your own paid account, redistributes nothing, and circumvents no DRM — but
it is not sanctioned. See `plans/pandora-native-client.md` for the full protocol research,
measurements, and the living task list.

## Architecture

A Tauri app whose only webview is our own UI. Three library crates do the real work:

- **`crates/pandora`** — the protocol. Login over the tuner API (Blowfish-encrypted, and the only
  endpoint not behind PerimeterX bot detection), then Pandora's modern REST API for the station
  collection, and the tuner API for playlists, audio and feedback. Portable, no platform code.
- **`crates/audio`** — decoding and playback. Windows Media Foundation decodes HE-AAC *with SBR*
  (Symphonia implements neither SBR nor PS, and would silently decode at half the sample rate),
  resampling to the output device's rate; a lock-free ring buffer feeds cpal.
- **`crates/engine`** — the radio. Queue refill, auto-advance, transport, thumbs, station Modes,
  and credentials in the Windows Credential Manager.

`app/src-tauri/src/native.rs` drives the engine and emits the same `engine://` events the old
bridge script did, so the UI, media integration and lyrics needed no changes when the webview was
removed. Lyrics still come from [LRCLIB](https://lrclib.net) via a Rust command.

State flows engine → Rust events → UI. Controls flow UI → Rust → engine — direct method calls
rather than `eval`-ing JavaScript into someone else's page.

Three more Rust-side services complete the picture: a native Windows SMTC session (`souvlaki`)
fed by the same engine events; the taskbar thumbnail toolbar (`app/src-tauri/src/thumbbar.rs`) —
a separate shell API that SMTC knows nothing about, so it registers its own buttons and
subclasses the window to receive their clicks; and a network-player watcher
(`app/src-tauri/src/upnp.rs`) that discovers a renderer via SSDP and reads it directly —
LinkPlay/WiiM native HTTP API first, generic DLNA AVTransport as fallback.

Both media surfaces route through one shared action dispatcher, so a press behaves identically
whether it came from a media key or the taskbar. The toolbar's glyphs are extracted from
`app/index.html` by `build.rs`, keeping them in lockstep with the in-app transport icons.

## Develop

```sh
cd app
bun install
bun run tauri dev
```

Requirements: Rust, Bun, and the WebView2 runtime (WebView2 renders Jarlid's own UI; it no longer
loads pandora.com). First launch: sign in with your Pandora account — the password goes to the
Windows Credential Manager, encrypted for your user.

**Build with the Tauri CLI, not bare `cargo build`.** Only the CLI embeds the built frontend; a
plain `cargo build --release` produces a binary that tries to load the dev server and fails with
`ERR_CONNECTION_REFUSED`.

`scripts/stress-window-move.ps1` is a regression test for the window-state saver deadlock fixed
in v0.6.12 — it hammers the window with moves timed to collide with the debounced state save.
Worth running against a release build after touching window-state or event-loop code:

```sh
pwsh scripts/stress-window-move.ps1 -Exe app/src-tauri/target/release/jarlid.exe
```

## Install

Grab `Jarlid_<version>_x64-setup.exe` from the
[latest release](https://github.com/cinderblock/jarlid/releases/latest) and run it. The app
keeps itself up to date from there. First launch: sign in with your Pandora account; the
credentials persist across updates.

To build locally instead: `cd app && bun run tauri build` (requires Rust, Bun, WebView2;
releases are built by `.github/workflows/release.yml` on version tags).

## Status

Daily-driver ready. Remaining ideas are tracked in `plans/pandora-desktop-app.md`.

Station-preferences export is newly built. Its API layer is verified against a live account
(`cargo run -p engine --example dump-station-shape`) and the Stations/Settings pages render
correctly, but the export button has not yet been clicked end-to-end in the app — select,
export, save dialog, file on disk. Import is not started. See `plans/station-prefs-export.md`.

## Disclaimer

Jarlid is an unofficial, personal project. It is not affiliated with, endorsed by, or supported
by Pandora Media or SiriusXM; "Pandora" is used only to identify the service this client
connects to. It provides no content itself and requires your own valid (paid) Pandora account.

It circumvents no DRM, because Pandora applies none to this audio — the streams are unencrypted
and the only protection is a signed, expiring URL, which Jarlid uses exactly as intended rather
than working around. It does, however, access Pandora through a client they did not write, which
their Terms of Use prohibit. That is a real risk to your account, knowingly taken.

## License

[MIT](LICENSE) © Cameron Tacklind
